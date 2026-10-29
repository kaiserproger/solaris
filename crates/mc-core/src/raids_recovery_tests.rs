use super::*;
use crate::boundary::{ChangeSet, TickOutput, WorldSnapshot};
use crate::dimensions::Dimension;
use crate::effects::{ActiveEffect, EffectId};
use uuid::Uuid;

fn output(state: &TickState, changes: ChangeSet) -> TickOutput {
    TickOutput {
        tick: state.tick,
        snapshot: Arc::new(WorldSnapshot::default()),
        changes: Arc::new(changes),
        times: Default::default(),
    }
}
fn checkpoint(root: &std::path::Path, state: &mut TickState) -> CheckpointManifest {
    let config = CoreConfig::new(root.to_owned(), 42);
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    state.native_globals = store.read_checkpoint().unwrap().globals;
    // Align the world debt clock with the journal tick before capturing
    // post-images: a stale `sim_tick` stamps SimDebt outside the appended
    // batch and recovery rejects it as an invalid boundary.
    state.world.refresh_active_clock(state.tick);
    let out = output(state, ChangeSet::default());
    let entries = journal_entries_for_tick(state, &out);
    let mut writer = JournalWriter::open(root, 1).unwrap();
    writer.append(1, state.tick, &entries).unwrap();
    for dimension in [Dimension::Nether, Dimension::End] {
        let mut child = crate::core::recover_dimension_state(&config, dimension).unwrap();
        child.tick = state.tick;
        state.other_dimensions.insert(dimension, Box::new(child));
    }
    let (work_tx, work_rx) = crate::core::journal_work_channel();
    let (done, _) = std::sync::mpsc::channel();
    let tick = state.tick;
    assert!(start_checkpoint(state, &work_tx, &done, tick));
    let JournalWork::Checkpoint(job) = work_rx.recv().unwrap() else {
        panic!()
    };
    let mut manifest =
        crate::native_checkpoint::tests::read_manifest(&CoreConfig::new(root.to_owned(), 42))
            .unwrap();
    write_checkpoint(root, &job, &state.plugins, &mut writer, &mut manifest).unwrap();
    state.other_dimensions.clear();
    manifest
}
/// A recovered dimension has no online players; a ticket keeps the village
/// chunk loaded and active so the raid clock runs (inactive raids freeze).
fn activate(state: &mut TickState) {
    state.tickets.insert(
        "raid-test".into(),
        (crate::boundary::ChunkPos { x: 0, z: 0 }, 2),
    );
    state.world.ensure_blocking(&state.desired_chunks());
    state.refresh_active_chunks(state.tick);
}
#[test]
fn raids_checkpoint_restart_and_wal_wave_removal_resume_without_duplicates() {
    let root = tempfile::tempdir().unwrap();
    let (mut state, player, _) = crate::raids::tests::fixture(Some(root.path()));
    state
        .players
        .get_mut(&player)
        .unwrap()
        .give_effect(ActiveEffect::new(EffectId::BadOmen, 120000, 0));
    crate::raids::raid_omens(&mut state);
    state.tick += 600;
    crate::raids::raid_omens(&mut state);
    for _ in 0..300 {
        crate::raids::run(&mut state, &mut ChangeSet::default());
        state.tick += 1;
    }
    let expected = state.raids.clone();
    assert_eq!(expected.raids[&2].members.len(), 4);
    let manifest = checkpoint(root.path(), &mut state);
    let config = CoreConfig::new(root.path().to_owned(), 42);
    let load = |manifest: &CheckpointManifest| {
        let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
        let mut recovered = crate::native_checkpoint::recover_with_reader(
            &config,
            Arc::clone(&state.registry),
            store.checkpoint_reader().unwrap(),
        )
        .unwrap();
        let mut replay = recovered.dimensions.remove(&Dimension::Overworld).unwrap();
        if manifest.tick == replay.tick {
            replay.raids = manifest.raids.clone();
        }
        let settlement_globals = recovered.checkpoint.globals.clone();
        load_dimension_state(
            &config,
            Arc::clone(&state.registry),
            Dimension::Overworld,
            manifest,
            replay,
            store,
            &settlement_globals,
        )
        .unwrap()
    };
    let mut imported = load(&manifest);
    assert_eq!(
        imported.raids, expected,
        "native checkpoint retains authoritative raid membership"
    );
    let removed = *imported.raids.raids[&2].members.iter().next().unwrap();
    imported.remove_entity(imported.entities.slot_of_uuid(removed).unwrap());
    crate::raids::run(&mut imported, &mut ChangeSet::default());
    imported.tick += 1;
    assert!(
        !imported.raids.raids[&2].active,
        "raw recovery has not admitted village chunks"
    );
    assert!(
        !imported.raids.raids[&2].members.contains(&removed),
        "unloaded member removal must not leave a stale durable UUID"
    );
    crate::raids::restore(&mut imported).unwrap();
    let mut missing_manifest = manifest.clone();
    let missing = Uuid::from_u128(0xBAD);
    missing_manifest
        .raids
        .as_mut()
        .unwrap()
        .raids
        .get_mut(&2)
        .unwrap()
        .members
        .insert(missing);
    let mut restored = load(&missing_manifest);
    assert!(
        !restored.raids.raids[&2].members.contains(&missing),
        "world opens with an unloaded or missing member"
    );
    assert_eq!(restored.raids, expected);
    assert_eq!(
        restored.raids.raids[&2]
            .members
            .iter()
            .filter(|uuid| restored.entities.slot_of_uuid(**uuid).is_some())
            .count(),
        4
    );
    activate(&mut restored);
    crate::raids::run(&mut restored, &mut ChangeSet::default());
    assert_eq!(restored.raids.raids[&2].groups_spawned, 1);
    let ids: Vec<_> = expected.raids[&2]
        .members
        .iter()
        .map(|uuid| {
            restored
                .entities
                .body(restored.entities.slot_of_uuid(*uuid).unwrap())
                .id
        })
        .collect();
    for id in &ids {
        restored.remove_entity(restored.entities.slot_of(*id).unwrap());
    }
    let mut changes = ChangeSet {
        entities_removed: ids,
        ..Default::default()
    };
    crate::raids::run(&mut restored, &mut changes);
    restored.tick = manifest.tick + 1;
    // The tick bump above moves the journal batch forward; re-stamp the
    // restored debt images at the new tick so the tail stays inside it.
    restored.world.refresh_active_clock(restored.tick);
    let out = output(&restored, changes);
    let entries = journal_entries_for_tick(&mut restored, &out);
    let mut writer = JournalWriter::open(root.path(), 1).unwrap();
    writer
        .append(restored.tick, restored.tick, &entries)
        .unwrap();
    writer.flush().unwrap();
    // Obsolete external saved data cannot override the native checkpoint + WAL.
    let directory = Dimension::Overworld
        .directory(root.path())
        .join("data/minecraft");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("raids.dat"),
        b"obsolete non-authoritative data",
    )
    .unwrap();
    let mut replayed = load(&manifest);
    assert_eq!(replayed.raids, restored.raids);
    assert!(replayed.raids.raids[&2].members.is_empty());
    activate(&mut replayed);
    for _ in 0..299 {
        crate::raids::run(&mut replayed, &mut ChangeSet::default());
        replayed.tick += 1;
    }
    assert_eq!(replayed.raids.raids[&2].groups_spawned, 2);
    assert_eq!(replayed.raids.raids[&2].members.len(), 5);
}
#[test]
fn raids_pending_omen_checkpoint_restart_keeps_center_and_deadline() {
    let root = tempfile::tempdir().unwrap();
    let (mut state, player, _) = crate::raids::tests::fixture(Some(root.path()));
    state
        .players
        .get_mut(&player)
        .unwrap()
        .give_effect(ActiveEffect::new(EffectId::BadOmen, 120000, 2));
    crate::raids::raid_omens(&mut state);
    let expected = state.raids.clone();
    checkpoint(root.path(), &mut state);
    let config = CoreConfig::new(root.path().to_owned(), 42);
    let restored = crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    assert_eq!(restored.raids, expected);
    let player = restored.offline_players.get(&Uuid::from_u128(77)).unwrap();
    assert!(player.active_effects.has(EffectId::RaidOmen));
}

#[test]
fn raids_idle_ticks_skip_wal_and_restore_exact_elapsed_counters() {
    let root = tempfile::tempdir().unwrap();
    let (mut state, player, _) = crate::raids::tests::fixture(Some(root.path()));
    state
        .players
        .get_mut(&player)
        .unwrap()
        .give_effect(ActiveEffect::new(EffectId::BadOmen, 120000, 0));
    crate::raids::raid_omens(&mut state);
    state.tick += 600;
    crate::raids::raid_omens(&mut state);
    crate::raids::run(&mut state, &mut ChangeSet::default());
    state.tick += 1;
    checkpoint(root.path(), &mut state);
    let mut writer = JournalWriter::open(root.path(), 1).unwrap();
    for _ in 0..100 {
        crate::raids::run(&mut state, &mut ChangeSet::default());
        state.tick += 1;
        let out = output(&state, ChangeSet::default());
        let entries = journal_entries_for_tick(&mut state, &out);
        assert!(
            !entries
                .iter()
                .any(|entry| matches!(entry, JournalEntry::Raids { .. })),
            "an unchanged countdown must not write a raid post-image"
        );
        writer.append(state.tick, state.tick, &entries).unwrap();
    }
    writer.flush().unwrap();
    let config = CoreConfig::new(root.path().to_owned(), 42);
    let mut restored = crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    assert_eq!(restored.tick, state.tick);
    assert_eq!(
        restored.raids, state.raids,
        "WAL world ticks rebase all native countdown counters"
    );
    activate(&mut restored);
    let remaining = restored.raids.raids[&2].cooldown_ticks;
    for _ in 0..remaining {
        crate::raids::run(&mut restored, &mut ChangeSet::default());
        restored.tick += 1;
    }
    assert_eq!(
        restored.raids.raids[&2].groups_spawned, 1,
        "restart cannot reset or duplicate a wave countdown"
    );
    assert_eq!(restored.raids.raids[&2].members.len(), 4);
    let out = output(&restored, ChangeSet::default());
    assert!(
        journal_entries_for_tick(&mut restored, &out)
            .iter()
            .any(|entry| matches!(entry, JournalEntry::Raids { .. })),
        "wave transition is durable"
    );
}

#[test]
fn persisted_unsafe_random_tick_speed_is_clamped_on_recovery() {
    let root = tempfile::tempdir().unwrap();
    let (mut state, _, _) = crate::raids::tests::fixture(Some(root.path()));
    state
        .gamerules
        .insert("random_tick_speed".into(), i32::MAX.to_string());
    state.gamerules_dirty = true;
    checkpoint(root.path(), &mut state);
    let config = CoreConfig::new(root.path().to_owned(), 42);
    let recovered = crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    assert_eq!(recovered.gamerules["random_tick_speed"], "4096");
    assert!(recovered.gamerules_dirty);
}
