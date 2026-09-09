use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use mc_data::items::ItemRegistry;
use mc_world::anvil::{chunk_from_nbt_with_items, chunk_to_payload_with_items_at_tick};
use mc_world::{BlockRegistry, Chunk, ChunkPos, ChunkSnapshot};
use thiserror::Error;

use crate::lock_policy::lock_authoritative_mutex;

const SOLARIS_DIRECTORY: &str = "solaris";
const JOURNAL_FILE: &str = "world-chunk-journal.bin";
const JOURNAL_LOCK_FILE: &str = "world-chunk-journal.lock";
const JOURNAL_MAGIC: &[u8] = b"SOLARIS_WORLD_CHUNK_JOURNAL";
const JOURNAL_VERSION: u32 = 3;
const JOURNAL_HEADER_BYTES: usize = JOURNAL_MAGIC.len() + size_of::<u32>() + size_of::<u64>() * 2;
const MAX_JOURNAL_ID: u64 = i64::MAX as u64;
const FRAME_MAGIC: &[u8; 4] = b"WCF1";
const FRAME_PREFIX_BYTES: usize = FRAME_MAGIC.len() + size_of::<u64>();
const FRAME_SUFFIX_BYTES: usize = size_of::<u32>();
const DECISION_FIXED_BYTES: usize = size_of::<u64>() * 2 + size_of::<u32>();
const IMAGE_PREFIX_BYTES: usize = size_of::<i32>() * 2 + size_of::<u32>();
const MAX_IMAGES_PER_DECISION: usize = 512;
const MAX_PENDING_DECISIONS: usize = 65_536;
const MAX_IMAGE_NBT_BYTES: usize = mc_nbt::MAX_NBT_LENGTH;
const MAX_FRAME_BYTES: u64 = 64 * 1024 * 1024;
const MAX_JOURNAL_FILE_BYTES: u64 = 256 * 1024 * 1024;
const WRITER_QUEUE_CAPACITY: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorldChunkImage {
    position: ChunkPos,
    nbt: Vec<u8>,
}

#[cfg(test)]
impl WorldChunkImage {
    fn position(&self) -> ChunkPos {
        self.position
    }

    fn nbt(&self) -> &[u8] {
        &self.nbt
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorldChunkDecision {
    id: u64,
    current_tick: u64,
    images: Vec<WorldChunkImage>,
}

impl WorldChunkDecision {
    #[must_use]
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    #[cfg(test)]
    fn current_tick(&self) -> u64 {
        self.current_tick
    }

    #[cfg(test)]
    fn images(&self) -> &[WorldChunkImage] {
        &self.images
    }

    pub(crate) fn decode(
        &self,
        blocks: &BlockRegistry,
        items: &ItemRegistry,
    ) -> Result<Vec<Chunk>, WorldChunkJournalError> {
        self.images
            .iter()
            .map(|image| decode_image(self.id, image, blocks, items))
            .collect()
    }
}

#[derive(Debug, Error)]
pub(crate) enum WorldChunkJournalError {
    #[error("world chunk journal IO failed during {operation} at {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("unsupported world chunk journal version {0}")]
    UnsupportedVersion(u32),
    #[error("world chunk journal is corrupt at byte {offset}: {reason}")]
    Corrupt { offset: u64, reason: String },
    #[error("world chunk journal frame is too large: {0} bytes")]
    FrameTooLarge(u64),
    #[error("world chunk journal file is too large: {0} bytes")]
    JournalTooLarge(u64),
    #[error("world chunk journal could not reserve {0} bytes")]
    AllocationFailed(usize),
    #[error("world chunk journal decision has too many images: {0}")]
    TooManyImages(usize),
    #[error("world chunk journal image {position:?} is too large: {bytes} bytes")]
    ImageTooLarge { position: ChunkPos, bytes: usize },
    #[cfg(test)]
    #[error("cannot journal an empty chunk snapshot group")]
    EmptySnapshotGroup,
    #[error("world chunk journal record id space is exhausted")]
    RecordIdExhausted,
    #[error("world chunk journal reserved decision ids are invalid")]
    InvalidReservation,
    #[error("journal decision {decision_id} contains chunk {position:?} stamped with LSN {actual}")]
    SnapshotLsnMismatch {
        decision_id: u64,
        position: ChunkPos,
        actual: u64,
    },
    #[error("chunk {position:?} could not be encoded for the world journal: {source}")]
    EncodeChunk {
        position: ChunkPos,
        #[source]
        source: mc_world::anvil::ChunkNbtError,
    },
    #[error("journal decision {decision_id} chunk {position:?} contains invalid NBT: {source}")]
    DecodeNbt {
        decision_id: u64,
        position: ChunkPos,
        #[source]
        source: mc_nbt::NbtError,
    },
    #[error("journal decision {decision_id} chunk {position:?} has trailing NBT bytes")]
    TrailingNbt {
        decision_id: u64,
        position: ChunkPos,
    },
    #[error("journal decision {decision_id} chunk {position:?} could not be decoded: {source}")]
    DecodeChunk {
        decision_id: u64,
        position: ChunkPos,
        #[source]
        source: mc_world::anvil::ChunkNbtError,
    },
    #[error("journal decision {decision_id} stores chunk {stored:?} in image slot {declared:?}")]
    PositionMismatch {
        decision_id: u64,
        declared: ChunkPos,
        stored: ChunkPos,
    },
    #[error("world chunk journal writer closed during {operation}")]
    WriterClosed { operation: &'static str },
    #[error("world chunk journal checkpoint outcome is unknown at {path}: {source}")]
    CheckpointOutcomeUnknown {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("world chunk journal checkpoint completion was lost; its outcome is unknown")]
    CheckpointCompletionLost,
    #[error("world chunk journal is poisoned by an earlier append with unknown outcome")]
    PoisonedOutcomeUnknown,
}

impl WorldChunkJournalError {
    #[must_use]
    pub(crate) fn outcome_unknown(&self) -> bool {
        matches!(
            self,
            Self::CheckpointOutcomeUnknown { .. }
                | Self::CheckpointCompletionLost
                | Self::PoisonedOutcomeUnknown
        )
    }
}

pub(crate) struct JournalWriter {
    pub(super) requests: std::sync::mpsc::SyncSender<WriterRequest>,
    pub(super) failed: Arc<AtomicBool>,
    pub(super) failure_reporter: Arc<Mutex<Option<tokio::sync::watch::Sender<bool>>>>,
    advanced: Arc<tokio::sync::Notify>,
    worker: Option<std::thread::JoinHandle<()>>,
    lease: Mutex<Option<File>>,
}

impl JournalWriter {
    pub(crate) fn open(world_root: &Path) -> std::io::Result<Arc<Self>> {
        let (requests, receiver) = std::sync::mpsc::sync_channel(WRITER_QUEUE_CAPACITY);
        let failed = Arc::new(AtomicBool::new(false));
        let failure_reporter = Arc::new(Mutex::new(None::<tokio::sync::watch::Sender<bool>>));
        let advanced = Arc::new(tokio::sync::Notify::new());
        let directory = world_root.join(SOLARIS_DIRECTORY);
        let worker_failed = Arc::clone(&failed);
        let worker_reporter = Arc::clone(&failure_reporter);
        let worker_advanced = Arc::clone(&advanced);
        let worker = std::thread::Builder::new()
            .name("solaris-journal".to_owned())
            .spawn(move || {
                if let Err(error) = run_writer(&directory, receiver) {
                    tracing::error!(?error, "journal writer failed");
                    worker_failed.store(true, Ordering::Release);
                    if let Some(reporter) =
                        &*worker_reporter.lock().expect("journal failure reporter")
                    {
                        reporter.send_replace(true);
                    }
                    worker_advanced.notify_waiters();
                }
            })?;
        Ok(Arc::new(Self {
            requests,
            failed,
            failure_reporter,
            advanced,
            worker: Some(worker),
            lease: Mutex::new(None),
        }))
    }

    pub(crate) fn flush(&self) -> Result<(), WorldChunkJournalError> {
        let (reply, completion) = std::sync::mpsc::sync_channel(0);
        self.requests
            .send(WriterRequest::Flush { reply })
            .map_err(|_| WorldChunkJournalError::WriterClosed { operation: "flush" })?;
        completion
            .recv()
            .map_err(|_| WorldChunkJournalError::CheckpointCompletionLost)
    }
}

impl Drop for JournalWriter {
    fn drop(&mut self) {
        let (reply, completion) = std::sync::mpsc::channel();
        let _ = self.requests.send(WriterRequest::Shutdown { reply });
        let _ = completion.recv();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Clone)]
pub(crate) struct WorldChunkJournal {
    shared: Arc<JournalShared>,
    pub(crate) writer: Arc<JournalWriter>,
}

struct JournalShared {
    blocks: Arc<BlockRegistry>,
    items: Arc<ItemRegistry>,
    state: Mutex<JournalState>,
    append_advanced: Arc<tokio::sync::Notify>,
}

impl JournalShared {
    fn lock_state(&self) -> std::sync::MutexGuard<'_, JournalState> {
        let mut state = lock_authoritative_mutex(&self.state, "persistence.world_chunk_journal");
        state.poisoned |= state.writer.failed.load(Ordering::Acquire);
        state
    }
}

struct JournalState {
    path: PathBuf,
    checkpoint_base: u64,
    pending: Vec<WorldChunkDecision>,
    next_id: u64,
    next_append_id: u64,
    poisoned: bool,
    requests: std::sync::mpsc::SyncSender<WriterRequest>,
    writer: Arc<JournalWriter>,
}

impl fmt::Debug for WorldChunkJournal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("WorldChunkJournal");
        match self.shared.state.try_lock() {
            Ok(state) => debug
                .field("path", &state.path)
                .field("pending_decisions", &state.pending.len())
                .field(
                    "watermark",
                    &state.pending.last().map(WorldChunkDecision::id),
                )
                .field("poisoned", &state.poisoned)
                .finish(),
            Err(_) => debug.field("state", &"busy").finish(),
        }
    }
}

impl WorldChunkJournal {
    #[cfg(test)]
    pub(crate) fn open_for_test(
        root: &Path,
        blocks: Arc<BlockRegistry>,
        items: Arc<ItemRegistry>,
    ) -> Result<(Self, Vec<WorldChunkDecision>), WorldChunkJournalError> {
        Self::open(root, blocks, items, JournalWriter::open(root).unwrap())
    }

    pub(crate) fn open(
        world_root: &Path,
        blocks: Arc<BlockRegistry>,
        items: Arc<ItemRegistry>,
        writer: Arc<JournalWriter>,
    ) -> Result<(Self, Vec<WorldChunkDecision>), WorldChunkJournalError> {
        let path = world_root.join(SOLARIS_DIRECTORY).join(JOURNAL_FILE);
        let directory = path.parent().expect("journal directory");
        ensure_journal_directory(directory).map_err(|source| WorldChunkJournalError::Io {
            operation: "create journal directory",
            path: directory.to_path_buf(),
            source,
        })?;
        let lease_path = directory.join(JOURNAL_LOCK_FILE);
        let lease = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lease_path)
            .and_then(|file| {
                file.try_lock().map_err(std::io::Error::from)?;
                Ok(file)
            })
            .map_err(|source| WorldChunkJournalError::Io {
                operation: "acquire journal lease",
                path: lease_path,
                source,
            })?;
        let (base_id, allocated_high, pending) = read_and_repair_journal(&path)?;
        let next_id = pending
            .iter()
            .map(WorldChunkDecision::id)
            .max()
            .unwrap_or(allocated_high)
            .max(base_id)
            .max(allocated_high);
        let next_append_id = match next_id.checked_add(1).filter(|_| next_id <= MAX_JOURNAL_ID) {
            Some(next) => next,
            None => {
                drop(lease);
                return Err(WorldChunkJournalError::RecordIdExhausted);
            }
        };
        *writer.lease.lock().expect("journal lease") = Some(lease);
        let journal = Self {
            shared: Arc::new(JournalShared {
                blocks,
                items,
                append_advanced: Arc::clone(&writer.advanced),
                state: Mutex::new(JournalState {
                    path,
                    checkpoint_base: base_id,
                    pending: pending.clone(),
                    next_id,
                    next_append_id,
                    poisoned: false,
                    requests: writer.requests.clone(),
                    writer: Arc::clone(&writer),
                }),
            }),
            writer,
        };
        Ok((journal, pending))
    }

    #[cfg(test)]
    pub(crate) fn record_snapshots(
        &self,
        current_tick: u64,
        snapshots: Vec<ChunkSnapshot>,
    ) -> Result<u64, WorldChunkJournalError> {
        if snapshots.is_empty() {
            return Err(WorldChunkJournalError::EmptySnapshotGroup);
        }
        let images = self.encode_images(current_tick, snapshots)?;

        let mut state = self.shared.lock_state();
        if state.poisoned {
            return Err(WorldChunkJournalError::PoisonedOutcomeUnknown);
        }
        if state.next_append_id
            != state
                .next_id
                .checked_add(1)
                .ok_or(WorldChunkJournalError::RecordIdExhausted)?
        {
            return Err(WorldChunkJournalError::InvalidReservation);
        }
        let id = reserve_ids_locked(&mut state, 1)?
            .into_iter()
            .next()
            .expect("one reserved id");
        let decision = WorldChunkDecision {
            id,
            current_tick,
            images,
        };
        if let Err(error) = append_decisions(&mut state, vec![decision]) {
            drop(state);
            self.shared.append_advanced.notify_waiters();
            return Err(error);
        }
        state.next_append_id = id
            .checked_add(1)
            .ok_or(WorldChunkJournalError::RecordIdExhausted)?;
        drop(state);
        self.shared.append_advanced.notify_waiters();
        Ok(id)
    }

    pub(crate) fn reserve_decision_ids(
        &self,
        count: usize,
    ) -> Result<Vec<u64>, WorldChunkJournalError> {
        let mut state = self.shared.lock_state();
        if state.poisoned {
            return Err(WorldChunkJournalError::PoisonedOutcomeUnknown);
        }
        let result = reserve_ids_locked(&mut state, count);
        let poisoned = state.poisoned;
        drop(state);
        if poisoned {
            self.shared.append_advanced.notify_waiters();
        }
        result
    }

    pub(crate) fn record_reserved_snapshot_groups(
        &self,
        current_tick: u64,
        groups: Vec<(u64, Vec<ChunkSnapshot>)>,
    ) -> Result<(), WorldChunkJournalError> {
        if groups.is_empty() {
            return Ok(());
        }
        let mut decisions = Vec::with_capacity(groups.len());
        for (id, snapshots) in groups {
            for snapshot in &snapshots {
                let actual = snapshot.world_journal_lsn();
                if actual != id {
                    return Err(WorldChunkJournalError::SnapshotLsnMismatch {
                        decision_id: id,
                        position: snapshot.pos,
                        actual,
                    });
                }
            }
            decisions.push(WorldChunkDecision {
                id,
                current_tick,
                images: self.encode_images(current_tick, snapshots)?,
            });
        }

        let mut state = self.shared.lock_state();
        if state.poisoned {
            return Err(WorldChunkJournalError::PoisonedOutcomeUnknown);
        }
        let first_id = decisions
            .first()
            .map(WorldChunkDecision::id)
            .expect("non-empty reserved decision group");
        let last_id = decisions
            .last()
            .map(WorldChunkDecision::id)
            .expect("non-empty reserved decision group");
        if first_id != state.next_append_id
            || last_id > state.next_id
            || !decisions
                .iter()
                .map(WorldChunkDecision::id)
                .eq(first_id..=last_id)
        {
            return Err(WorldChunkJournalError::InvalidReservation);
        }
        if let Err(error) = append_decisions(&mut state, decisions) {
            drop(state);
            self.shared.append_advanced.notify_waiters();
            return Err(error);
        }
        state.next_append_id = last_id
            .checked_add(1)
            .ok_or(WorldChunkJournalError::RecordIdExhausted)?;
        drop(state);
        self.shared.append_advanced.notify_waiters();
        Ok(())
    }

    pub(crate) async fn wait_for_append_turn(
        &self,
        decision_id: u64,
    ) -> Result<(), WorldChunkJournalError> {
        loop {
            let advanced = self.shared.append_advanced.notified();
            {
                let state = self.shared.lock_state();
                if state.poisoned {
                    return Err(WorldChunkJournalError::PoisonedOutcomeUnknown);
                }
                if state.next_append_id == decision_id {
                    return Ok(());
                }
                if state.next_append_id > decision_id || decision_id > state.next_id {
                    return Err(WorldChunkJournalError::InvalidReservation);
                }
            }
            advanced.await;
        }
    }

    fn encode_images(
        &self,
        current_tick: u64,
        snapshots: Vec<ChunkSnapshot>,
    ) -> Result<Vec<WorldChunkImage>, WorldChunkJournalError> {
        snapshots
            .into_iter()
            .map(|snapshot| {
                let position = snapshot.pos;
                let payload = chunk_to_payload_with_items_at_tick(
                    &snapshot,
                    &self.shared.blocks,
                    Some(&self.shared.items),
                    0,
                    current_tick,
                )
                .map_err(|source| WorldChunkJournalError::EncodeChunk { position, source })?;
                Ok(WorldChunkImage {
                    position,
                    nbt: payload.uncompressed_nbt,
                })
            })
            .collect()
    }

    #[must_use]
    pub(crate) fn watermark(&self) -> Option<u64> {
        self.shared
            .lock_state()
            .pending
            .last()
            .map(WorldChunkDecision::id)
    }

    #[cfg(test)]
    pub(crate) fn pending_decisions_for_test(&self) -> Vec<WorldChunkDecision> {
        self.shared.lock_state().pending.clone()
    }

    pub(crate) fn checkpoint_through(&self, watermark: u64) -> Result<(), WorldChunkJournalError> {
        let mut state = self.shared.lock_state();
        if state.poisoned {
            return Err(WorldChunkJournalError::PoisonedOutcomeUnknown);
        }
        let first_retained = state
            .pending
            .partition_point(|decision| decision.id <= watermark);
        if first_retained == 0 {
            return Ok(());
        }
        let checkpoint_base = state.pending[first_retained - 1].id;
        let retained = &state.pending[first_retained..];
        let replacement = encode_journal(checkpoint_base, state.next_id, retained)?;
        let (reply, completion) = std::sync::mpsc::channel();
        if state
            .requests
            .send(WriterRequest::Replace { replacement, reply })
            .is_err()
        {
            state.poisoned = true;
            drop(state);
            self.shared.append_advanced.notify_waiters();
            return Err(WorldChunkJournalError::WriterClosed {
                operation: "checkpoint",
            });
        }
        match completion.recv() {
            Ok(Ok(())) => {}
            Ok(Err(WriterFailure::JournalTooLarge(bytes))) => {
                return Err(WorldChunkJournalError::JournalTooLarge(bytes));
            }
            Ok(Err(WriterFailure::Io(source))) => {
                state.poisoned = true;
                let error = WorldChunkJournalError::CheckpointOutcomeUnknown {
                    path: state.path.clone(),
                    source,
                };
                drop(state);
                self.shared.append_advanced.notify_waiters();
                return Err(error);
            }
            Err(_) => {
                state.poisoned = true;
                drop(state);
                self.shared.append_advanced.notify_waiters();
                return Err(WorldChunkJournalError::CheckpointCompletionLost);
            }
        }
        state.pending.drain(..first_retained);
        state.checkpoint_base = checkpoint_base;
        Ok(())
    }

    pub(crate) fn decode_decision(
        &self,
        decision: &WorldChunkDecision,
    ) -> Result<Vec<Chunk>, WorldChunkJournalError> {
        decision.decode(&self.shared.blocks, &self.shared.items)
    }

    pub(crate) fn decode_pending(
        &self,
        pending: &[WorldChunkDecision],
    ) -> Result<Vec<Chunk>, WorldChunkJournalError> {
        let chunk_count = pending.iter().map(|decision| decision.images.len()).sum();
        let mut chunks = Vec::with_capacity(chunk_count);
        for decision in pending {
            chunks.extend(self.decode_decision(decision)?);
        }
        Ok(chunks)
    }

    #[cfg(test)]
    pub(super) fn from_parts_for_test(
        path: PathBuf,
        blocks: Arc<BlockRegistry>,
        items: Arc<ItemRegistry>,
        requests: std::sync::mpsc::SyncSender<WriterRequest>,
        worker: std::thread::JoinHandle<()>,
    ) -> Self {
        let writer = Arc::new(JournalWriter {
            requests: requests.clone(),
            failed: Arc::new(AtomicBool::new(false)),
            failure_reporter: Arc::new(Mutex::new(None)),
            advanced: Arc::new(tokio::sync::Notify::new()),
            worker: Some(worker),
            lease: Mutex::new(None),
        });
        Self {
            shared: Arc::new(JournalShared {
                blocks,
                items,
                append_advanced: Arc::new(tokio::sync::Notify::new()),
                state: Mutex::new(JournalState {
                    path,
                    checkpoint_base: 0,
                    pending: Vec::new(),
                    next_id: 0,
                    next_append_id: 1,
                    poisoned: false,
                    requests: requests.clone(),
                    writer: Arc::clone(&writer),
                }),
            }),
            writer,
        }
    }
}

fn reserve_ids_locked(
    state: &mut JournalState,
    count: usize,
) -> Result<Vec<u64>, WorldChunkJournalError> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let count = u64::try_from(count).map_err(|_| WorldChunkJournalError::RecordIdExhausted)?;
    let first = state
        .next_id
        .checked_add(1)
        .ok_or(WorldChunkJournalError::RecordIdExhausted)?;
    let last = state
        .next_id
        .checked_add(count)
        .ok_or(WorldChunkJournalError::RecordIdExhausted)?;
    if last > MAX_JOURNAL_ID {
        return Err(WorldChunkJournalError::RecordIdExhausted);
    }
    if state
        .requests
        .send(WriterRequest::Reserve {
            allocated_high: last,
        })
        .is_err()
    {
        state.poisoned = true;
        return Err(WorldChunkJournalError::WriterClosed {
            operation: "reserve decision ids",
        });
    }
    state.next_id = last;
    Ok((first..=last).collect())
}

fn append_decisions(
    state: &mut JournalState,
    decisions: Vec<WorldChunkDecision>,
) -> Result<(), WorldChunkJournalError> {
    let bytes = (|| {
        let encoded_len = encoded_decisions_len(&decisions, 0)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(encoded_len)
            .map_err(|_| WorldChunkJournalError::AllocationFailed(encoded_len))?;
        for decision in &decisions {
            bytes.extend_from_slice(&encode_frame(decision)?);
        }
        Ok(bytes)
    })();
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(error) => {
            state.poisoned = true;
            return Err(error);
        }
    };
    if state
        .requests
        .send(WriterRequest::Append { bytes })
        .is_err()
    {
        state.poisoned = true;
        return Err(WorldChunkJournalError::WriterClosed {
            operation: "append",
        });
    }
    state.pending.extend(decisions);
    Ok(())
}

pub(super) enum WriterRequest {
    Reserve {
        allocated_high: u64,
    },
    Append {
        bytes: Vec<u8>,
    },
    EntityAppend {
        decisions: Vec<mc_entity::RegionalCommitDecision>,
    },
    EntityReplace {
        pending: Vec<mc_entity::RegionalCommitDecision>,
        reply: std::sync::mpsc::Sender<Result<(), mc_entity::RegionalDecisionJournalError>>,
    },
    Flush {
        reply: std::sync::mpsc::SyncSender<()>,
    },
    Replace {
        replacement: Vec<u8>,
        reply: std::sync::mpsc::Sender<Result<(), WriterFailure>>,
    },
    Shutdown {
        reply: std::sync::mpsc::Sender<()>,
    },
}

#[derive(Debug)]
pub(super) enum WriterFailure {
    Io(std::io::Error),
    JournalTooLarge(u64),
}

impl From<std::io::Error> for WriterFailure {
    fn from(source: std::io::Error) -> Self {
        Self::Io(source)
    }
}

fn run_writer(
    directory: &Path,
    receiver: std::sync::mpsc::Receiver<WriterRequest>,
) -> Result<(), WriterFailure> {
    let paths = [
        directory.join(JOURNAL_FILE),
        directory.join(super::persistence::REGIONAL_DECISION_JOURNAL_FILE),
    ];
    let mut batch = std::collections::VecDeque::with_capacity(WRITER_QUEUE_CAPACITY);
    let mut dirty = [false; 2];
    while let Ok(first) = receiver.recv() {
        batch.push_back(first);
        batch.extend(receiver.try_iter().take(WRITER_QUEUE_CAPACITY - 1));
        while !batch.is_empty() {
            let count = batch
                .iter()
                .position(|request| {
                    matches!(
                        request,
                        WriterRequest::Replace { .. }
                            | WriterRequest::EntityReplace { .. }
                            | WriterRequest::Flush { .. }
                            | WriterRequest::Shutdown { .. }
                    )
                })
                .unwrap_or(batch.len());
            let allocated_high = batch
                .iter()
                .take(count)
                .filter_map(|request| {
                    if let WriterRequest::Reserve { allocated_high } = request {
                        Some(*allocated_high)
                    } else {
                        None
                    }
                })
                .max();
            if let Some(allocated_high) = allocated_high {
                ensure_journal_directory(directory)?;
                let mut file = OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .open(&paths[0])?;
                if file.metadata()?.len() == 0 {
                    write_header(&mut file, 0, allocated_high)?;
                } else {
                    file.seek(SeekFrom::Start(
                        (JOURNAL_HEADER_BYTES - size_of::<u64>()) as u64,
                    ))?;
                    file.write_all(&allocated_high.to_le_bytes())?;
                }
                // Persist the allocation prefix before either WAL can reference its IDs.
                file.sync_all()?;
                sync_directory(directory)?;
            }
            for request in batch.drain(..count) {
                match request {
                    WriterRequest::Reserve { .. } => {}
                    WriterRequest::Append { bytes } => {
                        append_frames(&paths[0], &bytes)?;
                        dirty[0] = true;
                    }
                    WriterRequest::EntityAppend { decisions } => {
                        super::persistence::append_regional_decisions(&paths[1], &decisions)
                            .map_err(|error| WriterFailure::Io(std::io::Error::other(error)))?;
                        dirty[1] |= !decisions.is_empty();
                    }
                    _ => unreachable!("checkpoint bounds the append batch"),
                }
            }
            sync_journal_batch(directory, &paths, &mut dirty)?;
            match batch.pop_front() {
                Some(WriterRequest::Replace { replacement, reply }) => {
                    if let Err(error) = replace_journal(&paths[0], &replacement) {
                        let _ = reply.send(Err(error));
                        return Err(std::io::Error::other("world journal checkpoint failed").into());
                    }
                    let _ = reply.send(Ok(()));
                }
                Some(WriterRequest::EntityReplace { pending, reply }) => {
                    if let Err(error) =
                        super::persistence::persist_regional_decisions(&paths[1], &pending)
                    {
                        let _ = reply.send(Err(
                            mc_entity::RegionalDecisionJournalError::OUTCOME_UNKNOWN,
                        ));
                        return Err(std::io::Error::other(error).into());
                    }
                    let _ = reply.send(Ok(()));
                }
                Some(WriterRequest::Flush { reply }) => {
                    let _ = reply.send(());
                }
                Some(WriterRequest::Shutdown { reply }) => {
                    let _ = reply.send(());
                    return Ok(());
                }
                None => {}
                _ => unreachable!("append batch ends at a checkpoint"),
            }
        }
    }
    Ok(())
}

fn sync_journal_batch(
    directory: &Path,
    paths: &[PathBuf; 2],
    dirty: &mut [bool; 2],
) -> Result<(), WriterFailure> {
    if dirty.iter().any(|&dirty| dirty) {
        for (path, dirty) in paths.iter().zip(dirty.iter_mut()) {
            if *dirty {
                File::open(path)?.sync_all()?;
                *dirty = false;
            }
        }
        sync_directory(directory)?;
    }
    Ok(())
}

fn append_frames(path: &Path, bytes: &[u8]) -> Result<(), WriterFailure> {
    let directory = path.parent().expect("journal path has a parent");
    ensure_journal_directory(directory)?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let current_len = file.metadata()?.len();
    let header_len = if current_len == 0 {
        u64::try_from(JOURNAL_HEADER_BYTES).expect("journal header length fits u64")
    } else {
        0
    };
    let appended_len = u64::try_from(bytes.len()).expect("usize always fits u64");
    let final_len = current_len
        .checked_add(header_len)
        .and_then(|len| len.checked_add(appended_len))
        .ok_or(WriterFailure::JournalTooLarge(u64::MAX))?;
    if final_len > MAX_JOURNAL_FILE_BYTES {
        return Err(WriterFailure::JournalTooLarge(final_len));
    }
    if current_len == 0 {
        write_header(&mut file, 0, 0)?;
    }
    file.write_all(bytes)?;
    Ok(())
}

fn replace_journal(path: &Path, replacement: &[u8]) -> Result<(), WriterFailure> {
    let replacement_len = u64::try_from(replacement.len()).expect("usize always fits u64");
    if replacement_len > MAX_JOURNAL_FILE_BYTES {
        return Err(WriterFailure::JournalTooLarge(replacement_len));
    }
    let directory = path.parent().expect("journal path has a parent");
    ensure_journal_directory(directory)?;
    let temporary = path.with_extension("bin.tmp");
    let result = (|| {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(replacement)?;
        file.flush()?;
        file.sync_all()?;
        mc_world::atomic_file::replace_file(&temporary, path)?;
        sync_directory(directory)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result.map_err(WriterFailure::Io)
}

fn ensure_journal_directory(directory: &Path) -> std::io::Result<()> {
    if directory.is_dir() {
        return Ok(());
    }
    let world_root = directory.parent().expect("solaris directory has a parent");
    std::fs::create_dir_all(directory)?;
    sync_directory(world_root)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn read_and_repair_journal(
    path: &Path,
) -> Result<(u64, u64, Vec<WorldChunkDecision>), WorldChunkJournalError> {
    let mut file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((0, 0, Vec::new()));
        }
        Err(source) => {
            return Err(WorldChunkJournalError::Io {
                operation: "open",
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let file_len = file
        .metadata()
        .map_err(|source| WorldChunkJournalError::Io {
            operation: "stat",
            path: path.to_path_buf(),
            source,
        })?
        .len();
    if file_len > MAX_JOURNAL_FILE_BYTES {
        return Err(WorldChunkJournalError::JournalTooLarge(file_len));
    }
    let capacity =
        usize::try_from(file_len).map_err(|_| WorldChunkJournalError::JournalTooLarge(file_len))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|_| WorldChunkJournalError::AllocationFailed(capacity))?;
    bytes.resize(capacity, 0);
    file.read_exact(&mut bytes)
        .map_err(|source| WorldChunkJournalError::Io {
            operation: "read",
            path: path.to_path_buf(),
            source,
        })?;
    let observed_len = file
        .metadata()
        .map_err(|source| WorldChunkJournalError::Io {
            operation: "restat",
            path: path.to_path_buf(),
            source,
        })?
        .len();
    if observed_len != file_len {
        return Err(corrupt(
            0,
            format!("journal length changed during recovery: {file_len} -> {observed_len}"),
        ));
    }
    let (base_id, allocated_high, pending, valid_len) = decode_journal(&bytes)?;
    if valid_len < bytes.len() {
        file.set_len(valid_len as u64)
            .and_then(|()| file.sync_all())
            .map_err(|source| WorldChunkJournalError::Io {
                operation: "repair final frame",
                path: path.to_path_buf(),
                source,
            })?;
    }
    Ok((base_id, allocated_high, pending))
}

fn decode_journal(
    bytes: &[u8],
) -> Result<(u64, u64, Vec<WorldChunkDecision>, usize), WorldChunkJournalError> {
    if bytes.is_empty() {
        return Ok((0, 0, Vec::new(), 0));
    }
    if bytes.len() < JOURNAL_HEADER_BYTES {
        if JOURNAL_MAGIC.starts_with(bytes) {
            return Ok((0, 0, Vec::new(), 0));
        }
        return Err(corrupt(0, "invalid or incomplete file header"));
    }
    if &bytes[..JOURNAL_MAGIC.len()] != JOURNAL_MAGIC {
        return Err(corrupt(0, "invalid file magic"));
    }
    let version_offset = JOURNAL_MAGIC.len();
    let version_end = version_offset + size_of::<u32>();
    let version = u32::from_le_bytes(
        bytes[version_offset..version_end]
            .try_into()
            .expect("version slice has fixed length"),
    );
    if version != JOURNAL_VERSION {
        return Err(WorldChunkJournalError::UnsupportedVersion(version));
    }
    let base_end = version_end + size_of::<u64>();
    let base_id = u64::from_le_bytes(
        bytes[version_end..base_end]
            .try_into()
            .expect("base id slice has fixed length"),
    );
    let allocated_high = u64::from_le_bytes(
        bytes[base_end..JOURNAL_HEADER_BYTES]
            .try_into()
            .expect("allocated high slice has fixed length"),
    );
    if allocated_high < base_id {
        return Err(corrupt(
            0,
            "allocated high watermark is below checkpoint base",
        ));
    }

    let mut pending = Vec::new();
    let mut offset = JOURNAL_HEADER_BYTES;
    while offset < bytes.len() {
        match decode_frame_at(bytes, offset) {
            Ok((decision, next_offset)) => {
                let previous_id = pending.last().map_or(base_id, WorldChunkDecision::id);
                if previous_id >= decision.id {
                    return Err(corrupt(offset, "record ids are not strictly increasing"));
                }
                if decision.id > allocated_high {
                    return Err(corrupt(
                        offset,
                        "record id exceeds the durable allocation high watermark",
                    ));
                }
                if pending.len() >= MAX_PENDING_DECISIONS {
                    return Err(corrupt(
                        offset,
                        format!("pending decision count exceeds limit {MAX_PENDING_DECISIONS}"),
                    ));
                }
                pending.push(decision);
                offset = next_offset;
            }
            Err(FrameDecodeError::Incomplete(reason)) => {
                if has_valid_frame_after(bytes, offset.saturating_add(1)) {
                    return Err(corrupt(offset, reason));
                }
                return Ok((base_id, allocated_high, pending, offset));
            }
            Err(FrameDecodeError::Corrupt(reason)) => return Err(corrupt(offset, reason)),
        }
    }
    Ok((base_id, allocated_high, pending, offset))
}

#[derive(Debug)]
enum FrameDecodeError {
    Incomplete(String),
    Corrupt(String),
}

fn decode_frame_at(
    bytes: &[u8],
    offset: usize,
) -> Result<(WorldChunkDecision, usize), FrameDecodeError> {
    let remaining = &bytes[offset..];
    if remaining.len() < FRAME_MAGIC.len() {
        return if FRAME_MAGIC.starts_with(remaining) {
            Err(FrameDecodeError::Incomplete(
                "incomplete frame magic".to_owned(),
            ))
        } else {
            Err(FrameDecodeError::Corrupt("invalid frame magic".to_owned()))
        };
    }
    let prefix_end = offset
        .checked_add(FRAME_PREFIX_BYTES)
        .ok_or_else(|| FrameDecodeError::Corrupt("frame prefix offset overflow".to_owned()))?;
    let prefix = bytes
        .get(offset..prefix_end)
        .ok_or_else(|| FrameDecodeError::Incomplete("incomplete frame prefix".to_owned()))?;
    if &prefix[..FRAME_MAGIC.len()] != FRAME_MAGIC {
        return Err(FrameDecodeError::Corrupt("invalid frame magic".to_owned()));
    }
    let payload_len = u64::from_le_bytes(
        prefix[FRAME_MAGIC.len()..]
            .try_into()
            .expect("frame length slice has fixed length"),
    );
    if payload_len > MAX_FRAME_BYTES {
        return Err(FrameDecodeError::Corrupt(format!(
            "frame length {payload_len} exceeds limit"
        )));
    }
    let payload_len = usize::try_from(payload_len).map_err(|_| {
        FrameDecodeError::Corrupt("frame length does not fit this platform".to_owned())
    })?;
    let payload_end = prefix_end
        .checked_add(payload_len)
        .ok_or_else(|| FrameDecodeError::Corrupt("frame payload offset overflow".to_owned()))?;
    let frame_end = payload_end
        .checked_add(FRAME_SUFFIX_BYTES)
        .ok_or_else(|| FrameDecodeError::Corrupt("frame checksum offset overflow".to_owned()))?;
    let payload = bytes
        .get(prefix_end..payload_end)
        .ok_or_else(|| FrameDecodeError::Incomplete("incomplete frame payload".to_owned()))?;
    let stored_crc = u32::from_le_bytes(
        bytes
            .get(payload_end..frame_end)
            .ok_or_else(|| FrameDecodeError::Incomplete("incomplete frame checksum".to_owned()))?
            .try_into()
            .expect("checksum slice has fixed length"),
    );
    let actual_crc = crc32fast::hash(payload);
    if stored_crc != actual_crc {
        return Err(FrameDecodeError::Corrupt(format!(
            "frame checksum mismatch: stored {stored_crc:#010x}, computed {actual_crc:#010x}"
        )));
    }
    let decision = decode_decision_payload(payload).map_err(FrameDecodeError::Corrupt)?;
    Ok((decision, frame_end))
}

fn has_valid_frame_after(bytes: &[u8], start: usize) -> bool {
    if start >= bytes.len() {
        return false;
    }
    bytes[start..]
        .windows(FRAME_MAGIC.len())
        .enumerate()
        .filter(|(_, candidate)| *candidate == FRAME_MAGIC)
        .any(|(relative, _)| decode_frame_at(bytes, start + relative).is_ok())
}

fn encode_journal(
    base_id: u64,
    allocated_high: u64,
    decisions: &[WorldChunkDecision],
) -> Result<Vec<u8>, WorldChunkJournalError> {
    let encoded_len = encoded_decisions_len(decisions, JOURNAL_HEADER_BYTES)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(encoded_len)
        .map_err(|_| WorldChunkJournalError::AllocationFailed(encoded_len))?;
    write_header(&mut bytes, base_id, allocated_high).map_err(|source| {
        WorldChunkJournalError::Io {
            operation: "encode header",
            path: PathBuf::from(JOURNAL_FILE),
            source,
        }
    })?;
    for decision in decisions {
        bytes.extend_from_slice(&encode_frame(decision)?);
    }
    Ok(bytes)
}

fn write_header(writer: &mut impl Write, base_id: u64, allocated_high: u64) -> std::io::Result<()> {
    writer.write_all(JOURNAL_MAGIC)?;
    writer.write_all(&JOURNAL_VERSION.to_le_bytes())?;
    writer.write_all(&base_id.to_le_bytes())?;
    writer.write_all(&allocated_high.to_le_bytes())
}

fn encode_frame(decision: &WorldChunkDecision) -> Result<Vec<u8>, WorldChunkJournalError> {
    let frame_len = encoded_frame_len(decision)?;
    let payload = encode_decision_payload(decision)?;
    let payload_len = u64::try_from(payload.len()).expect("usize always fits u64");
    let mut frame = Vec::new();
    frame
        .try_reserve_exact(frame_len)
        .map_err(|_| WorldChunkJournalError::AllocationFailed(frame_len))?;
    frame.extend_from_slice(FRAME_MAGIC);
    frame.extend_from_slice(&payload_len.to_le_bytes());
    frame.extend_from_slice(&payload);
    frame.extend_from_slice(&crc32fast::hash(&payload).to_le_bytes());
    Ok(frame)
}

fn encoded_frame_len(decision: &WorldChunkDecision) -> Result<usize, WorldChunkJournalError> {
    FRAME_PREFIX_BYTES
        .checked_add(decision_payload_len(decision)?)
        .and_then(|len| len.checked_add(FRAME_SUFFIX_BYTES))
        .ok_or(WorldChunkJournalError::FrameTooLarge(u64::MAX))
}

fn encoded_decisions_len(
    decisions: &[WorldChunkDecision],
    initial: usize,
) -> Result<usize, WorldChunkJournalError> {
    let mut total = checked_journal_len(0, initial)?;
    for decision in decisions {
        total = checked_journal_len(total, encoded_frame_len(decision)?)?;
    }
    Ok(total)
}

fn checked_journal_len(current: usize, additional: usize) -> Result<usize, WorldChunkJournalError> {
    let total = current
        .checked_add(additional)
        .ok_or(WorldChunkJournalError::JournalTooLarge(u64::MAX))?;
    let total_u64 = u64::try_from(total).expect("usize always fits u64");
    if total_u64 > MAX_JOURNAL_FILE_BYTES {
        return Err(WorldChunkJournalError::JournalTooLarge(total_u64));
    }
    Ok(total)
}

fn encode_decision_payload(
    decision: &WorldChunkDecision,
) -> Result<Vec<u8>, WorldChunkJournalError> {
    let payload_len = decision_payload_len(decision)?;
    let image_count = u32::try_from(decision.images.len())
        .map_err(|_| WorldChunkJournalError::TooManyImages(decision.images.len()))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(payload_len)
        .map_err(|_| WorldChunkJournalError::AllocationFailed(payload_len))?;
    bytes.extend_from_slice(&decision.id.to_le_bytes());
    bytes.extend_from_slice(&decision.current_tick.to_le_bytes());
    bytes.extend_from_slice(&image_count.to_le_bytes());
    for image in &decision.images {
        let nbt_len =
            u32::try_from(image.nbt.len()).map_err(|_| WorldChunkJournalError::ImageTooLarge {
                position: image.position,
                bytes: image.nbt.len(),
            })?;
        bytes.extend_from_slice(&image.position.x.to_le_bytes());
        bytes.extend_from_slice(&image.position.z.to_le_bytes());
        bytes.extend_from_slice(&nbt_len.to_le_bytes());
        bytes.extend_from_slice(&image.nbt);
    }
    Ok(bytes)
}

fn decision_payload_len(decision: &WorldChunkDecision) -> Result<usize, WorldChunkJournalError> {
    if decision.images.len() > MAX_IMAGES_PER_DECISION {
        return Err(WorldChunkJournalError::TooManyImages(decision.images.len()));
    }
    let mut payload_len = DECISION_FIXED_BYTES;
    for image in &decision.images {
        if image.nbt.len() > MAX_IMAGE_NBT_BYTES {
            return Err(WorldChunkJournalError::ImageTooLarge {
                position: image.position,
                bytes: image.nbt.len(),
            });
        }
        payload_len = payload_len
            .checked_add(IMAGE_PREFIX_BYTES)
            .and_then(|len| len.checked_add(image.nbt.len()))
            .ok_or(WorldChunkJournalError::FrameTooLarge(u64::MAX))?;
    }
    let payload_len_u64 = u64::try_from(payload_len).expect("usize always fits u64");
    if payload_len_u64 > MAX_FRAME_BYTES {
        return Err(WorldChunkJournalError::FrameTooLarge(payload_len_u64));
    }
    Ok(payload_len)
}

fn decode_decision_payload(payload: &[u8]) -> Result<WorldChunkDecision, String> {
    let mut reader = PayloadReader::new(payload);
    let id = reader.u64("record id")?;
    if id == 0 {
        return Err("record id must be non-zero".to_owned());
    }
    let current_tick = reader.u64("current tick")?;
    let image_count = usize::try_from(reader.u32("image count")?)
        .map_err(|_| "image count does not fit this platform".to_owned())?;
    if image_count > MAX_IMAGES_PER_DECISION {
        return Err(format!(
            "image count {image_count} exceeds limit {MAX_IMAGES_PER_DECISION}"
        ));
    }
    let max_by_remaining = reader.remaining().len() / IMAGE_PREFIX_BYTES;
    if image_count > max_by_remaining {
        return Err(format!(
            "image count {image_count} exceeds payload feasibility {max_by_remaining}"
        ));
    }
    let mut images = Vec::with_capacity(image_count);
    for _ in 0..image_count {
        let position = ChunkPos {
            x: reader.i32("chunk x")?,
            z: reader.i32("chunk z")?,
        };
        let nbt_len = usize::try_from(reader.u32("NBT length")?)
            .map_err(|_| "NBT length does not fit this platform".to_owned())?;
        if nbt_len > MAX_IMAGE_NBT_BYTES {
            return Err(format!(
                "NBT length {nbt_len} exceeds limit {MAX_IMAGE_NBT_BYTES}"
            ));
        }
        let nbt = reader.bytes(nbt_len, "NBT payload")?.to_vec();
        images.push(WorldChunkImage { position, nbt });
    }
    if !reader.remaining().is_empty() {
        return Err("record payload has trailing bytes".to_owned());
    }
    Ok(WorldChunkDecision {
        id,
        current_tick,
        images,
    })
}

struct PayloadReader<'a> {
    remaining: &'a [u8],
}

impl<'a> PayloadReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { remaining: bytes }
    }

    fn bytes(&mut self, len: usize, field: &str) -> Result<&'a [u8], String> {
        if self.remaining.len() < len {
            return Err(format!("incomplete {field}"));
        }
        let (value, remaining) = self.remaining.split_at(len);
        self.remaining = remaining;
        Ok(value)
    }

    fn u64(&mut self, field: &str) -> Result<u64, String> {
        Ok(u64::from_le_bytes(
            self.bytes(size_of::<u64>(), field)?
                .try_into()
                .expect("u64 field has fixed length"),
        ))
    }

    fn u32(&mut self, field: &str) -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            self.bytes(size_of::<u32>(), field)?
                .try_into()
                .expect("u32 field has fixed length"),
        ))
    }

    fn i32(&mut self, field: &str) -> Result<i32, String> {
        Ok(i32::from_le_bytes(
            self.bytes(size_of::<i32>(), field)?
                .try_into()
                .expect("i32 field has fixed length"),
        ))
    }

    fn remaining(&self) -> &[u8] {
        self.remaining
    }
}

fn decode_image(
    decision_id: u64,
    image: &WorldChunkImage,
    blocks: &BlockRegistry,
    items: &ItemRegistry,
) -> Result<Chunk, WorldChunkJournalError> {
    let mut bytes = image.nbt.as_slice();
    let (_, root) =
        mc_nbt::read_named(&mut bytes).map_err(|source| WorldChunkJournalError::DecodeNbt {
            decision_id,
            position: image.position,
            source,
        })?;
    if !bytes.is_empty() {
        return Err(WorldChunkJournalError::TrailingNbt {
            decision_id,
            position: image.position,
        });
    }
    let chunk = chunk_from_nbt_with_items(&root, blocks, Some(items)).map_err(|source| {
        WorldChunkJournalError::DecodeChunk {
            decision_id,
            position: image.position,
            source,
        }
    })?;
    if chunk.pos != image.position {
        return Err(WorldChunkJournalError::PositionMismatch {
            decision_id,
            declared: image.position,
            stored: chunk.pos,
        });
    }
    Ok(chunk)
}

fn corrupt(offset: usize, reason: impl Into<String>) -> WorldChunkJournalError {
    WorldChunkJournalError::Corrupt {
        offset: offset as u64,
        reason: reason.into(),
    }
}

#[cfg(test)]
#[path = "world_journal_tests.rs"]
mod tests;
