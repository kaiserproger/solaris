use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mc_data::items::ItemRegistry;
use mc_world::anvil::{chunk_from_nbt_with_items, chunk_to_payload_with_items_at_tick};
use mc_world::{BlockRegistry, Chunk, ChunkPos, ChunkSnapshot};
use thiserror::Error;

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
const WRITER_QUEUE_CAPACITY: usize = 16;

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

type JournalRecovery = (u64, u64, Vec<WorldChunkDecision>);
type JournalInitialization = Result<JournalRecovery, WorldChunkJournalError>;

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
    #[error("world chunk journal append outcome is unknown at {path}: {source}")]
    AppendOutcomeUnknown {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("world chunk journal append completion was lost; its outcome is unknown")]
    AppendCompletionLost,
    #[error("world chunk journal checkpoint outcome is unknown at {path}: {source}")]
    CheckpointOutcomeUnknown {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("world chunk journal checkpoint completion was lost; its outcome is unknown")]
    CheckpointCompletionLost,
    #[error("world chunk journal reservation outcome is unknown at {path}: {source}")]
    ReservationOutcomeUnknown {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("world chunk journal reservation completion was lost; its outcome is unknown")]
    ReservationCompletionLost,
    #[error("world chunk journal is poisoned by an earlier append with unknown outcome")]
    PoisonedOutcomeUnknown,
}

impl WorldChunkJournalError {
    #[must_use]
    #[allow(
        dead_code,
        reason = "journal callers must distinguish unknown append outcomes"
    )]
    pub(crate) fn outcome_unknown(&self) -> bool {
        matches!(
            self,
            Self::AppendOutcomeUnknown { .. }
                | Self::AppendCompletionLost
                | Self::CheckpointOutcomeUnknown { .. }
                | Self::CheckpointCompletionLost
                | Self::ReservationOutcomeUnknown { .. }
                | Self::ReservationCompletionLost
                | Self::PoisonedOutcomeUnknown
        )
    }
}

#[derive(Clone)]
pub(crate) struct WorldChunkJournal {
    shared: Arc<JournalShared>,
}

struct JournalShared {
    blocks: Arc<BlockRegistry>,
    items: Arc<ItemRegistry>,
    state: Mutex<JournalState>,
    append_advanced: tokio::sync::Notify,
}

struct JournalState {
    path: PathBuf,
    checkpoint_base: u64,
    pending: Vec<WorldChunkDecision>,
    next_id: u64,
    next_append_id: u64,
    poisoned: bool,
    requests: std::sync::mpsc::SyncSender<WriterRequest>,
    worker: Option<std::thread::JoinHandle<()>>,
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

impl Drop for JournalState {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            shutdown_writer(&self.requests, worker);
        }
    }
}

fn shutdown_writer(
    requests: &std::sync::mpsc::SyncSender<WriterRequest>,
    worker: std::thread::JoinHandle<()>,
) {
    let (reply, completion) = std::sync::mpsc::channel();
    let _ = requests.send(WriterRequest::Shutdown { reply });
    let _ = completion.recv();
    let _ = worker.join();
}

impl WorldChunkJournal {
    pub(crate) fn open(
        world_root: &Path,
        blocks: Arc<BlockRegistry>,
        items: Arc<ItemRegistry>,
    ) -> Result<(Self, Vec<WorldChunkDecision>), WorldChunkJournalError> {
        let path = world_root.join(SOLARIS_DIRECTORY).join(JOURNAL_FILE);
        let (requests, receiver) = std::sync::mpsc::sync_channel(WRITER_QUEUE_CAPACITY);
        let (initialized, initialization) = std::sync::mpsc::sync_channel(1);
        let writer_path = path.clone();
        let worker = std::thread::Builder::new()
            .name("solaris-world-chunk-journal".to_owned())
            .spawn(move || run_writer(writer_path, receiver, initialized))
            .map_err(|source| WorldChunkJournalError::Io {
                operation: "spawn writer",
                path: path.clone(),
                source,
            })?;
        let (base_id, allocated_high, pending) = match initialization.recv() {
            Ok(Ok(recovered)) => recovered,
            Ok(Err(error)) => {
                let _ = worker.join();
                return Err(error);
            }
            Err(_) => {
                let _ = worker.join();
                return Err(WorldChunkJournalError::Io {
                    operation: "initialize writer",
                    path: path.clone(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "world chunk journal writer exited before initialization",
                    ),
                });
            }
        };
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
                shutdown_writer(&requests, worker);
                return Err(WorldChunkJournalError::RecordIdExhausted);
            }
        };
        let journal = Self {
            shared: Arc::new(JournalShared {
                blocks,
                items,
                append_advanced: tokio::sync::Notify::new(),
                state: Mutex::new(JournalState {
                    path,
                    checkpoint_base: base_id,
                    pending: pending.clone(),
                    next_id,
                    next_append_id,
                    poisoned: false,
                    requests,
                    worker: Some(worker),
                }),
            }),
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

        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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

        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
                let state = self
                    .shared
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
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
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pending
            .last()
            .map(WorldChunkDecision::id)
    }

    #[cfg(test)]
    pub(crate) fn pending_decisions_for_test(&self) -> Vec<WorldChunkDecision> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pending
            .clone()
    }

    pub(crate) fn checkpoint_through(&self, watermark: u64) -> Result<(), WorldChunkJournalError> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        Self {
            shared: Arc::new(JournalShared {
                blocks,
                items,
                append_advanced: tokio::sync::Notify::new(),
                state: Mutex::new(JournalState {
                    path,
                    checkpoint_base: 0,
                    pending: Vec::new(),
                    next_id: 0,
                    next_append_id: 1,
                    poisoned: false,
                    requests,
                    worker: Some(worker),
                }),
            }),
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
    let replacement = encode_journal(state.checkpoint_base, last, &state.pending)?;
    let (reply, completion) = std::sync::mpsc::channel();
    if state
        .requests
        .send(WriterRequest::Replace { replacement, reply })
        .is_err()
    {
        state.poisoned = true;
        return Err(WorldChunkJournalError::WriterClosed {
            operation: "reserve decision ids",
        });
    }
    match completion.recv() {
        Ok(Ok(())) => {}
        Ok(Err(WriterFailure::JournalTooLarge(bytes))) => {
            return Err(WorldChunkJournalError::JournalTooLarge(bytes));
        }
        Ok(Err(WriterFailure::Io(source))) => {
            state.poisoned = true;
            return Err(WorldChunkJournalError::ReservationOutcomeUnknown {
                path: state.path.clone(),
                source,
            });
        }
        Err(_) => {
            state.poisoned = true;
            return Err(WorldChunkJournalError::ReservationCompletionLost);
        }
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
    let (reply, completion) = std::sync::mpsc::channel();
    if state
        .requests
        .send(WriterRequest::Append { bytes, reply })
        .is_err()
    {
        state.poisoned = true;
        return Err(WorldChunkJournalError::WriterClosed {
            operation: "append",
        });
    }
    match completion.recv() {
        Ok(Ok(())) => {
            state.pending.extend(decisions);
            Ok(())
        }
        Ok(Err(WriterFailure::JournalTooLarge(bytes))) => {
            state.poisoned = true;
            Err(WorldChunkJournalError::JournalTooLarge(bytes))
        }
        Ok(Err(WriterFailure::Io(source))) => {
            state.poisoned = true;
            Err(WorldChunkJournalError::AppendOutcomeUnknown {
                path: state.path.clone(),
                source,
            })
        }
        Err(_) => {
            state.poisoned = true;
            Err(WorldChunkJournalError::AppendCompletionLost)
        }
    }
}

pub(super) enum WriterRequest {
    Append {
        bytes: Vec<u8>,
        reply: std::sync::mpsc::Sender<Result<(), WriterFailure>>,
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
    path: PathBuf,
    receiver: std::sync::mpsc::Receiver<WriterRequest>,
    initialized: std::sync::mpsc::SyncSender<JournalInitialization>,
) {
    let directory = path.parent().expect("journal path has a parent");
    if let Err(source) = ensure_journal_directory(directory) {
        let _ = initialized.send(Err(WorldChunkJournalError::Io {
            operation: "create journal directory",
            path: directory.to_path_buf(),
            source,
        }));
        return;
    }
    let lock_path = directory.join(JOURNAL_LOCK_FILE);
    let lock_file = match OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
    {
        Ok(file) => file,
        Err(source) => {
            let _ = initialized.send(Err(WorldChunkJournalError::Io {
                operation: "open journal lease",
                path: lock_path,
                source,
            }));
            return;
        }
    };
    let mut lease = fd_lock::RwLock::new(lock_file);
    let guard = match lease.try_write() {
        Ok(guard) => guard,
        Err(source) => {
            let _ = initialized.send(Err(WorldChunkJournalError::Io {
                operation: "acquire journal lease",
                path: lock_path,
                source,
            }));
            return;
        }
    };
    let recovered = match read_and_repair_journal(&path) {
        Ok(recovered) => recovered,
        Err(error) => {
            let _ = initialized.send(Err(error));
            return;
        }
    };
    if initialized.send(Ok(recovered)).is_err() {
        return;
    }

    while let Ok(request) = receiver.recv() {
        match request {
            WriterRequest::Append { bytes, reply } => {
                let _ = reply.send(append_frames(&path, &bytes));
            }
            WriterRequest::Replace { replacement, reply } => {
                let _ = reply.send(replace_journal(&path, &replacement));
            }
            WriterRequest::Shutdown { reply } => {
                let _ = reply.send(());
                break;
            }
        }
    }
    drop(guard);
}

fn append_frames(path: &Path, bytes: &[u8]) -> Result<(), WriterFailure> {
    let directory = path.parent().expect("journal path has a parent");
    ensure_journal_directory(directory)?;
    let existed = path.is_file();
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
    file.flush()?;
    file.sync_all()?;
    if !existed {
        sync_directory(directory)?;
    }
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
        std::fs::rename(&temporary, path)?;
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
mod tests {
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

    fn registries() -> (Arc<BlockRegistry>, Arc<ItemRegistry>) {
        (
            Arc::new(
                BlockRegistry::from_report(&solaris_required_blocks_report())
                    .expect("embedded block registry"),
            ),
            Arc::new(solaris_required_items()),
        )
    }

    fn snapshot(blocks: &BlockRegistry, position: ChunkPos, current_tick: u64) -> ChunkSnapshot {
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
            block: Some(vec![0x21; mc_world::LIGHT_LAYER_BYTES]),
            sky: Some(vec![0x54; mc_world::LIGHT_LAYER_BYTES]),
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

    fn snapshot_with_lsn(
        blocks: &BlockRegistry,
        position: ChunkPos,
        current_tick: u64,
        lsn: u64,
    ) -> ChunkSnapshot {
        let mut snapshot = snapshot(blocks, position, current_tick);
        Arc::get_mut(&mut snapshot).unwrap().extras.push((
            "SolarisJournalLsn".to_owned(),
            mc_nbt::Tag::Long(i64::try_from(lsn).unwrap()),
        ));
        snapshot
    }

    #[test]
    fn round_trips_full_chunk_snapshot_and_restart_relative_tick() {
        let temp = tempfile::tempdir().unwrap();
        let (blocks, items) = registries();
        let position = ChunkPos { x: -3, z: 9 };
        let (journal, pending) =
            WorldChunkJournal::open(temp.path(), Arc::clone(&blocks), Arc::clone(&items)).unwrap();
        assert!(pending.is_empty());

        let id = journal
            .record_snapshots(120, vec![snapshot(&blocks, position, 120)])
            .unwrap();
        assert_eq!(id, 1);
        assert_eq!(journal.watermark(), Some(1));
        drop(journal);

        let (journal, pending) =
            WorldChunkJournal::open(temp.path(), Arc::clone(&blocks), Arc::clone(&items)).unwrap();
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
            chunks[0].section_lights[8].block.as_deref(),
            Some(&vec![0x21; mc_world::LIGHT_LAYER_BYTES][..])
        );
        assert_eq!(chunks[0].scheduled_block_ticks()[0].trigger_tick, 17);
        assert!(chunks[0].extras.iter().any(|(name, value)| {
            name == "SolarisJournalTest" && value == &mc_nbt::Tag::Long(91)
        }));
    }

    #[test]
    fn appends_decision_prefixes_and_keeps_each_snapshot_group_together() {
        let temp = tempfile::tempdir().unwrap();
        let (blocks, items) = registries();
        let (journal, _) =
            WorldChunkJournal::open(temp.path(), Arc::clone(&blocks), Arc::clone(&items)).unwrap();

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

        let (_, pending) = WorldChunkJournal::open(temp.path(), blocks, items).unwrap();
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
            WorldChunkJournal::open(temp.path(), Arc::clone(&blocks), Arc::clone(&items)).unwrap();
        journal
            .record_snapshots(10, vec![snapshot(&blocks, ChunkPos { x: 0, z: 0 }, 10)])
            .unwrap();
        drop(journal);
        let prefix_len = std::fs::metadata(&path).unwrap().len();

        let (journal, _) =
            WorldChunkJournal::open(temp.path(), Arc::clone(&blocks), Arc::clone(&items)).unwrap();
        journal
            .record_snapshots(20, vec![snapshot(&blocks, ChunkPos { x: 1, z: 0 }, 20)])
            .unwrap();
        drop(journal);
        let damaged_len = std::fs::metadata(&path).unwrap().len() - 3;
        let file = OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(damaged_len).unwrap();
        file.sync_all().unwrap();

        let (_, pending) = WorldChunkJournal::open(temp.path(), blocks, items).unwrap();
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
            WorldChunkJournal::open(temp.path(), Arc::clone(&blocks), Arc::clone(&items)).unwrap();
        journal
            .record_snapshots(10, vec![snapshot(&blocks, ChunkPos { x: 0, z: 0 }, 10)])
            .unwrap();
        drop(journal);
        let (journal, _) =
            WorldChunkJournal::open(temp.path(), Arc::clone(&blocks), Arc::clone(&items)).unwrap();
        journal
            .record_snapshots(20, vec![snapshot(&blocks, ChunkPos { x: 1, z: 0 }, 20)])
            .unwrap();
        drop(journal);

        let damaged_len = std::fs::metadata(&path).unwrap().len();
        flip_byte(&path, std::fs::metadata(&path).unwrap().len() - 1);
        let error = WorldChunkJournal::open(temp.path(), blocks, items).unwrap_err();
        assert!(matches!(error, WorldChunkJournalError::Corrupt { .. }));
        assert_eq!(std::fs::metadata(path).unwrap().len(), damaged_len);
    }

    #[test]
    fn rejects_corruption_before_a_valid_later_frame() {
        let temp = tempfile::tempdir().unwrap();
        let (blocks, items) = registries();
        let path = temp.path().join("solaris/world-chunk-journal.bin");
        let (journal, _) =
            WorldChunkJournal::open(temp.path(), Arc::clone(&blocks), Arc::clone(&items)).unwrap();
        journal
            .record_snapshots(10, vec![snapshot(&blocks, ChunkPos { x: 0, z: 0 }, 10)])
            .unwrap();
        journal
            .record_snapshots(20, vec![snapshot(&blocks, ChunkPos { x: 1, z: 0 }, 20)])
            .unwrap();
        drop(journal);

        let first_payload_byte = (JOURNAL_HEADER_BYTES + FRAME_PREFIX_BYTES) as u64;
        flip_byte(&path, first_payload_byte);
        let error = WorldChunkJournal::open(temp.path(), blocks, items).unwrap_err();
        assert!(matches!(error, WorldChunkJournalError::Corrupt { .. }));
    }

    #[test]
    fn rejects_impossible_image_count_before_allocation() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&1_u64.to_le_bytes());
        payload.extend_from_slice(&0_u64.to_le_bytes());
        payload.extend_from_slice(&u32::MAX.to_le_bytes());

        let error = decode_decision_payload(&payload).unwrap_err();
        assert!(error.contains("image count"), "{error}");
        assert!(error.contains("exceeds limit"), "{error}");
    }

    #[test]
    fn rejects_infeasible_image_count_before_allocation() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&1_u64.to_le_bytes());
        payload.extend_from_slice(&0_u64.to_le_bytes());
        payload.extend_from_slice(&1_u32.to_le_bytes());

        let error = decode_decision_payload(&payload).unwrap_err();
        assert!(error.contains("payload feasibility"), "{error}");
    }

    #[test]
    fn rejects_oversized_image_nbt_before_copy() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&1_u64.to_le_bytes());
        payload.extend_from_slice(&0_u64.to_le_bytes());
        payload.extend_from_slice(&1_u32.to_le_bytes());
        payload.extend_from_slice(&0_i32.to_le_bytes());
        payload.extend_from_slice(&0_i32.to_le_bytes());
        payload.extend_from_slice(
            &u32::try_from(MAX_IMAGE_NBT_BYTES + 1)
                .expect("test NBT limit fits u32")
                .to_le_bytes(),
        );

        let error = decode_decision_payload(&payload).unwrap_err();
        assert!(error.contains("NBT length"), "{error}");
        assert!(error.contains("exceeds limit"), "{error}");
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

        let error = WorldChunkJournal::open(temp.path(), blocks, items).unwrap_err();
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
            WorldChunkJournal::open(temp.path(), Arc::clone(&blocks), Arc::clone(&items)).unwrap();

        let error = WorldChunkJournal::open(temp.path(), blocks, items).unwrap_err();
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
        let (requests, receiver) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let WriterRequest::Replace { reply, .. } = receiver.recv().unwrap() else {
                panic!("expected reservation request");
            };
            reply.send(Ok(())).unwrap();
            let WriterRequest::Append { reply, .. } = receiver.recv().unwrap() else {
                panic!("expected append request");
            };
            reply
                .send(Err(WriterFailure::JournalTooLarge(
                    MAX_JOURNAL_FILE_BYTES + 1,
                )))
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

        let append = tokio::task::spawn_blocking({
            let journal = journal.clone();
            move || journal.record_reserved_snapshot_groups(10, vec![(1, Vec::new())])
        })
        .await
        .unwrap();
        assert!(matches!(
            append,
            Err(WorldChunkJournalError::JournalTooLarge(bytes))
                if bytes == MAX_JOURNAL_FILE_BYTES + 1
        ));
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
            WorldChunkJournal::open(temp.path(), Arc::clone(&blocks), Arc::clone(&items)).unwrap();
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
            WorldChunkJournal::open(temp.path(), Arc::clone(&blocks), Arc::clone(&items)).unwrap();
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
            WorldChunkJournal::open(temp.path(), Arc::clone(&blocks), Arc::clone(&items)).unwrap();
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
            WorldChunkJournal::open(temp.path(), Arc::clone(&blocks), Arc::clone(&items)).unwrap();
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

        let (_, pending) = WorldChunkJournal::open(temp.path(), blocks, items).unwrap();
        assert_eq!(
            pending
                .iter()
                .map(WorldChunkDecision::id)
                .collect::<Vec<_>>(),
            vec![2]
        );
    }

    #[test]
    fn reserved_snapshot_groups_use_one_ordered_append() {
        let temp = tempfile::tempdir().unwrap();
        let (blocks, items) = registries();
        let (requests, receiver) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let WriterRequest::Replace { reply, .. } = receiver.recv().unwrap() else {
                panic!("expected reservation request");
            };
            reply.send(Ok(())).unwrap();
            let WriterRequest::Append { bytes, reply } = receiver.recv().unwrap() else {
                panic!("expected append request");
            };
            let (first, offset) = decode_frame_at(&bytes, 0).unwrap();
            let (second, end) = decode_frame_at(&bytes, offset).unwrap();
            assert_eq!((first.id(), second.id()), (1, 2));
            assert_eq!(end, bytes.len());
            reply.send(Ok(())).unwrap();
            let WriterRequest::Shutdown { reply } = receiver.recv().unwrap() else {
                panic!("expected shutdown request");
            };
            reply.send(()).unwrap();
        });
        let journal = WorldChunkJournal::from_parts_for_test(
            temp.path().join("solaris/world-chunk-journal.bin"),
            Arc::clone(&blocks),
            items,
            requests,
            worker,
        );

        assert_eq!(journal.reserve_decision_ids(2).unwrap(), vec![1, 2]);
        journal
            .record_reserved_snapshot_groups(
                10,
                vec![
                    (
                        1,
                        vec![snapshot_with_lsn(&blocks, ChunkPos { x: 0, z: 0 }, 10, 1)],
                    ),
                    (
                        2,
                        vec![snapshot_with_lsn(&blocks, ChunkPos { x: 1, z: 0 }, 10, 2)],
                    ),
                ],
            )
            .unwrap();
        assert_eq!(journal.watermark(), Some(2));
    }

    #[test]
    fn reserved_ids_are_not_reused_after_restart_without_an_append() {
        let temp = tempfile::tempdir().unwrap();
        let (blocks, items) = registries();
        let (journal, _) =
            WorldChunkJournal::open(temp.path(), Arc::clone(&blocks), Arc::clone(&items)).unwrap();
        assert_eq!(journal.reserve_decision_ids(2).unwrap(), vec![1, 2]);
        drop(journal);

        let (journal, pending) =
            WorldChunkJournal::open(temp.path(), Arc::clone(&blocks), items).unwrap();
        assert!(pending.is_empty());
        assert_eq!(journal.reserve_decision_ids(1).unwrap(), vec![3]);
    }

    #[test]
    fn reserved_decisions_can_append_in_ordered_prefixes() {
        let temp = tempfile::tempdir().unwrap();
        let (blocks, items) = registries();
        let (journal, pending) = WorldChunkJournal::open(temp.path(), blocks, items).unwrap();
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
            WorldChunkJournal::open(temp.path(), blocks.clone(), items.clone()).unwrap();
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

        let (_reopened, pending) = WorldChunkJournal::open(temp.path(), blocks, items).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id(), decision_id);
        assert!(pending[0].images().is_empty());
    }

    #[tokio::test]
    async fn later_reserved_decision_waits_for_append_turn() {
        let temp = tempfile::tempdir().unwrap();
        let (blocks, items) = registries();
        let (journal, pending) = WorldChunkJournal::open(temp.path(), blocks, items).unwrap();
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
            let WriterRequest::Replace { reply, .. } = receiver.recv().unwrap() else {
                panic!("expected reservation request");
            };
            reply.send(Ok(())).unwrap();
            let WriterRequest::Append { reply, .. } = receiver.recv().unwrap() else {
                panic!("expected append request");
            };
            reply.send(Ok(())).unwrap();
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
        let worker = std::thread::spawn(move || {
            let WriterRequest::Replace { reply, .. } = receiver.recv().unwrap() else {
                panic!("expected reservation request");
            };
            reply.send(Ok(())).unwrap();
        });
        let journal = WorldChunkJournal::from_parts_for_test(
            temp.path().join("solaris/world-chunk-journal.bin"),
            blocks,
            items,
            requests,
            worker,
        );
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
            let WriterRequest::Replace { reply, .. } = receiver.recv().unwrap() else {
                panic!("expected reservation request");
            };
            reply.send(Ok(())).unwrap();
            let WriterRequest::Append { reply, .. } = receiver.recv().unwrap() else {
                panic!("expected append request");
            };
            reply.send(Ok(())).unwrap();
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
            let WriterRequest::Replace { replacement, reply } = receiver.recv().unwrap() else {
                panic!("expected reservation request");
            };
            std::fs::write(&writer_path, replacement).unwrap();
            reply.send(Ok(())).unwrap();
            let WriterRequest::Append { bytes, reply } = receiver.recv().unwrap() else {
                panic!("expected append request");
            };
            let mut file = OpenOptions::new().append(true).open(&writer_path).unwrap();
            file.write_all(&bytes).unwrap();
            file.sync_all().unwrap();
            reply
                .send(Err(WriterFailure::Io(std::io::Error::other("injected"))))
                .unwrap();
        });
        let journal = WorldChunkJournal::from_parts_for_test(
            path,
            blocks.clone(),
            items.clone(),
            requests,
            worker,
        );

        let error = journal
            .record_snapshots(10, vec![snapshot(&blocks, ChunkPos { x: 0, z: 0 }, 10)])
            .expect_err("injected append failure");
        assert!(error.outcome_unknown());
        assert_eq!(journal.watermark(), None);
        drop(journal);

        let (_reopened, pending) = WorldChunkJournal::open(temp.path(), blocks, items).unwrap();
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
}
