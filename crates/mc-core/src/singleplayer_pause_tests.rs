use std::time::Duration;

use crate::boundary::{ConnectionId, PlayerInput};
use crate::composter::tests::{POS, data_dir, fixture, snapshot_block};
use crate::core::Core;
use crate::pause_when_empty_tests::{config, drain_until_idle, next_tick, runtime};
use crate::tick::run_tick;
use mc_data::Identifier;
use mc_world::ScheduledBlockTick;
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
#[test]
fn singleplayer_pause_holds_scheduled_block_deadline_and_entity() {
    use crate::journal::{JournalEntry, JournalWriter};
    use crate::native_checkpoint::tests::{fixture_store, read_manifest, read_player};
    use crate::{Core, CoreConfig};
    use std::time::Duration;

    let dir = std::env::temp_dir().join(format!("mc-core-sp-deadline-{}", uuid::Uuid::new_v4()));
    let mut config = CoreConfig::new(dir.clone(), 42);
    config.vanilla_data_dir = Some(data_dir());
    config.view_distance = 1;
    config.simulation_distance = 1;
    config.checkpoint_every_ticks = u64::MAX;
    let initial = Core::open(config.clone()).unwrap();
    initial.shutdown();
    let base = read_manifest(&config).unwrap().tick;
    let (mut state, player) = fixture(7);
    let uuid = state
        .entities
        .row(state.entities.slot_of(player).unwrap())
        .uuid;
    state.tick = base;
    let deadline = base + 400;
    state.block_ticks.schedule(ScheduledBlockTick::new(
        POS,
        Identifier::parse("minecraft:composter").unwrap(),
        deadline,
        0,
    ));
    let full = state.world.block_at(POS.x, POS.y, POS.z).unwrap();
    let (output, _, _) = run_tick(&mut state, &[]);
    let mut entries = crate::core::journal_entries_for_tick(&mut state, &output);
    entries.push(JournalEntry::Block {
        pos: [POS.x, POS.y, POS.z],
        state: full,
    });
    for x in -3..=3 {
        for z in -3..=3 {
            entries.push(JournalEntry::Block {
                pos: [x, 199, z],
                state: state.registry.states.stone,
            });
        }
    }
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    writer.append(base + 1, output.tick, &entries).unwrap();
    writer.flush().unwrap();
    drop(writer);

    let core = Core::open(config.clone()).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let mut rx = core.subscribe();
    core.submit(PlayerInput::Join {
        connection: ConnectionId(1),
        uuid,
        name: "owner".into(),
        permission_level: 4,
    });
    let next = |rx: &mut tokio::sync::broadcast::Receiver<crate::boundary::TickOutput>, timeout| {
        rt.block_on(async { tokio::time::timeout(timeout, rx.recv()).await.ok()?.ok() })
    };
    core.submit(PlayerInput::Command {
        connection: ConnectionId(1),
        command: "tp @s 0.5 200 0.5".into(),
        permission_level: 4,
    });
    core.submit(PlayerInput::Command {
        connection: ConnectionId(1),
        command: "summon minecraft:zombie 2.5 205 2.5".into(),
        permission_level: 4,
    });
    let mut mob_uuid = None;
    let mut held = None;
    for _ in 0..100 {
        let output = next(&mut rx, Duration::from_secs(10)).expect("joined tick");
        if snapshot_block(&output) == Some(full) {
            mob_uuid = output
                .snapshot
                .entities
                .iter()
                .find(|entity| entity.type_id == state.registry.zombie_type_id)
                .map(|entity| entity.uuid);
            if mob_uuid.is_some() {
                held = Some(output.snapshot.clone());
                break;
            }
        }
    }
    let mob_uuid = mob_uuid.expect("mob and scheduled block become visible");
    core.submit(PlayerInput::Command {
        connection: ConnectionId(1),
        command: "solaris_pause on".into(),
        permission_level: 4,
    });
    while let Some(output) = next(&mut rx, Duration::from_millis(500)) {
        held = Some(output.snapshot);
    }
    let held = held.expect("last snapshot before pause");
    assert!(core.checkpoint());
    let before = read_manifest(&config).unwrap();
    let player_before = read_player(&config, &uuid.to_string(), &state.registry.items).unwrap();
    let entity_record = || {
        fixture_store(&config)
            .unwrap()
            .checkpoint_reader()
            .unwrap()
            .read_checkpoint()
            .unwrap()
            .entities
            .into_iter()
            .find(|entity| entity.identity == *mob_uuid.as_bytes())
            .expect("checkpointed mob")
            .payload
    };
    let mob_before = entity_record();
    let scheduled = before
        .block_ticks
        .chunks
        .iter()
        .flat_map(|chunk| &chunk.ticks)
        .find(|tick| tick.pos == [POS.x, POS.y, POS.z])
        .expect("scheduled composter");
    assert_eq!(scheduled.trigger_tick, deadline);
    assert!(next(&mut rx, Duration::from_millis(600)).is_none());
    assert!(core.checkpoint());
    let after = read_manifest(&config).unwrap();
    let player_after = read_player(&config, &uuid.to_string(), &state.registry.items).unwrap();
    assert_eq!(entity_record(), mob_before, "paused mob does not change");
    assert_eq!(
        player_after.pos, player_before.pos,
        "paused player does not move"
    );
    assert_eq!(after.tick, before.tick);
    assert_eq!(after.block_ticks, before.block_ticks);
    core.submit(PlayerInput::Command {
        connection: ConnectionId(1),
        command: "solaris_pause off".into(),
        permission_level: 4,
    });
    let resumed = next(&mut rx, Duration::from_secs(10)).expect("resumed tick");
    assert_eq!(resumed.tick, before.tick + 1);
    assert_eq!(
        resumed.snapshot.day_time,
        held.day_time + 1,
        "day clock advances only on resume"
    );
    assert!(
        resumed
            .snapshot
            .entities
            .iter()
            .any(|entity| entity.uuid == mob_uuid)
    );
    assert_eq!(snapshot_block(&resumed), Some(full));
    core.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
