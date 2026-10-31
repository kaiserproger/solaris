//! Settlement persistence regression tests (D006 S1): checkpoint rows,
//! touched-only WAL replay, deletion/high-water retention, legacy refusal,
//! and a real SIGKILL-child reopen with identical records plus crash-claim
//! villager memories rehydrated through the first allocator tick.

use super::*;
use crate::config::CoreConfig;
use crate::dimensions::Dimension;
use crate::journal::{JournalEntry, JournalWriter, Replay};
use crate::settlements::construction;
use std::sync::Arc;

fn registry() -> Arc<crate::registry::CoreRegistry> {
    Arc::new(crate::registry::CoreRegistry::load(None).unwrap())
}

fn state() -> TickState {
    let registry = registry();
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 64, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    TickState::new(registry, world, 2, 512)
}

fn town(state: &mut TickState, name: &str) -> SettlementId {
    state
        .settlements
        .create(
            &mut state.settlement_sequence,
            name.into(),
            "owner".into(),
            [0, 64, 0],
            48,
        )
        .unwrap()
}

fn resident(state: &mut TickState, pos: [f64; 3]) -> crate::boundary::EntityId {
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let id = state.spawn_mob(type_id, pos, 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).villager =
        Some(Box::new(crate::villager::VillagerState::default()));
    id
}
fn crash_block(state: &TickState, name: &str, props: &[(&str, &str)]) -> u32 {
    let id = mc_data::Identifier::parse(name).unwrap();
    let default = state.registry.blocks.block(&id).unwrap().default;
    let mut full = state
        .registry
        .blocks
        .by_id(default)
        .unwrap()
        .properties
        .clone();
    for (key, value) in props {
        if let Some(entry) = full.iter_mut().find(|(k, _)| k == key) {
            entry.1 = (*value).to_owned();
        }
    }
    state
        .registry
        .blocks
        .by_name_and_props(&id, &full)
        .unwrap()
        .0
}

#[test]
fn pending_owner_order_survives_native_checkpoint_and_rejects_corruption() {
    let mut live = state();
    let id = town(&mut live, "queued");
    let _ = construction::request_build(&mut live, id, "house");
    let rows = checkpoint_rows(&live, 9).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&rows[&row_key(id)].payload).unwrap();
    assert_eq!(value["value"]["pending_order"]["blueprint"], "house");
    let mut restored = state();
    hydrate_dimension(&mut restored, &rows, &Replay::default(), id.0).unwrap();
    assert_eq!(restored.settlements.get(id), live.settlements.get(id));
    let mut old_rows = rows.clone();
    let mut old_value = value.clone();
    old_value["value"]
        .as_object_mut()
        .unwrap()
        .remove("pending_order");
    old_rows.get_mut(&row_key(id)).unwrap().payload = serde_json::to_vec(&old_value).unwrap();
    hydrate_dimension(&mut state(), &old_rows, &Replay::default(), id.0).unwrap();
    for corrupt in [
        serde_json::json!({"blueprint": "", "level": 1, "building": null}),
        serde_json::json!({"blueprint": "unknown", "level": 1, "building": null}),
        serde_json::json!({"blueprint": "house", "level": 0, "building": null}),
        serde_json::json!({"blueprint": "house", "level": 2, "building": 42}),
        serde_json::json!({"blueprint": "house", "level": 1, "building": null, "extra": true}),
    ] {
        let mut corrupt_rows = rows.clone();
        let mut corrupt_value = value.clone();
        corrupt_value["value"]["pending_order"] = corrupt;
        corrupt_rows.get_mut(&row_key(id)).unwrap().payload =
            serde_json::to_vec(&corrupt_value).unwrap();
        assert!(hydrate_dimension(&mut state(), &corrupt_rows, &Replay::default(), id.0).is_err());
    }
}

#[test]
fn checkpoint_rows_are_per_record_with_shared_sequence() {
    let mut state = state();
    let first = town(&mut state, "first");
    let second = town(&mut state, "second");
    // A child-dimension record rides the same capture with its home dimension.
    let mut child = self::state();
    child.dimension = Dimension::Nether;
    child.settlement_sequence = state.settlement_sequence;
    let nether_id = child
        .settlements
        .create(
            &mut child.settlement_sequence,
            "nethertown".into(),
            "owner".into(),
            [8, 64, 8],
            32,
        )
        .unwrap();
    state
        .other_dimensions
        .insert(Dimension::Nether, Box::new(child));
    let rows = checkpoint_rows(&state, 9).unwrap();
    assert_eq!(rows.len(), 4);
    for (id, dimension) in [
        (first, Dimension::Overworld),
        (second, Dimension::Overworld),
        (nether_id, Dimension::Nether),
    ] {
        let row = &rows[&row_key(id)];
        assert_eq!(row.version, 1);
        assert_eq!(row.captured_tick, 9);
        assert!(row.payload.len() <= 16 * 1024 * 1024);
        let typed: Row = serde_json::from_slice(&row.payload).unwrap();
        assert_eq!(typed.dimension, dimension);
        assert_eq!(typed.value.id, id);
    }
    let sequence: u64 = serde_json::from_slice(&rows["settlement_sequence"].payload).unwrap();
    assert_eq!(sequence, second.0.max(nether_id.0));
    // Canonical keys only.
    assert!(parse_row_key(&row_key(first)).is_some());
    for key in [
        "settlement/0",
        "settlement/007",
        "settlement/",
        "settlement/abc",
        "settlement/7/extra",
        "settlements/7",
    ] {
        assert_eq!(parse_row_key(key), None, "{key}");
    }
}

#[test]
fn journal_captures_touched_only_and_replay_converges() {
    let mut state = state();
    let first = town(&mut state, "first");
    let second = town(&mut state, "second");
    let mut entries = Vec::new();
    journal_entries(&mut state, 3, &mut entries);
    assert_eq!(entries.len(), 2);
    // Drained: unchanged records emit nothing.
    let mut idle = Vec::new();
    journal_entries(&mut state, 4, &mut idle);
    assert!(idle.is_empty());
    // Every dimension drains its own registry (per-dim registries); the caller
    // wraps each dimension's images under its Dimension envelope.
    let mut child = self::state();
    child.dimension = Dimension::Nether;
    let _ = child.settlements.take_images();
    let mut child_entries = Vec::new();
    journal_entries(&mut child, 4, &mut child_entries);
    assert!(child_entries.is_empty());
    let child_id = child
        .settlements
        .create(
            &mut child.settlement_sequence,
            "childtown".into(),
            "owner".into(),
            [0, 64, 0],
            48,
        )
        .unwrap();
    let mut child_touched = Vec::new();
    journal_entries(&mut child, 5, &mut child_touched);
    assert_eq!(child_touched.len(), 1);
    assert!(matches!(
        &child_touched[0],
        JournalEntry::Settlement { image, .. } if image.id == child_id
    ));
    // Rename one record: exactly one touched image.
    state.settlements.rename(first, "renamed".into()).unwrap();
    let mut renamed = Vec::new();
    journal_entries(&mut state, 5, &mut renamed);
    assert_eq!(renamed.len(), 1);

    // Fold the batches in order: live images converge, tombstones delete.
    let mut replay = Replay::default();
    replay = crate::journal::apply_batch_images(replay, &[(3, 3, entries)]);
    assert_eq!(replay.settlements.len(), 2);
    assert_eq!(replay.settlement_high_water, second.0);
    replay = crate::journal::apply_batch_images(replay, &[(5, 5, renamed)]);
    assert_eq!(replay.settlements[&first.0].name, "renamed");
    // Deletion tombstone.
    assert!(state.settlements.remove(first));
    let mut tombstoned = Vec::new();
    journal_entries(&mut state, 6, &mut tombstoned);
    assert_eq!(tombstoned.len(), 1);
    replay = crate::journal::apply_batch_images(replay, &[(6, 6, tombstoned)]);
    assert!(!replay.settlements.contains_key(&first.0));
    assert!(replay.settlement_removed.contains(&first.0));
    assert!(replay.settlements.contains_key(&second.0));
    // High water survives deletion: never reused.
    assert_eq!(replay.settlement_high_water, second.0);
}

#[test]
fn hydrate_validates_rows_and_keeps_sequence_monotonic() {
    let mut state = state();
    let first = town(&mut state, "first");
    assert!(state.settlements.remove(first));
    let rows = checkpoint_rows(&state, 7).unwrap();
    assert!(!rows.keys().any(|key| key == &row_key(first)));

    // Hydrate into a fresh state: tombstone replay keeps it removed. Rows
    // homed to other dimensions are skipped by this dimension's hydrate.
    let mut reopened = self::state();
    let mut replay = Replay::default();
    replay.settlement_removed.insert(first.0);
    replay.settlement_high_water = first.0;
    hydrate_dimension(&mut reopened, &rows, &replay, first.0).unwrap();
    assert!(reopened.settlements.get(first).is_none());
    assert_eq!(reopened.settlement_sequence, first.0);
    // Next allocation never reuses the removed id.
    let second = reopened
        .settlements
        .create(
            &mut reopened.settlement_sequence,
            "second".into(),
            "owner".into(),
            [0, 64, 0],
            48,
        )
        .unwrap();
    assert!(second.0 > first.0);

    // Corrupt rows fail closed: identity mismatch, bad name, duplicate names.
    let mut bad = rows.clone();
    bad.insert(
        "settlement/999".into(),
        crate::native_checkpoint::record(
            7,
            serde_json::to_vec(&Row {
                dimension: Dimension::Overworld,
                value: SettlementRecord {
                    id: SettlementId(1000),
                    name: "mismatch".into(),
                    owner: Some("owner".into()),
                    owner_name: None,
                    anchor: [0, 64, 0],
                    radius: 48,
                    revision: 1,
                    household_sequence: 0,
                    households: Default::default(),
                    building_sequence: 0,
                    buildings: Default::default(),
                    district: None,
                    economy: Default::default(),
                    village_source: None,
                    bootstrap_storage: None,
                    project: None,
                    pending_order: None,
                    construction_cursor: 0,
                },
            })
            .unwrap(),
        ),
    );
    let mut probe = self::state();
    assert!(hydrate_dimension(&mut probe, &bad, &Replay::default(), 0).is_err());

    // Duplicate names within ONE dimension (distinct ids, built as corrupt
    // RecordImages the registry itself would never create) are refused at
    // finish_recovery convergence.
    let mut root = self::state();
    for id in [SettlementId(101), SettlementId(102)] {
        root.settlements.apply_image(RecordImage {
            id,
            value: Some(SettlementRecord {
                id,
                name: "same".into(),
                owner: Some("owner".into()),
                owner_name: None,
                anchor: [0, 64, 0],
                radius: 48,
                revision: 1,
                household_sequence: 0,
                households: Default::default(),
                building_sequence: 0,
                buildings: Default::default(),
                district: None,
                economy: Default::default(),
                village_source: None,
                bootstrap_storage: None,
                project: None,
                pending_order: None,
                construction_cursor: 0,
            }),
        });
    }
    root.other_dimensions
        .insert(Dimension::Nether, Box::new(self::state()));
    assert!(finish_recovery(&mut root).is_err());

    // The same name across dimensions with globally distinct ids is allowed:
    // names are unique per dimension registry, identities globally.
    let mut multi = self::state();
    multi.settlements.apply_image(RecordImage {
        id: SettlementId(201),
        value: Some(SettlementRecord {
            id: SettlementId(201),
            name: "same".into(),
            owner: Some("owner".into()),
            owner_name: None,
            anchor: [0, 64, 0],
            radius: 48,
            revision: 1,
            household_sequence: 0,
            households: Default::default(),
            building_sequence: 0,
            buildings: Default::default(),
            district: None,
            economy: Default::default(),
            village_source: None,
            bootstrap_storage: None,
            project: None,
            pending_order: None,
            construction_cursor: 0,
        }),
    });
    let mut child = self::state();
    child.dimension = Dimension::Nether;
    child.settlements.apply_image(RecordImage {
        id: SettlementId(202),
        value: Some(SettlementRecord {
            id: SettlementId(202),
            name: "same".into(),
            owner: Some("owner".into()),
            owner_name: None,
            anchor: [8, 64, 8],
            radius: 32,
            revision: 1,
            household_sequence: 0,
            households: Default::default(),
            building_sequence: 0,
            buildings: Default::default(),
            district: None,
            economy: Default::default(),
            village_source: None,
            bootstrap_storage: None,
            project: None,
            pending_order: None,
            construction_cursor: 0,
        }),
    });
    multi
        .other_dimensions
        .insert(Dimension::Nether, Box::new(child));
    assert!(finish_recovery(&mut multi).is_ok());
}

#[test]
fn derived_counts_rebuild_from_columns_not_rows() {
    let mut state = state();
    let id = town(&mut state, "town");
    let villager = resident(&mut state, [1.0, 64.0, 1.0]);
    crate::settlements::bind(&mut state, id, villager).unwrap();
    assert_eq!(crate::settlements::member_count(&state, id), 1);
    // Rows carry no roster: a reopened registry counts zero until columns hydrate.
    let rows = checkpoint_rows(&state, 11).unwrap();
    let mut reopened = self::state();
    hydrate_dimension(&mut reopened, &rows, &Replay::default(), id.0).unwrap();
    assert_eq!(crate::settlements::member_count(&reopened, id), 0);
    // Hydrate the sole membership column, then the recovery hook derives it.
    let restored = resident(&mut reopened, [1.0, 64.0, 1.0]);
    let slot = reopened.entities.slot_of(restored).unwrap();
    reopened
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .settlement = Some(id);
    finish_recovery(&mut reopened).unwrap();
    assert_eq!(crate::settlements::member_count(&reopened, id), 1);
}

#[test]
fn legacy_plugin_state_is_refused_not_wiped() {
    let mut state = state();
    town(&mut state, "town");
    assert!(refuse_legacy_plugin_state(&state).is_ok());

    // Legacy registry root key under the real owner.
    state.plugins.apply_storage(
        "solaris-settlements",
        &[
            mc_script::ScriptStorageMutation::compare_and_swap("settlement-root-v2", None, "{}")
                .unwrap(),
        ],
    );
    let error = refuse_legacy_plugin_state(&state).unwrap_err().to_string();
    assert!(error.contains("solaris-settlements"), "{error}");
    assert!(error.contains("refusing"), "{error}");
}

#[test]
fn legacy_fragment_and_trade_keys_are_refused_for_any_owner() {
    for (owner, key) in [
        ("other", "settlement-fragment-v2/abcd/0"),
        ("other", "settlement-v2/abcd/head"),
        ("other", "settlement-trade-v1/abcd"),
        ("other", "player-contract-v1/x"),
        ("other", "paid-shipment-v1/x"),
        ("other", "production-root-v2"),
        ("other", "development-v2/district/abcd"),
    ] {
        let mut state = state();
        state.plugins.apply_storage(
            owner,
            &[mc_script::ScriptStorageMutation::compare_and_swap(key, None, "{}").unwrap()],
        );
        let error = refuse_legacy_plugin_state(&state).unwrap_err().to_string();
        assert!(error.contains(key), "{error}");
    }
}

#[test]
fn legacy_settlement_owned_npc_custody_is_refused() {
    // Custody rows owned by the legacy settlement plugin refuse, identified
    // through the custody map's owner metadata (entity UUID -> owner, handle).
    let mut state = state();
    let mut custody = std::collections::BTreeMap::new();
    custody.insert(
        "11111111-1111-1111-1111-111111111111".to_owned(),
        (
            "solaris-settlements".to_owned(),
            "settlement-handle-1".to_owned(),
        ),
    );
    state.plugins.set_domain(
        "__native_npcs",
        "npc/custody",
        serde_json::to_value(&custody).unwrap(),
    );
    state.plugins.set_domain(
        "__native_npcs",
        "npc/record/settlement-handle-1",
        serde_json::json!({"handle": "settlement-handle-1"}),
    );
    let error = refuse_legacy_plugin_state(&state).unwrap_err().to_string();
    assert!(error.contains("solaris-settlements"), "{error}");
    assert!(error.contains("refusing"), "{error}");
}

#[test]
fn unrelated_npc_consumers_stay_admitted_but_legacy_births_and_courtship_refuse() {
    // Unrelated NPC custody still exists until the final WIT consumer leaves;
    // the retired births/courtship KV owner must never be silently dropped.
    let mut clean = state();
    let mut custody = std::collections::BTreeMap::new();
    custody.insert(
        "22222222-2222-2222-2222-222222222222".to_owned(),
        ("other-plugin".to_owned(), "other-handle-1".to_owned()),
    );
    clean.plugins.set_domain(
        "__native_npcs",
        "npc/custody",
        serde_json::to_value(&custody).unwrap(),
    );
    clean.plugins.set_domain(
        "__native_npcs",
        "npc/record/other-handle-1",
        serde_json::json!({"handle": "other-handle-1"}),
    );
    clean
        .plugins
        .set_domain("__native_npcs", "npc/revision", serde_json::json!(3u64));
    assert!(refuse_legacy_plugin_state(&clean).is_ok());
    for key in ["npc/births", "courtship/a/b"] {
        clean
            .plugins
            .set_domain("__native_npcs", key, serde_json::json!(1));
        assert!(refuse_legacy_plugin_state(&clean).is_err(), "{key}");
        clean.plugins.delete_domain("__native_npcs", key);
    }
}

/// Production journal shape: every dimension's touched images ride an explicit
/// Dimension envelope, Overworld included. Uses the real tick capture helper.
#[test]
fn tick_capture_wraps_all_dimension_images_in_envelopes() {
    use crate::boundary::{ChangeSet, TickOutput, WorldSnapshot};
    let mut state = state();
    let root_id = town(&mut state, "roottown");
    let mut child = self::state();
    child.dimension = Dimension::Nether;
    child.settlement_sequence = state.settlement_sequence;
    let child_id = child
        .settlements
        .create(
            &mut child.settlement_sequence,
            "childtown".into(),
            "owner".into(),
            [8, 64, 8],
            32,
        )
        .unwrap();
    state
        .other_dimensions
        .insert(Dimension::Nether, Box::new(child));
    // Minimal child publication so the shared capture can route it.
    let mut snapshot = WorldSnapshot::default();
    snapshot.dimensions.insert(
        Dimension::Nether,
        TickOutput {
            tick: state.tick,
            snapshot: Arc::new(WorldSnapshot::default()),
            changes: Arc::new(ChangeSet::default()),
            times: Default::default(),
        },
    );
    let output = TickOutput {
        tick: state.tick,
        snapshot: Arc::new(snapshot),
        changes: Arc::new(ChangeSet::default()),
        times: Default::default(),
    };
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let mut enveloped: Vec<(Dimension, SettlementId)> = Vec::new();
    let mut bare = 0u32;
    fn visit(
        entries: &[JournalEntry],
        enveloped: &mut Vec<(Dimension, SettlementId)>,
        bare: &mut u32,
    ) {
        for entry in entries {
            match entry {
                JournalEntry::Dimension { dimension, entries } => {
                    for inner in entries {
                        if let JournalEntry::Settlement { image, .. } = inner {
                            enveloped.push((*dimension, image.id));
                        }
                    }
                    // Inner entries are already accounted for within this
                    // envelope; only outer bare records are forbidden.
                }
                JournalEntry::Settlement { .. } => *bare += 1,
                _ => {}
            }
        }
    }
    visit(&entries, &mut enveloped, &mut bare);
    assert_eq!(bare, 0, "no bare settlement images outside envelopes");
    enveloped.sort();
    assert_eq!(
        enveloped,
        vec![
            (Dimension::Overworld, root_id),
            (Dimension::Nether, child_id),
        ]
    );
}

/// Two-dimension roundtrip: equal names with different ids checkpoint and
/// replay per dimension. Root-only fixtures cannot catch cross-dim leakage,
/// so this test captures rows from both registries, folds each dimension's
/// own WAL tail, hydrates per dimension, and asserts exact placement.
#[test]
fn two_dimension_equal_name_checkpoint_and_tail_roundtrip() {
    // Root and Nether each own a distinct "same" record.
    let mut root = state();
    let root_id = root
        .settlements
        .create(
            &mut root.settlement_sequence,
            "same".into(),
            "owner".into(),
            [0, 64, 0],
            48,
        )
        .unwrap();
    let mut nether = self::state();
    nether.dimension = Dimension::Nether;
    nether.settlement_sequence = root.settlement_sequence;
    let nether_id = nether
        .settlements
        .create(
            &mut nether.settlement_sequence,
            "same".into(),
            "owner".into(),
            [8, 64, 8],
            32,
        )
        .unwrap();
    assert_ne!(root_id.0, nether_id.0);
    root.other_dimensions
        .insert(Dimension::Nether, Box::new(nether));

    // Checkpoint captures both rows with home dimensions; sequence is global max.
    let rows = checkpoint_rows(&root, 21).unwrap();
    assert_eq!(rows.len(), 3);
    let root_row: Row = serde_json::from_slice(&rows[&row_key(root_id)].payload).unwrap();
    let nether_row: Row = serde_json::from_slice(&rows[&row_key(nether_id)].payload).unwrap();
    assert_eq!(root_row.dimension, Dimension::Overworld);
    assert_eq!(nether_row.dimension, Dimension::Nether);
    assert_eq!(root_row.value.name, "same");
    assert_eq!(nether_row.value.name, "same");

    // Rename the Nether record after the checkpoint: its touched image folds
    // only into the Nether tail.
    root.other_dimensions
        .get_mut(&Dimension::Nether)
        .unwrap()
        .settlements
        .rename(nether_id, "renamed".into())
        .unwrap();
    let mut nether_tail = Vec::new();
    journal_entries(
        root.other_dimensions.get_mut(&Dimension::Nether).unwrap(),
        22,
        &mut nether_tail,
    );
    assert_eq!(nether_tail.len(), 1);
    let mut nether_replay = Replay::default();
    nether_replay = crate::journal::apply_batch_images(nether_replay, &[(22, 22, nether_tail)]);
    // Root tail carries its own touched create image (capture never drains).
    let mut root_tail = Vec::new();
    journal_entries(&mut root, 22, &mut root_tail);
    let mut root_replay = Replay::default();
    root_replay = crate::journal::apply_batch_images(
        root_replay,
        &[(
            22,
            22,
            vec![JournalEntry::Dimension {
                dimension: Dimension::Overworld,
                entries: root_tail,
            }],
        )],
    );

    // Hydrate per dimension: each registry keeps exactly its own record.
    let sequence: u64 = serde_json::from_slice(&rows["settlement_sequence"].payload).unwrap();
    let mut fresh_root = self::state();
    hydrate_dimension(&mut fresh_root, &rows, &root_replay, sequence).unwrap();
    let mut fresh_nether = self::state();
    fresh_nether.dimension = Dimension::Nether;
    hydrate_dimension(&mut fresh_nether, &rows, &nether_replay, sequence).unwrap();
    let root_names: Vec<String> = fresh_root
        .settlements
        .records()
        .map(|record| record.name.clone())
        .collect();
    let nether_names: Vec<String> = fresh_nether
        .settlements
        .records()
        .map(|record| record.name.clone())
        .collect();
    assert_eq!(root_names, vec!["same".to_owned()]);
    assert_eq!(nether_names, vec!["renamed".to_owned()]);
    assert_eq!(fresh_root.settlements.get(root_id).unwrap().revision, 1);
    // Global sequence + finish_recovery accept cross-dim equal names.
    fresh_root
        .other_dimensions
        .insert(Dimension::Nether, Box::new(fresh_nether));
    assert!(finish_recovery(&mut fresh_root).is_ok());
    assert!(fresh_root.settlement_sequence >= root_id.0.max(nether_id.0));
    assert_eq!(
        fresh_root.other_dimensions[&Dimension::Nether].settlement_sequence,
        sequence.max(nether_id.0)
    );
}

#[test]
fn autonomous_row_and_claim_wal_tail_recover_with_the_same_worldgen_identity() {
    let mut state = state();
    let source = "minecraft:overworld:12:64:12";
    let id = state
        .settlements
        .adopt_village(&mut state.settlement_sequence, source, [12, 64, 12])
        .unwrap();
    let rows = checkpoint_rows(&state, 21).unwrap();
    let sequence = state.settlement_sequence;
    state.settlements.take_images();
    let mut unclaimed = self::state();
    hydrate_dimension(&mut unclaimed, &rows, &Replay::default(), sequence).unwrap();
    assert!(unclaimed.settlements.get(id).unwrap().owner.is_none());
    state
        .settlements
        .claim(id, "alice".into(), [12, 64, 12])
        .unwrap();
    let mut entries = Vec::new();
    journal_entries(&mut state, 22, &mut entries);
    let replay = crate::journal::apply_batch_images(Replay::default(), &[(22, 22, entries)]);
    let mut reopened = self::state();
    hydrate_dimension(&mut reopened, &rows, &replay, sequence).unwrap();
    finish_recovery(&mut reopened).unwrap();
    assert_eq!(
        reopened.settlements.get(id).unwrap().owner.as_deref(),
        Some("alice")
    );
    assert_eq!(
        reopened
            .settlements
            .adopt_village(&mut reopened.settlement_sequence, source, [12, 64, 12]),
        Ok(id)
    );
    assert_eq!(reopened.settlements.records().count(), 1);
    assert!(
        reopened
            .settlements
            .claim(id, "bob".into(), [12, 64, 12])
            .is_err()
    );
}

fn crash_cash(state: &TickState, uuid: uuid::Uuid) -> i32 {
    let slot = state
        .entities
        .slot_of_uuid(uuid)
        .expect("original wallet body");
    state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .unwrap()
        .inventory
        .iter()
        .flatten()
        .filter(|stack| {
            state
                .registry
                .items
                .name_of(stack.item_id)
                .is_some_and(|name| name.as_str() == "minecraft:emerald")
        })
        .map(|stack| stack.count)
        .sum()
}

/// Real SIGKILL-child reopen: the child commits settlement records plus
/// touched WAL images wrapped in the production Dimension envelope, signals
/// readiness on stdout after the WAL fsync, and dies before any graceful
/// shutdown/checkpoint; the parent reopens and expects identical records,
/// allocator high water and paid physical wallets with an employment replay fence.
/// Readiness blocks on stdout, never a filesystem poll.
#[test]
fn sigkill_child_reopen_keeps_identical_records_and_high_water() {
    use crate::settlements::{commerce, economy, inventory::Endpoint};
    use std::io::BufRead;
    if std::env::var("MC_SETTLEMENT_CRASH_CHILD").is_ok() {
        let dir = std::env::var("MC_SETTLEMENT_CRASH_DIR").expect("child dir");
        let dir = std::path::PathBuf::from(dir);
        let config = CoreConfig::new(dir.clone(), 42);
        // Initialize a real native checkpoint, without starting a second
        // background tick/WAL writer beside the crash fixture's writer.
        // Mutation and the Dimension envelope match tick journal capture.
        let _store = crate::native_checkpoint::tests::fixture_store(&config).expect("native store");
        let mut live =
            crate::core::recover_dimension_state(&config, Dimension::Overworld).expect("live");
        let boundary = live.tick + 1;
        for name in ["killtown", "secondtown"] {
            live.settlements
                .create(
                    &mut live.settlement_sequence,
                    name.into(),
                    "owner".into(),
                    [0, 64, 0],
                    48,
                )
                .expect("create");
        }
        // Crash-claim villager: durable home/job/meeting memories journaled
        // through the production entity WAL capture, so SIGKILL must retain
        // identical claims. Home/job point at live POI blocks in the resident
        // chunk; the meeting point stays unloaded (unload is not destruction).
        let want: rustc_hash::FxHashMap<crate::boundary::ChunkPos, i32> =
            rustc_hash::FxHashMap::from_iter([(crate::boundary::ChunkPos { x: 0, z: 0 }, 0)]);
        live.world.ensure_blocking(&want);
        let foot = crash_block(
            &live,
            "minecraft:red_bed",
            &[("part", "foot"), ("facing", "east")],
        );
        let head = crash_block(
            &live,
            "minecraft:red_bed",
            &[("part", "head"), ("facing", "east")],
        );
        live.world.set_block(7, 80, 8, foot);
        live.world.set_block(8, 80, 8, head);
        for x in 3..=5 {
            for z in 3..=5 {
                live.world.set_block(x, 79, z, live.registry.states.stone);
            }
        }
        let composter = crash_block(&live, "minecraft:composter", &[]);
        live.world.set_block(4, 80, 4, composter);
        live.world.fold_overlay();
        let home = crate::boundary::BlockPos { x: 8, y: 80, z: 8 };
        let job = crate::boundary::BlockPos { x: 4, y: 80, z: 4 };
        let meeting = crate::boundary::BlockPos {
            x: 1600,
            y: 64,
            z: 1600,
        };
        let villager_type = live
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
            .expect("villager type");
        let villager_id = live.spawn_mob(villager_type, [8.5, 81.0, 8.5], 20.0);
        {
            let slot = live.entities.slot_of(villager_id).expect("villager slot");
            let villager = crate::villager::VillagerState {
                home: Some(home),
                job_site: Some(job),
                meeting_point: Some(meeting),
                data: crate::villager::VillagerData {
                    profession: crate::villager::VillagerProfession::Farmer,
                    ..Default::default()
                },
                ..Default::default()
            };
            live.entities.row_mut(slot).villager = Some(Box::new(villager));
        }
        let settlement = live.settlements.find("killtown").unwrap().id;
        crate::settlements::bind(&mut live, settlement, villager_id).unwrap();
        crate::settlements::buildings::certify(
            &mut live,
            settlement,
            crate::settlements::buildings::BuildingKind::Workshop(
                crate::villager::VillagerProfession::Farmer,
            ),
            mc_script::ScriptWorldBounds::new([3, 79, 3], [5, 82, 5]).unwrap(),
        )
        .expect("bound crash worker has a certified job");
        let family = crate::settlements::households::form(
            &mut live,
            settlement,
            &[villager_id],
            crate::settlements::households::HouseholdOrigin::Adoption,
        )
        .unwrap();
        assert_eq!(family, format!("{}-1", settlement.0));
        // A genuine autonomous wage, not a policy-only JSON roundtrip. Both
        // original native bodies and the inventory ledger enter the same WAL.
        let treasury_id = live.spawn_mob(villager_type, [6.5, 81.0, 8.5], 20.0);
        let treasury_slot = live.entities.slot_of(treasury_id).unwrap();
        let emerald = live
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:emerald").unwrap())
            .unwrap();
        let mut treasury = crate::villager::VillagerState::default();
        treasury.inventory[0] = Some(mc_data::ItemStack::new(emerald, 12));
        live.entities.row_mut(treasury_slot).villager = Some(Box::new(treasury));
        crate::settlements::bind(&mut live, settlement, treasury_id).unwrap();
        let treasury_uuid = live.entities.row(treasury_slot).uuid;
        let worker_uuid = live
            .entities
            .row(live.entities.slot_of(villager_id).unwrap())
            .uuid;
        economy::execute(
            &mut live,
            settlement,
            economy::Request::Commerce(commerce::Request::Employ {
                contract: "1".into(),
                worker: worker_uuid,
                treasury: Endpoint::VillagerCarry {
                    uuid: treasury_uuid,
                },
                wage: 3,
                period: 1,
                budget: 3,
            }),
        )
        .unwrap();
        live.tick = boundary;
        live.tick_budget.reset(boundary);
        economy::tick(&mut live);
        assert_eq!(crash_cash(&live, worker_uuid), 3);
        assert_eq!(crash_cash(&live, treasury_uuid), 9);
        for request in [
            commerce::Request::StopEmployment {
                contract: "1".into(),
            },
            commerce::Request::DismissEmployment {
                contract: "1".into(),
            },
        ] {
            economy::execute(&mut live, settlement, economy::Request::Commerce(request)).unwrap();
        }
        assert_eq!(
            live.settlements
                .get(settlement)
                .unwrap()
                .economy
                .commerce
                .ended_employment,
            1
        );
        let output = crate::boundary::TickOutput {
            tick: boundary,
            snapshot: std::sync::Arc::new(crate::boundary::WorldSnapshot::default()),
            changes: std::sync::Arc::new(crate::boundary::ChangeSet {
                entities_spawned: vec![villager_id, treasury_id],
                ..Default::default()
            }),
            times: Default::default(),
        };
        let mut entries = crate::core::journal_entries_for_tick(&mut live, &output);
        assert!(entries.iter().any(|entry| matches!(
            entry, JournalEntry::Dimension { dimension: Dimension::Overworld, entries }
                if entries.iter().filter(|entry| matches!(entry, JournalEntry::Settlement { .. })).count() == 2
        )), "production capture must retain the two touched settlement images");
        assert_eq!(
            entries
                .iter()
                .filter(|entry| matches!(
                    entry,
                    JournalEntry::Entity {
                        villager: Some(_),
                        ..
                    }
                ))
                .count(),
            2,
            "both physical native wallets enter the production WAL"
        );
        for (pos, state) in [
            ([7, 80, 8], foot),
            ([8, 80, 8], head),
            ([4, 80, 4], composter),
        ] {
            entries.push(JournalEntry::Block { pos, state });
        }
        for x in 3..=5 {
            for z in 3..=5 {
                entries.push(JournalEntry::Block {
                    pos: [x, 79, z],
                    state: live.registry.states.stone,
                });
            }
        }
        let mut writer = JournalWriter::open(&dir, 1).expect("writer");
        writer
            .append(boundary, boundary, &entries)
            .expect("durable");
        writer.flush().expect("fsync");
        assert_eq!(
            _store.read_checkpoint().unwrap().tick,
            0,
            "payment is WAL-only"
        );
        drop(writer);
        drop(live);
        // Signal only AFTER fsync, retaining the native store until SIGKILL.
        use std::io::Write;
        println!("READY {boundary} {worker_uuid} {treasury_uuid}");
        std::io::stdout().flush().expect("readiness flush");
        loop {
            std::thread::park();
        }
    }
    let dir = std::env::temp_dir().join(format!(
        "mc-settlement-kill-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let child_bin = std::env::current_exe().expect("test binary");
    let child = std::process::Command::new(child_bin)
        .arg("sigkill_child_reopen_keeps_identical_records_and_high_water")
        .env("MC_SETTLEMENT_CRASH_CHILD", "1")
        .arg("--nocapture")
        .env("MC_SETTLEMENT_CRASH_DIR", &dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn crash child");
    // Event-driven readiness: block on the child's stdout marker (fails
    // clearly on EOF/error). The child is always killed and reaped below,
    // including on assertion failure.
    struct CrashChild(std::process::Child);
    impl Drop for CrashChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = CrashChild(child);
    let stdout = child.0.stdout.take().expect("child stdout");
    let mut reader = std::io::BufReader::new(stdout);
    let (boundary_tick, crash_worker, crash_treasury) = loop {
        let mut line = String::new();
        assert_ne!(
            reader.read_line(&mut line).expect("child readiness"),
            0,
            "crash child exited without signalling durable records"
        );
        if let Some(tick) = line.trim().strip_prefix("READY ") {
            let mut fields = tick.split_whitespace();
            let boundary = fields
                .next()
                .unwrap()
                .parse::<u64>()
                .expect("boundary tick");
            let worker = fields
                .next()
                .unwrap()
                .parse::<uuid::Uuid>()
                .expect("worker uuid");
            let treasury = fields
                .next()
                .unwrap()
                .parse::<uuid::Uuid>()
                .expect("treasury uuid");
            assert!(fields.next().is_none(), "exact crash readiness fields");
            break (boundary, worker, treasury);
        }
    };
    // KILL after the observed durability point (SIGKILL on unix; real process
    // death, never a graceful shutdown or checkpoint), then reap before the
    // reopen so a failed assertion below can never leak the child.
    child.0.kill().expect("SIGKILL crash child");
    let status = child.0.wait().expect("reap crash child");
    assert!(!status.success(), "child died by signal, exit {status:?}");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            status.signal(),
            Some(9),
            "the durability probe really used SIGKILL"
        );
    }
    // Reopen: identical complete records (ids, owners, anchors, radii,
    // revisions) plus allocator high water, no reuse.
    let config = CoreConfig::new(dir.clone(), 42);
    let mut reopened =
        crate::core::recover_dimension_state(&config, Dimension::Overworld).expect("reopen");
    let mut records: Vec<SettlementRecord> = reopened.settlements.records().cloned().collect();
    records.sort_by_key(|record| record.id);
    assert_eq!(records.len(), 2);
    for record in &records {
        assert_eq!(record.owner.as_deref(), Some("owner"));
        assert_eq!(record.anchor, [0, 64, 0]);
        assert_eq!(record.radius, 48);
        assert_eq!(
            record.revision,
            if record.name == "killtown" { 7 } else { 1 }
        );
    }
    assert_eq!(
        records[0].buildings.len(),
        1,
        "certificate survives SIGKILL WAL recovery"
    );
    assert_eq!(
        crate::settlements::buildings::job_profession(
            &reopened.settlements,
            Some(records[0].id),
            crate::boundary::BlockPos { x: 4, y: 80, z: 4 },
        ),
        Some(crate::villager::VillagerProfession::Farmer),
    );
    assert_eq!(records[0].name, "killtown");
    assert_eq!(records[1].name, "secondtown");
    assert!(records[1].id.0 > records[0].id.0);
    let water = reopened.settlement_sequence;
    assert_eq!(reopened.settlements.high_water(), water);
    assert!(
        water >= records[1].id.0,
        "high water covers every committed id"
    );
    assert_eq!(crash_cash(&reopened, crash_worker), 3);
    assert_eq!(crash_cash(&reopened, crash_treasury), 9);
    let paid = &records[0].economy.commerce;
    assert_eq!(paid.ended_employment, 1);
    assert!(paid.employments.is_empty());
    // Fresh recovery runs the same native tick and receives a replay of the
    // retired contract. Neither can create a second physical debit/payment.
    economy::tick(&mut reopened);
    assert_eq!(
        economy::execute(
            &mut reopened,
            records[0].id,
            economy::Request::Commerce(commerce::Request::Employ {
                contract: "1".into(),
                worker: crash_worker,
                treasury: Endpoint::VillagerCarry {
                    uuid: crash_treasury
                },
                wage: 3,
                period: 1,
                budget: 3,
            }),
        ),
        Err(mc_script::ScriptOperationFailure::OperationConflict),
    );
    assert_eq!(crash_cash(&reopened, crash_worker), 3);
    assert_eq!(crash_cash(&reopened, crash_treasury), 9);
    // Crash-claim: kill -9 retained identical home/job/meeting memories
    // through the production entity WAL envelope.
    let home = crate::boundary::BlockPos { x: 8, y: 80, z: 8 };
    let job = crate::boundary::BlockPos { x: 4, y: 80, z: 4 };
    let meeting = crate::boundary::BlockPos {
        x: 1600,
        y: 64,
        z: 1600,
    };
    let claim_slot = reopened
        .entities
        .slots()
        .find(|slot| {
            reopened
                .entities
                .row(*slot)
                .villager
                .as_ref()
                .is_some_and(|v| v.home.is_some())
        })
        .expect("crash-claim villager reopened");
    let villager_id = reopened.entities.body(claim_slot).id;
    assert_eq!(reopened.entities.row(claim_slot).uuid, crash_worker);
    {
        let v = reopened.entities.row(claim_slot).villager.as_ref().unwrap();
        assert_eq!(v.home, Some(home));
        assert_eq!(v.job_site, Some(job));
        assert_eq!(v.meeting_point, Some(meeting));
        let settlement = records[0].id;
        let family = format!("{}-1", settlement.0);
        assert_eq!(v.settlement, Some(settlement));
        assert_eq!(v.household.as_deref(), Some(family.as_str()));
        assert!(records[0].households.contains_key(&family));
        assert_eq!(reopened.settlements.household_count(settlement, &family), 1);
        assert_eq!(
            v.data.profession,
            crate::villager::VillagerProfession::Farmer
        );
        assert!(!v.wandering);
        assert_eq!(v.conversion_time, None);
    }
    // Runtime claim indexes are never persisted: the fresh cache is empty
    // until the first allocator tick rehydrates it from live rows.
    assert!(reopened.villager_pois.homes.is_empty());
    assert!(reopened.villager_pois.jobs.is_empty());
    // First validation needs the resident active; the claim chunk is already
    // resident from the replayed block images.
    reopened.tickets.insert(
        "crash-claim".into(),
        (crate::boundary::ChunkPos { x: 0, z: 0 }, 0),
    );
    reopened.refresh_active_chunks(reopened.tick);
    let mut oracle_cache = crate::settlements::allocator::PoiCache::default();
    oracle_cache.refresh(&reopened);
    let mut oracle = reopened.entities.row(claim_slot).villager.clone().unwrap();
    crate::settlements::allocator::validate_pois(
        &reopened,
        &oracle_cache,
        villager_id,
        &mut oracle,
    );
    let tick = reopened.tick;
    reopened.tick_budget.reset(tick);
    let mut changes = crate::boundary::ChangeSet::default();
    crate::settlements::tick(&mut reopened, &mut changes);
    {
        let slot = reopened.entities.slot_of(villager_id).expect("claim slot");
        let v = reopened.entities.row(slot).villager.as_ref().unwrap();
        assert_eq!(
            v, &oracle,
            "event-driven hydration matches full validation after SIGKILL"
        );
        assert_eq!(v.home, Some(home), "loaded home survives first validation");
        assert_eq!(
            v.job_site,
            Some(job),
            "loaded job survives first validation"
        );
        assert_eq!(
            v.meeting_point,
            Some(meeting),
            "unloaded claim is not destruction"
        );
    }
    assert!(
        reopened
            .villager_pois
            .homes
            .get(&home)
            .is_some_and(|owners| owners.contains(&villager_id)),
        "home claim index rehydrated"
    );
    assert!(
        reopened
            .villager_pois
            .jobs
            .get(&job)
            .is_some_and(|owners| owners.contains(&villager_id)),
        "job claim index rehydrated"
    );
    assert!(
        !reopened.world.has_chunk(100, 100),
        "claim validation never loads chunks"
    );
    // The next allocation never reuses a committed id, even after deletion.
    let mut later = reopened;
    assert!(later.settlements.remove(records[0].id));
    let next = later
        .settlements
        .create(
            &mut later.settlement_sequence,
            "latertown".into(),
            "owner".into(),
            [0, 64, 0],
            48,
        )
        .unwrap();
    assert!(
        next.0 > records[1].id.0,
        "ids never reused after SIGKILL reopen"
    );
    // The boundary tick the child signalled is durable in the reopened view.
    assert!(later.tick >= boundary_tick);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn district_plan_uses_touched_native_wal_and_checkpoint_not_saved_readiness() {
    let mut state = state();
    let id = town(&mut state, "district");
    let other = town(&mut state, "untouched");
    let checkpoint = checkpoint_rows(&state, 1).unwrap();
    state.settlements.take_images();
    // A retained plan may outlive its building; only live observation grants
    // qualification. The persistence image stores policy, not invented facts.
    let plan = crate::settlements::districts::DistrictPlan {
        owner: "owner".into(),
        branch: "town".into(),
        level: 2,
        buildings: vec![mc_script::ScriptSettlementBuildingRef::new(
            "worksite".into(),
            "retained-certificate".into(),
            "solaris:market".into(),
        )],
    };
    let record = state.settlements.get_mut(id).unwrap();
    record.district = Some(plan.clone());
    record.revision += 1;
    let mut entries = Vec::new();
    journal_entries(&mut state, 2, &mut entries);
    assert_eq!(entries.len(), 1);
    assert!(matches!(&entries[0], JournalEntry::Settlement { image, .. }
        if image.id == id && image.value.as_ref().unwrap().district == Some(plan.clone())));
    let replay = crate::journal::apply_batch_images(Replay::default(), &[(2, 2, entries)]);
    let mut restored = self::state();
    hydrate_dimension(&mut restored, &checkpoint, &replay, 1).unwrap();
    finish_recovery(&mut restored).unwrap();
    assert_eq!(
        restored.settlements.get(id).unwrap().district,
        Some(plan.clone())
    );
    assert!(restored.settlements.get(other).unwrap().district.is_none());
    let rows = checkpoint_rows(&restored, 3).unwrap();
    let mut from_checkpoint = self::state();
    hydrate_dimension(&mut from_checkpoint, &rows, &Replay::default(), 3).unwrap();
    finish_recovery(&mut from_checkpoint).unwrap();
    let record = from_checkpoint.settlements.get(id).unwrap().clone();
    assert_eq!(record.district, Some(plan));
    let observed = crate::settlements::districts::observe(
        &mut from_checkpoint,
        record.id,
        record.anchor,
        record.radius,
        record.district.as_ref(),
    )
    .unwrap();
    assert_eq!(observed.stage, None);
    assert!(observed.reason.unwrap().contains("building unavailable"));
    let mut idle = Vec::new();
    journal_entries(&mut from_checkpoint, 4, &mut idle);
    assert!(
        idle.is_empty(),
        "query observation must not journal readiness"
    );
}
