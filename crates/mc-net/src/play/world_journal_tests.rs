use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::sync::Arc;

use mc_data::Identifier;
use mc_data::blocks::solaris_required_blocks_report;
use mc_data::items::{ItemRegistry, solaris_required_items};
use mc_world::{
    BlockPos, BlockRegistry, Chunk, ChunkPos, ChunkSnapshot, ScheduledBlockTick, SectionLight,
};

use super::*;

pub(super) fn registries() -> (Arc<BlockRegistry>, Arc<ItemRegistry>) {
    (
        Arc::new(
            BlockRegistry::from_report(&solaris_required_blocks_report())
                .expect("embedded block registry"),
        ),
        Arc::new(solaris_required_items()),
    )
}

pub(super) fn snapshot(
    blocks: &BlockRegistry,
    position: ChunkPos,
    current_tick: u64,
) -> ChunkSnapshot {
    let air = blocks
        .block(&Identifier::parse("minecraft:air").unwrap())
        .expect("air")
        .default;
    let stone = blocks
        .block(&Identifier::parse("minecraft:stone").unwrap())
        .expect("stone")
        .default;
    let mut chunk = Chunk::empty(
        position,
        air,
        Identifier::parse("minecraft:plains").unwrap(),
    );
    chunk
        .set_block(1, 64, 2, stone)
        .expect("test position is in the chunk");
    chunk.section_lights[8] = SectionLight {
        block: Some(mc_world::LightSection::uniform(0x21)),
        sky: Some(mc_world::LightSection::uniform(0x54)),
    };
    chunk
        .extras
        .push(("SolarisJournalTest".to_owned(), mc_nbt::Tag::Long(91)));
    assert!(chunk.schedule_block_tick(ScheduledBlockTick::new(
        BlockPos {
            x: position.x * 16 + 1,
            y: 64,
            z: position.z * 16 + 2,
        },
        Identifier::parse("minecraft:stone").unwrap(),
        current_tick + 17,
        2,
    )));
    Arc::new(chunk)
}

#[test]
fn round_trips_full_chunk_snapshot_and_restart_relative_tick() {
    let temp = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let position = ChunkPos { x: -3, z: 9 };
    let (journal, pending) =
        WorldChunkJournal::open_for_test(temp.path(), Arc::clone(&blocks), Arc::clone(&items))
            .unwrap();
    assert!(pending.is_empty());

    let id = journal
        .record_snapshots(120, vec![snapshot(&blocks, position, 120)])
        .unwrap();
    assert_eq!(id, 1);
    assert_eq!(journal.watermark(), Some(1));
    drop(journal);

    let (journal, pending) =
        WorldChunkJournal::open_for_test(temp.path(), Arc::clone(&blocks), Arc::clone(&items))
            .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id(), 1);
    assert_eq!(pending[0].current_tick(), 120);
    assert_eq!(pending[0].images().len(), 1);
    assert_eq!(pending[0].images()[0].position(), position);
    assert!(!pending[0].images()[0].nbt().is_empty());

    let chunks = journal.decode_pending(&pending).unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].pos, position);
    assert_eq!(chunks[0].get_block(1, 64, 2).unwrap().0, 1);
    assert_eq!(
        chunks[0].section_lights[8].block,
        Some(mc_world::LightSection::uniform(0x21))
    );
    assert_eq!(chunks[0].scheduled_block_ticks()[0].trigger_tick, 17);
    assert!(
        chunks[0].extras.iter().any(|(name, value)| {
            name == "SolarisJournalTest" && value == &mc_nbt::Tag::Long(91)
        })
    );
}

#[test]
fn shared_journal_accepts_in_ram_and_flushes_both_streams_across_batches() {
    use crate::play::persistence::FileRegionalDecisionJournal;
    use mc_entity::{RegionPhase, RegionalCommitDecision, RegionalDecisionJournal};
    let temp = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let writer = JournalWriter::open(temp.path()).unwrap();
    let (world, _) =
        WorldChunkJournal::open(temp.path(), blocks.clone(), items.clone(), writer.clone())
            .unwrap();
    let (mut entities, _) = FileRegionalDecisionJournal::open(temp.path(), writer.clone()).unwrap();
    let (paused, resume_writer) = std::sync::mpsc::sync_channel(0);
    writer
        .requests
        .send(WriterRequest::Flush { reply: paused })
        .unwrap();
    let first =
        RegionalCommitDecision::from_parts(RegionPhase(1), 1, Vec::new(), Vec::new()).unwrap();
    entities.record_commit(&first).unwrap();
    world.reserve_decision_ids(1).unwrap();
    world
        .record_reserved_snapshot_groups(1, vec![(1, Vec::new())])
        .unwrap();
    assert_eq!(world.watermark(), Some(1));
    assert_eq!(entities.pending_commit_identities(), vec![first.identity()]);
    assert!(
        !temp
            .path()
            .join(SOLARIS_DIRECTORY)
            .join(JOURNAL_FILE)
            .exists()
    );
    assert!(
        !temp
            .path()
            .join(SOLARIS_DIRECTORY)
            .join(crate::play::persistence::REGIONAL_DECISION_JOURNAL_FILE)
            .exists()
    );
    resume_writer.recv().unwrap();
    let mut expected = vec![first];
    for id in 2..=96 {
        let decision =
            RegionalCommitDecision::from_parts(RegionPhase(id), id, Vec::new(), Vec::new())
                .unwrap();
        entities.record_commit(&decision).unwrap();
        expected.push(decision);
        world.reserve_decision_ids(1).unwrap();
        world
            .record_reserved_snapshot_groups(id, vec![(id, Vec::new())])
            .unwrap();
    }
    writer.flush().unwrap();
    drop(world);
    assert!(
        WorldChunkJournal::open(
            temp.path(),
            blocks.clone(),
            items.clone(),
            JournalWriter::open(temp.path()).unwrap()
        )
        .is_err()
    );
    drop(entities);
    drop(writer);
    let writer = JournalWriter::open(temp.path()).unwrap();
    let (_, decisions) =
        WorldChunkJournal::open(temp.path(), blocks, items, writer.clone()).unwrap();
    let (_, recovered) = FileRegionalDecisionJournal::open(temp.path(), writer).unwrap();
    assert_eq!(
        decisions
            .iter()
            .map(WorldChunkDecision::id)
            .collect::<Vec<_>>(),
        (1..=96).collect::<Vec<_>>()
    );
    assert_eq!(recovered, expected);
}

#[test]
fn appends_decision_prefixes_and_keeps_each_snapshot_group_together() {
    let temp = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let (journal, _) =
        WorldChunkJournal::open_for_test(temp.path(), Arc::clone(&blocks), Arc::clone(&items))
            .unwrap();

    assert_eq!(
        journal
            .record_snapshots(10, vec![snapshot(&blocks, ChunkPos { x: 0, z: 0 }, 10)])
            .unwrap(),
        1
    );
    assert_eq!(
        journal
            .record_snapshots(
                20,
                vec![
                    snapshot(&blocks, ChunkPos { x: 1, z: 0 }, 20),
                    snapshot(&blocks, ChunkPos { x: 2, z: 0 }, 20),
                ],
            )
            .unwrap(),
        2
    );
    drop(journal);

    let (_, pending) = WorldChunkJournal::open_for_test(temp.path(), blocks, items).unwrap();
    assert_eq!(
        pending
            .iter()
            .map(WorldChunkDecision::id)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(pending[0].images().len(), 1);
    assert_eq!(pending[1].images().len(), 2);
}

#[test]
fn truncates_an_incomplete_final_frame_and_preserves_the_prefix() {
    let temp = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let path = temp.path().join("solaris/world-chunk-journal.bin");
    let (journal, _) =
        WorldChunkJournal::open_for_test(temp.path(), Arc::clone(&blocks), Arc::clone(&items))
            .unwrap();
    journal
        .record_snapshots(10, vec![snapshot(&blocks, ChunkPos { x: 0, z: 0 }, 10)])
        .unwrap();
    drop(journal);
    let prefix_len = std::fs::metadata(&path).unwrap().len();

    let (journal, _) =
        WorldChunkJournal::open_for_test(temp.path(), Arc::clone(&blocks), Arc::clone(&items))
            .unwrap();
    journal
        .record_snapshots(20, vec![snapshot(&blocks, ChunkPos { x: 1, z: 0 }, 20)])
        .unwrap();
    drop(journal);
    let damaged_len = std::fs::metadata(&path).unwrap().len() - 3;
    let file = OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len(damaged_len).unwrap();
    file.sync_all().unwrap();

    let (_, pending) = WorldChunkJournal::open_for_test(temp.path(), blocks, items).unwrap();
    assert_eq!(
        pending
            .iter()
            .map(WorldChunkDecision::id)
            .collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(std::fs::metadata(path).unwrap().len(), prefix_len);
}

#[test]
fn rejects_a_corrupt_final_frame_without_truncating_it() {
    let temp = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let path = temp.path().join("solaris/world-chunk-journal.bin");
    let (journal, _) =
        WorldChunkJournal::open_for_test(temp.path(), Arc::clone(&blocks), Arc::clone(&items))
            .unwrap();
    journal
        .record_snapshots(10, vec![snapshot(&blocks, ChunkPos { x: 0, z: 0 }, 10)])
        .unwrap();
    drop(journal);
    let (journal, _) =
        WorldChunkJournal::open_for_test(temp.path(), Arc::clone(&blocks), Arc::clone(&items))
            .unwrap();
    journal
        .record_snapshots(20, vec![snapshot(&blocks, ChunkPos { x: 1, z: 0 }, 20)])
        .unwrap();
    drop(journal);

    let damaged_len = std::fs::metadata(&path).unwrap().len();
    flip_byte(&path, std::fs::metadata(&path).unwrap().len() - 1);
    let error = WorldChunkJournal::open_for_test(temp.path(), blocks, items).unwrap_err();
    assert!(matches!(error, WorldChunkJournalError::Corrupt { .. }));
    assert_eq!(std::fs::metadata(path).unwrap().len(), damaged_len);
}

#[test]
fn rejects_corruption_before_a_valid_later_frame() {
    let temp = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let path = temp.path().join("solaris/world-chunk-journal.bin");
    let (journal, _) =
        WorldChunkJournal::open_for_test(temp.path(), Arc::clone(&blocks), Arc::clone(&items))
            .unwrap();
    journal
        .record_snapshots(10, vec![snapshot(&blocks, ChunkPos { x: 0, z: 0 }, 10)])
        .unwrap();
    journal
        .record_snapshots(20, vec![snapshot(&blocks, ChunkPos { x: 1, z: 0 }, 20)])
        .unwrap();
    drop(journal);

    let first_payload_byte = (JOURNAL_HEADER_BYTES + FRAME_PREFIX_BYTES) as u64;
    flip_byte(&path, first_payload_byte);
    let error = WorldChunkJournal::open_for_test(temp.path(), blocks, items).unwrap_err();
    assert!(matches!(error, WorldChunkJournalError::Corrupt { .. }));
}

#[test]
fn encoder_rejects_too_many_images_before_building_payload() {
    let image = WorldChunkImage {
        position: ChunkPos { x: 0, z: 0 },
        nbt: Vec::new(),
    };
    let decision = WorldChunkDecision {
        id: 1,
        current_tick: 0,
        images: vec![image; MAX_IMAGES_PER_DECISION + 1],
        inventory: None,
    };

    assert!(matches!(
        encode_decision_payload(&decision),
        Err(WorldChunkJournalError::TooManyImages(count))
            if count == MAX_IMAGES_PER_DECISION + 1
    ));
}

#[test]
fn aggregate_preflight_rejects_file_budget_without_allocation() {
    let maximum = usize::try_from(MAX_JOURNAL_FILE_BYTES).unwrap();
    assert_eq!(checked_journal_len(0, maximum).unwrap(), maximum);
    assert!(matches!(
        checked_journal_len(maximum, 1),
        Err(WorldChunkJournalError::JournalTooLarge(bytes))
            if bytes == MAX_JOURNAL_FILE_BYTES + 1
    ));
}

#[test]
fn open_rejects_oversized_sparse_journal_before_reading() {
    let temp = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let directory = temp.path().join(SOLARIS_DIRECTORY);
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(JOURNAL_FILE);
    let file = File::create(&path).unwrap();
    file.set_len(MAX_JOURNAL_FILE_BYTES + 1).unwrap();

    let error = WorldChunkJournal::open_for_test(temp.path(), blocks, items).unwrap_err();
    assert!(matches!(
        error,
        WorldChunkJournalError::JournalTooLarge(bytes)
            if bytes == MAX_JOURNAL_FILE_BYTES + 1
    ));
}

#[test]
fn append_rejects_growth_beyond_file_budget_without_writing() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join(SOLARIS_DIRECTORY);
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(JOURNAL_FILE);
    let file = File::create(&path).unwrap();
    file.set_len(MAX_JOURNAL_FILE_BYTES).unwrap();

    assert!(matches!(
        append_frames(&path, b"x"),
        Err(WriterFailure::JournalTooLarge(bytes))
            if bytes == MAX_JOURNAL_FILE_BYTES + 1
    ));
    assert_eq!(
        std::fs::metadata(path).unwrap().len(),
        MAX_JOURNAL_FILE_BYTES
    );
}

#[test]
fn writer_lease_rejects_a_second_journal_instance() {
    let temp = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let (first, _) =
        WorldChunkJournal::open_for_test(temp.path(), Arc::clone(&blocks), Arc::clone(&items))
            .unwrap();

    let error = WorldChunkJournal::open_for_test(temp.path(), blocks, items).unwrap_err();
    assert!(matches!(
        error,
        WorldChunkJournalError::Io {
            operation: "acquire journal lease",
            ..
        }
    ));
    drop(first);
}

#[tokio::test]
async fn append_budget_failure_poisons_and_wakes_later_reserved_waiter() {
    let temp = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let writer = JournalWriter::open(temp.path()).unwrap();
    let (reporter, mut failure) = tokio::sync::watch::channel(false);
    *writer.failure_reporter.lock().unwrap() = Some(reporter);
    let (journal, _) = WorldChunkJournal::open(temp.path(), blocks, items, writer.clone()).unwrap();
    journal.reserve_decision_ids(3).unwrap();
    writer.flush().unwrap();
    OpenOptions::new()
        .write(true)
        .open(temp.path().join(SOLARIS_DIRECTORY).join(JOURNAL_FILE))
        .unwrap()
        .set_len(MAX_JOURNAL_FILE_BYTES)
        .unwrap();
    let waiter = tokio::spawn({
        let journal = journal.clone();
        async move { journal.wait_for_append_turn(3).await }
    });
    journal
        .record_reserved_snapshot_groups(10, vec![(1, Vec::new())])
        .unwrap();
    failure.changed().await.unwrap();
    assert!(*failure.borrow());
    assert!(writer.flush().is_err());
    assert!(matches!(
        waiter.await.unwrap(),
        Err(WorldChunkJournalError::PoisonedOutcomeUnknown)
    ));
}

#[test]
fn checkpoint_through_watermark_atomically_retains_newer_decisions() {
    let temp = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let path = temp.path().join("solaris/world-chunk-journal.bin");
    let (journal, _) =
        WorldChunkJournal::open_for_test(temp.path(), Arc::clone(&blocks), Arc::clone(&items))
            .unwrap();
    for id in 1..=3 {
        assert_eq!(
            journal
                .record_snapshots(
                    id * 10,
                    vec![snapshot(&blocks, ChunkPos { x: id as i32, z: 0 }, id * 10)],
                )
                .unwrap(),
            id
        );
    }

    journal.checkpoint_through(2).unwrap();
    assert_eq!(journal.watermark(), Some(3));
    drop(journal);
    let (journal, pending) =
        WorldChunkJournal::open_for_test(temp.path(), Arc::clone(&blocks), Arc::clone(&items))
            .unwrap();
    assert_eq!(
        pending
            .iter()
            .map(WorldChunkDecision::id)
            .collect::<Vec<_>>(),
        vec![3]
    );

    journal.checkpoint_through(3).unwrap();
    assert_eq!(journal.watermark(), None);
    assert!(path.exists());
    drop(journal);

    let (journal, pending) =
        WorldChunkJournal::open_for_test(temp.path(), Arc::clone(&blocks), Arc::clone(&items))
            .unwrap();
    assert!(pending.is_empty());
    assert_eq!(
        journal
            .record_snapshots(40, vec![snapshot(&blocks, ChunkPos { x: 4, z: 0 }, 40)])
            .unwrap(),
        4
    );
}

#[test]
fn checkpoint_base_is_the_last_removed_decision_not_the_requested_upper_bound() {
    let temp = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let (journal, _) =
        WorldChunkJournal::open_for_test(temp.path(), Arc::clone(&blocks), Arc::clone(&items))
            .unwrap();
    assert_eq!(
        journal
            .record_snapshots(10, vec![snapshot(&blocks, ChunkPos { x: 0, z: 0 }, 10)])
            .unwrap(),
        1
    );

    journal.checkpoint_through(100).unwrap();
    assert_eq!(
        journal
            .record_snapshots(20, vec![snapshot(&blocks, ChunkPos { x: 1, z: 0 }, 20)])
            .unwrap(),
        2
    );
    drop(journal);

    let (_, pending) = WorldChunkJournal::open_for_test(temp.path(), blocks, items).unwrap();
    assert_eq!(
        pending
            .iter()
            .map(WorldChunkDecision::id)
            .collect::<Vec<_>>(),
        vec![2]
    );
}

#[test]
fn reserved_ids_are_not_reused_after_restart_without_an_append() {
    let temp = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let (journal, _) =
        WorldChunkJournal::open_for_test(temp.path(), Arc::clone(&blocks), Arc::clone(&items))
            .unwrap();
    assert_eq!(journal.reserve_decision_ids(2).unwrap(), vec![1, 2]);
    drop(journal);

    let (journal, pending) =
        WorldChunkJournal::open_for_test(temp.path(), Arc::clone(&blocks), items).unwrap();
    assert!(pending.is_empty());
    assert_eq!(journal.reserve_decision_ids(1).unwrap(), vec![3]);
}

#[test]
fn reserved_decisions_can_append_in_ordered_prefixes() {
    let temp = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let (journal, pending) = WorldChunkJournal::open_for_test(temp.path(), blocks, items).unwrap();
    assert!(pending.is_empty());
    assert_eq!(journal.reserve_decision_ids(2).unwrap(), vec![1, 2]);

    journal
        .record_reserved_snapshot_groups(10, vec![(1, Vec::new())])
        .unwrap();
    assert_eq!(journal.watermark(), Some(1));
    journal
        .record_reserved_snapshot_groups(10, vec![(2, Vec::new())])
        .unwrap();
    assert_eq!(journal.watermark(), Some(2));
}

#[test]
fn known_reserved_append_failure_can_close_with_an_empty_decision() {
    let temp = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let (journal, pending) =
        WorldChunkJournal::open_for_test(temp.path(), blocks.clone(), items.clone()).unwrap();
    assert!(pending.is_empty());
    let decision_id = journal.reserve_decision_ids(1).unwrap()[0];
    let error = journal
        .record_reserved_snapshot_groups(
            20,
            vec![(
                decision_id,
                vec![snapshot(&blocks, ChunkPos { x: 0, z: 0 }, 20)],
            )],
        )
        .expect_err("unstamped snapshot is a known pre-append failure");
    assert!(!error.outcome_unknown());

    journal
        .record_reserved_snapshot_groups(20, vec![(decision_id, Vec::new())])
        .unwrap();
    drop(journal);

    let (_reopened, pending) =
        WorldChunkJournal::open_for_test(temp.path(), blocks, items).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id(), decision_id);
    assert!(pending[0].images().is_empty());
}

#[tokio::test]
async fn later_reserved_decision_waits_for_append_turn() {
    let temp = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let (journal, pending) = WorldChunkJournal::open_for_test(temp.path(), blocks, items).unwrap();
    assert!(pending.is_empty());
    assert_eq!(journal.reserve_decision_ids(2).unwrap(), vec![1, 2]);

    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let waiter = tokio::spawn({
        let journal = journal.clone();
        async move {
            started_tx.send(()).unwrap();
            journal.wait_for_append_turn(2).await
        }
    });
    started_rx.await.unwrap();

    tokio::task::spawn_blocking({
        let journal = journal.clone();
        move || journal.record_reserved_snapshot_groups(10, vec![(1, Vec::new())])
    })
    .await
    .unwrap()
    .unwrap();
    waiter.await.unwrap().unwrap();
    journal
        .record_reserved_snapshot_groups(10, vec![(2, Vec::new())])
        .unwrap();
}

#[tokio::test]
async fn checkpoint_poison_wakes_append_turn_waiter() {
    let temp = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let (requests, receiver) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let WriterRequest::Reserve { .. } = receiver.recv().unwrap() else {
            panic!("expected reservation request");
        };
        let WriterRequest::Append { .. } = receiver.recv().unwrap() else {
            panic!("expected append request");
        };
        let WriterRequest::Replace { reply, .. } = receiver.recv().unwrap() else {
            panic!("expected checkpoint request");
        };
        reply
            .send(Err(WriterFailure::Io(std::io::Error::other("injected"))))
            .unwrap();
        let WriterRequest::Shutdown { reply } = receiver.recv().unwrap() else {
            panic!("expected shutdown request");
        };
        reply.send(()).unwrap();
    });
    let journal = WorldChunkJournal::from_parts_for_test(
        temp.path().join("solaris/world-chunk-journal.bin"),
        blocks,
        items,
        requests,
        worker,
    );
    assert_eq!(journal.reserve_decision_ids(3).unwrap(), vec![1, 2, 3]);
    journal
        .record_reserved_snapshot_groups(10, vec![(1, Vec::new())])
        .unwrap();

    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let waiter = tokio::spawn({
        let journal = journal.clone();
        async move {
            started_tx.send(()).unwrap();
            journal.wait_for_append_turn(3).await
        }
    });
    started_rx.await.unwrap();
    let checkpoint = tokio::task::spawn_blocking({
        let journal = journal.clone();
        move || journal.checkpoint_through(1)
    })
    .await
    .unwrap();
    assert!(matches!(
        checkpoint,
        Err(WorldChunkJournalError::CheckpointOutcomeUnknown { .. })
    ));
    assert!(matches!(
        waiter.await.unwrap(),
        Err(WorldChunkJournalError::PoisonedOutcomeUnknown)
    ));
}

#[tokio::test]
async fn closed_writer_wakes_append_turn_waiter() {
    let temp = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let (requests, receiver) = std::sync::mpsc::sync_channel(1);
    let (closed, completion) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let WriterRequest::Reserve { .. } = receiver.recv().unwrap() else {
            panic!("expected reservation request");
        };
        drop(receiver);
        closed.send(()).unwrap();
    });
    let journal = WorldChunkJournal::from_parts_for_test(
        temp.path().join("solaris/world-chunk-journal.bin"),
        blocks,
        items,
        requests,
        worker,
    );
    assert_eq!(journal.reserve_decision_ids(2).unwrap(), vec![1, 2]);
    completion.recv().unwrap();

    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let waiter = tokio::spawn({
        let journal = journal.clone();
        async move {
            started_tx.send(()).unwrap();
            journal.wait_for_append_turn(2).await
        }
    });
    started_rx.await.unwrap();
    let append = tokio::task::spawn_blocking({
        let journal = journal.clone();
        move || journal.record_reserved_snapshot_groups(10, vec![(1, Vec::new())])
    })
    .await
    .unwrap();
    assert!(matches!(
        append,
        Err(WorldChunkJournalError::WriterClosed {
            operation: "append"
        })
    ));
    assert!(matches!(
        waiter.await.unwrap(),
        Err(WorldChunkJournalError::PoisonedOutcomeUnknown)
    ));
}

#[test]
fn checkpoint_failure_poisons_follow_up_writes() {
    let temp = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let (requests, receiver) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let WriterRequest::Reserve { .. } = receiver.recv().unwrap() else {
            panic!("expected reservation request");
        };
        let WriterRequest::Append { .. } = receiver.recv().unwrap() else {
            panic!("expected append request");
        };
        let WriterRequest::Replace { reply, .. } = receiver.recv().unwrap() else {
            panic!("expected checkpoint request");
        };
        reply
            .send(Err(WriterFailure::Io(std::io::Error::other("injected"))))
            .unwrap();
    });
    let journal = WorldChunkJournal::from_parts_for_test(
        temp.path().join("solaris/world-chunk-journal.bin"),
        Arc::clone(&blocks),
        items,
        requests,
        worker,
    );
    journal
        .record_snapshots(10, vec![snapshot(&blocks, ChunkPos { x: 0, z: 0 }, 10)])
        .unwrap();

    let error = journal.checkpoint_through(1).unwrap_err();
    assert!(error.outcome_unknown());
    let error = journal
        .record_snapshots(20, vec![snapshot(&blocks, ChunkPos { x: 1, z: 0 }, 20)])
        .unwrap_err();
    assert!(matches!(
        error,
        WorldChunkJournalError::PoisonedOutcomeUnknown
    ));
}

#[test]
fn append_outcome_unknown_recovery_uses_the_persisted_frame() {
    let temp = tempfile::tempdir().unwrap();
    let (blocks, items) = registries();
    let (requests, receiver) = std::sync::mpsc::sync_channel(1);
    let path = temp.path().join("solaris/world-chunk-journal.bin");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let writer_path = path.clone();
    let worker = std::thread::spawn(move || {
        let WriterRequest::Reserve { allocated_high } = receiver.recv().unwrap() else {
            panic!("expected reservation request");
        };
        write_header(&mut File::create(&writer_path).unwrap(), 0, allocated_high).unwrap();
        let WriterRequest::Append { bytes } = receiver.recv().unwrap() else {
            panic!("expected append request");
        };
        let mut file = OpenOptions::new().append(true).open(&writer_path).unwrap();
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
    });
    let journal = WorldChunkJournal::from_parts_for_test(
        path,
        blocks.clone(),
        items.clone(),
        requests,
        worker,
    );

    journal
        .record_snapshots(10, vec![snapshot(&blocks, ChunkPos { x: 0, z: 0 }, 10)])
        .expect("accepted in RAM before writer stops");
    assert_eq!(journal.watermark(), Some(1));
    drop(journal);

    let (_reopened, pending) =
        WorldChunkJournal::open_for_test(temp.path(), blocks, items).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id(), 1);
}

fn flip_byte(path: &Path, offset: u64) {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    let mut byte = [0_u8; 1];
    file.read_exact(&mut byte).unwrap();
    byte[0] ^= 0xff;
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(&byte).unwrap();
    file.sync_all().unwrap();
}
