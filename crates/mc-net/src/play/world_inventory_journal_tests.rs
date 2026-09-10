use super::super::tests::{registries, snapshot};
use super::super::*;
use crate::play::PlayerPose;
use crate::play::persistence::PlayerPersistedState;
use crate::play::persistence::inventory_recovery::PlayerInventoryRecovery;
use crate::script::storage::PreparedStorageBatch;

fn inventory_batch(items: &ItemRegistry) -> PreparedStorageBatch {
    let state = PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5));
    let recovery =
        PlayerInventoryRecovery::capture(uuid::Uuid::from_u128(1), &state, &state.inventory, items)
            .unwrap();
    serde_json::from_value(serde_json::json!({
        "transaction_id": 1,
        "plugin_id": "settlement",
        "mutations": [{
            "kind": "compare_and_swap",
            "key": "balance",
            "expected_version": null,
            "value": "10"
        }],
        "inventory": recovery
    }))
    .unwrap()
}

#[test]
fn unprojected_player_only_decision_survives_checkpoint_and_restart() {
    let root = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let (journal, _) =
        WorldChunkJournal::open_for_test(root.path(), blocks.clone(), items.clone()).unwrap();
    let batch = inventory_batch(&items);
    let id = journal.reserve_decision_ids(1).unwrap()[0];
    journal
        .record_reserved_inventory_decision(1, id, Vec::new(), &batch)
        .unwrap();
    let later = journal
        .record_snapshots(2, vec![snapshot(&blocks, ChunkPos { x: 1, z: 0 }, 2)])
        .unwrap();
    assert_eq!(journal.watermark(), None);
    journal.checkpoint_through(later).unwrap();
    drop(journal);

    let (journal, pending) =
        WorldChunkJournal::open_for_test(root.path(), blocks.clone(), items.clone()).unwrap();
    assert_eq!(
        pending
            .iter()
            .map(WorldChunkDecision::id)
            .collect::<Vec<_>>(),
        vec![id, later]
    );
    assert_eq!(pending[0].inventory_batch().unwrap(), Some(batch));
    assert_eq!(journal.watermark(), None);
    journal.mark_inventory_projected(id).unwrap();
    assert_eq!(journal.watermark(), Some(later));
    journal.checkpoint_through(later).unwrap();
    drop(journal);
    let (_, pending) = WorldChunkJournal::open_for_test(root.path(), blocks, items).unwrap();
    assert!(pending.is_empty());
}

#[test]
fn save_cutoff_cannot_capture_a_later_inventory_publication() {
    let root = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let (journal, _) =
        WorldChunkJournal::open_for_test(root.path(), blocks.clone(), items.clone()).unwrap();
    let earlier = journal
        .record_snapshots(1, vec![snapshot(&blocks, ChunkPos { x: 0, z: 0 }, 1)])
        .unwrap();
    let id = journal.reserve_decision_ids(1).unwrap()[0];
    journal
        .record_reserved_inventory_decision(2, id, Vec::new(), &inventory_batch(&items))
        .unwrap();
    let cutoff = journal.watermark().unwrap();
    assert_eq!(cutoff, earlier);
    journal.mark_inventory_projected(id).unwrap();
    journal.checkpoint_through(cutoff).unwrap();
    drop(journal);

    let (journal, pending) = WorldChunkJournal::open_for_test(root.path(), blocks, items).unwrap();
    assert_eq!(
        pending
            .iter()
            .map(WorldChunkDecision::id)
            .collect::<Vec<_>>(),
        vec![id]
    );
    assert!(pending[0].inventory_batch().unwrap().is_some());
    assert_eq!(journal.watermark(), None);
    journal.mark_inventory_projected(id).unwrap();
    assert_eq!(journal.watermark(), Some(id));
}

#[test]
fn incomplete_inventory_frame_recovers_no_participant() {
    let root = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let (journal, _) =
        WorldChunkJournal::open_for_test(root.path(), blocks.clone(), items.clone()).unwrap();
    journal.reserve_decision_ids(1).unwrap();
    journal
        .record_reserved_inventory_decision(1, 1, Vec::new(), &inventory_batch(&items))
        .unwrap();
    drop(journal);
    let path = root.path().join(SOLARIS_DIRECTORY).join(JOURNAL_FILE);
    let file = OpenOptions::new().write(true).open(path).unwrap();
    file.set_len(file.metadata().unwrap().len() - 1).unwrap();
    file.sync_all().unwrap();
    drop(file);

    let (journal, pending) = WorldChunkJournal::open_for_test(root.path(), blocks, items).unwrap();
    assert!(pending.is_empty());
    assert_eq!(journal.reserve_decision_ids(1).unwrap(), vec![2]);
}

#[test]
fn decoder_rejects_complete_payloads_above_image_and_inventory_limits() {
    let mut payload = Vec::new();
    payload.extend_from_slice(&1_u64.to_le_bytes());
    payload.extend_from_slice(&0_u64.to_le_bytes());
    payload.extend_from_slice(&((MAX_IMAGES_PER_DECISION + 1) as u32).to_le_bytes());
    payload.resize(
        DECISION_FIXED_BYTES + (MAX_IMAGES_PER_DECISION + 1) * IMAGE_PREFIX_BYTES,
        0,
    );
    assert!(decode_decision_payload(&payload, false).is_err());

    payload.truncate(DECISION_FIXED_BYTES);
    payload[16..20].copy_from_slice(&0_u32.to_le_bytes());
    let oversized = PreparedStorageBatch::MAX_ENCODED_BYTES + 1;
    payload.extend_from_slice(&(oversized as u32).to_le_bytes());
    payload.resize(payload.len() + oversized, 0);
    assert!(decode_decision_payload(&payload, true).is_err());
}
