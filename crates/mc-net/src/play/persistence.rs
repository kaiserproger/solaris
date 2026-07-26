use std::collections::BTreeMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use flate2::Compression as GzipCompression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use mc_entity::{
    EntityStore, RegionPhase, RegionalCommitDecision, RegionalDecisionJournal,
    RegionalDecisionJournalError,
};
use mc_nbt::{ListTag, Tag, tag_type};
use serde::Deserialize;
use thiserror::Error;

use super::*;

const PLAYERDATA_DIR: &str = "playerdata";
const SOLARIS_DIR: &str = "solaris";
const ENTITIES_FILE: &str = "entities.dat";
const ENTITY_FORMAT_VERSION_FIELD: &str = "SolarisEntityFormatVersion";
const ENTITY_FORMAT_VERSION: i32 = 3;
const ENTITY_LIFECYCLE_TICK_FIELD: &str = "SolarisEntityLifecycleTick";
const ENTITY_REGIONAL_SEQUENCE_FIELD: &str = "SolarisRegionalSequenceWatermark";
const ENTITY_LIFECYCLE_FIELD: &str = "SolarisLifecycle";
const ENTITY_ATTRIBUTES_FIELD: &str = "SolarisAttributes";
const ENTITY_RETAINED_STATE_FIELD: &str = "SolarisRetainedState";
const ENTITY_HEAD_YAW_FIELD: &str = "SolarisHeadYaw";
const ENTITY_GOAL_STATE_FIELD: &str = "SolarisGoalState";
const ENTITY_VEHICLE_STATE_FIELD: &str = "SolarisVehicleState";
const ENTITY_HORIZONTAL_POSITION_LIMIT_26_1_2: f64 = 3.0000512E7;
const ENTITY_VERTICAL_POSITION_LIMIT_26_1_2: f64 = 2.0E7;
const ENTITY_VELOCITY_LIMIT_26_1_2: f64 = 10.0;
const ENTITY_TICKS_PER_SECOND: f64 = 20.0;
const WORLD_FILE: &str = "world.dat";
const REGIONAL_DECISION_JOURNAL_FILE: &str = "entity-owner-journal.json";
const REGIONAL_DECISION_JOURNAL_HEADER: &[u8] = b"SOLARIS_ENTITY_OWNER_JOURNAL 3\n";
const REGIONAL_DECISION_JOURNAL_FRAME_HEADER_BYTES: usize = 8;
// One recovery set supports the documented 30k-entity workload. The byte caps leave room for
// rich entity snapshots while bounding crash recovery and writer memory independently.
const MAX_REGIONAL_DECISION_JOURNAL_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_REGIONAL_DECISION_FRAME_PAYLOAD_BYTES: u64 = 512 * 1024 * 1024;
const MAX_REGIONAL_DECISIONS_PER_FRAME: usize = 30_000;
const MAX_REGIONAL_ENTITY_MUTATIONS_PER_FRAME: usize = 30_000;
const DAMAGE_COMPONENT: &str = "minecraft:damage";
const CUSTOM_NAME_COMPONENT: &str = "minecraft:custom_name";
const ENCHANTMENTS_COMPONENT: &str = "minecraft:enchantments";
const CARRIED_ITEM_FIELD: &str = "SolarisCarriedItem";
const CRAFTING_TABLE_INPUT_FIELD: &str = "SolarisCraftingTableInput";
const ENCHANTING_TABLE_INPUT_FIELD: &str = "SolarisEnchantingTableInput";
static TMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub(crate) enum RegionalDecisionJournalOpenError {
    #[error("regional decision journal IO failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("regional decision journal JSON failed at {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("unsupported regional decision journal version {0}")]
    UnsupportedVersion(u32),
    #[error("regional decision journal framing failed at {path}: {reason}")]
    Framing { path: PathBuf, reason: &'static str },
    #[error("regional decision journal checksum failed at {path}")]
    Checksum { path: PathBuf },
    #[error("regional decision journal validation failed at {path}: {source}")]
    Validation {
        path: PathBuf,
        #[source]
        source: RegionalDecisionReplayError,
    },
}

#[derive(Debug, Error)]
pub(crate) enum RegionalDecisionReplayError {
    #[error("regional decision phases and sequence watermarks must be strictly increasing")]
    InvalidOrdering,
    #[error("regional decision violates upsert/removal invariants")]
    InvalidDecision,
    #[error("regional decision contains an invalid entity snapshot")]
    InvalidSnapshot,
    #[error("regional decision recovery exceeds the supported 30,000-decision boundary")]
    TooManyDecisions,
    #[error("regional decision recovery exceeds the supported 30,000-entity mutation boundary")]
    TooManyEntityMutations,
    #[error("duplicate restored entity UUID {0}")]
    DuplicateEntityUuid(uuid::Uuid),
}

impl From<RegionalDecisionReplayError> for std::io::Error {
    fn from(source: RegionalDecisionReplayError) -> Self {
        Self::new(std::io::ErrorKind::InvalidData, source)
    }
}

#[derive(Deserialize)]
struct EncodedRegionalCommitDecision {
    phase: RegionPhase,
    sequence_watermark: u64,
    lifecycle_epoch: u64,
    upserts: Vec<EntitySnapshot>,
    removed: Vec<EntityId>,
}

impl TryFrom<EncodedRegionalCommitDecision> for RegionalCommitDecision {
    type Error = RegionalDecisionReplayError;

    fn try_from(encoded: EncodedRegionalCommitDecision) -> Result<Self, Self::Error> {
        RegionalCommitDecision::from_parts_at_lifecycle_epoch(
            encoded.phase,
            encoded.sequence_watermark,
            encoded.lifecycle_epoch,
            encoded.upserts,
            encoded.removed,
        )
        .map_err(|_| RegionalDecisionReplayError::InvalidDecision)
    }
}

pub(crate) struct FileRegionalDecisionJournal {
    path: PathBuf,
    pending: Vec<RegionalCommitDecision>,
    needs_compaction: bool,
    requests: std::sync::mpsc::SyncSender<RegionalJournalWriteRequest>,
    worker: Option<std::thread::JoinHandle<()>>,
}

enum RegionalJournalWriteRequest {
    Append {
        decisions: Vec<RegionalCommitDecision>,
        reply: std::sync::mpsc::Sender<Result<(), RegionalDecisionJournalError>>,
    },
    Replace {
        pending: Vec<RegionalCommitDecision>,
        reply: std::sync::mpsc::Sender<Result<(), RegionalDecisionJournalError>>,
    },
    Shutdown {
        reply: std::sync::mpsc::Sender<()>,
    },
}

impl FileRegionalDecisionJournal {
    pub(crate) fn open(
        world_root: &Path,
    ) -> Result<(Self, Vec<RegionalCommitDecision>), RegionalDecisionJournalOpenError> {
        let path = world_root
            .join(SOLARIS_DIR)
            .join(REGIONAL_DECISION_JOURNAL_FILE);
        let pending = if path.is_file() {
            let bytes = read_regional_decision_journal_file(&path)?;
            let (pending, valid_len) = read_appended_regional_decisions(&path, &bytes)?;
            if valid_len < bytes.len() {
                let file = OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .map_err(|source| RegionalDecisionJournalOpenError::Io {
                        path: path.clone(),
                        source,
                    })?;
                file.set_len(valid_len as u64)
                    .and_then(|()| file.sync_all())
                    .map_err(|source| RegionalDecisionJournalOpenError::Io {
                        path: path.clone(),
                        source,
                    })?;
            }
            pending
        } else {
            Vec::new()
        };
        let (requests, receiver) = std::sync::mpsc::sync_channel(64);
        let worker_path = path.clone();
        let worker = std::thread::Builder::new()
            .name("solaris-entity-journal".to_owned())
            .spawn(move || run_regional_journal_writer(&worker_path, receiver))
            .map_err(|source| RegionalDecisionJournalOpenError::Io {
                path: path.clone(),
                source,
            })?;
        let journal = Self {
            path,
            pending: pending.clone(),
            needs_compaction: false,
            requests,
            worker: Some(worker),
        };
        Ok((journal, pending))
    }

    fn persist(&self) -> Result<(), RegionalDecisionJournalOpenError> {
        let (reply, completion) = std::sync::mpsc::channel();
        self.requests
            .send(RegionalJournalWriteRequest::Replace {
                pending: self.pending.clone(),
                reply,
            })
            .map_err(|_| journal_writer_closed(&self.path))?;
        completion
            .recv()
            .map_err(|_| journal_writer_closed(&self.path))?
            .map_err(|_| journal_writer_closed(&self.path))
    }

    fn append_commits(
        &self,
        decisions: &[RegionalCommitDecision],
    ) -> Result<(), RegionalDecisionJournalError> {
        validate_regional_decision_group(decisions)
            .map_err(|_| RegionalDecisionJournalError::SAFE)?;
        let (reply, completion) = std::sync::mpsc::channel();
        self.requests
            .send(RegionalJournalWriteRequest::Append {
                decisions: decisions.to_vec(),
                reply,
            })
            .map_err(|_| RegionalDecisionJournalError::SAFE)?;
        completion
            .recv()
            .map_err(|_| RegionalDecisionJournalError::OUTCOME_UNKNOWN)?
    }
}

impl Drop for FileRegionalDecisionJournal {
    fn drop(&mut self) {
        // The world checkpoint watermark makes acknowledged records replay-safe.
        // Compact only at this exact shutdown event so gameplay appends never queue
        // behind a checkpoint rewrite and its fsync.
        if self.needs_compaction {
            let _ = self.persist();
        }
        let (reply, completion) = std::sync::mpsc::channel();
        let _ = self
            .requests
            .send(RegionalJournalWriteRequest::Shutdown { reply });
        let _ = completion.recv();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_regional_journal_writer(
    path: &Path,
    receiver: std::sync::mpsc::Receiver<RegionalJournalWriteRequest>,
) {
    while let Ok(request) = receiver.recv() {
        match request {
            RegionalJournalWriteRequest::Append { decisions, reply } => {
                let result = append_regional_decisions(path, &decisions)
                    .map_err(|_| RegionalDecisionJournalError::OUTCOME_UNKNOWN);
                let _ = reply.send(result);
            }
            RegionalJournalWriteRequest::Replace { pending, reply } => {
                let result = persist_regional_decisions(path, &pending)
                    .map_err(|_| RegionalDecisionJournalError::SAFE);
                let _ = reply.send(result);
            }
            RegionalJournalWriteRequest::Shutdown { reply } => {
                let _ = reply.send(());
                break;
            }
        }
    }
}

fn append_regional_decisions(
    path: &Path,
    decisions: &[RegionalCommitDecision],
) -> Result<(), RegionalDecisionJournalOpenError> {
    if decisions.is_empty() {
        return Ok(());
    }
    validate_regional_commit_decisions(decisions)
        .map_err(|source| regional_decision_validation_error(path, source))?;
    validate_regional_decision_group(decisions)
        .map_err(|source| regional_decision_validation_error(path, source))?;
    let parent = path.parent().expect("journal path has parent");
    create_regional_journal_directory(parent)?;
    let existed = path.is_file();
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if file
        .metadata()
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len()
        == 0
    {
        file.write_all(REGIONAL_DECISION_JOURNAL_HEADER)
            .map_err(|source| RegionalDecisionJournalOpenError::Io {
                path: path.to_path_buf(),
                source,
            })?;
    }
    write_regional_decision_group(&mut file, path, decisions)?;
    file.flush()
        .and_then(|()| file.sync_all())
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if !existed {
        sync_regional_journal_directory(parent)?;
    }
    Ok(())
}

fn persist_regional_decisions(
    path: &Path,
    pending: &[RegionalCommitDecision],
) -> Result<(), RegionalDecisionJournalOpenError> {
    validate_regional_commit_decisions(pending)
        .map_err(|source| regional_decision_validation_error(path, source))?;
    validate_regional_decision_group(pending)
        .map_err(|source| regional_decision_validation_error(path, source))?;
    let parent = path.parent().expect("journal path has parent");
    create_regional_journal_directory(parent)?;
    if pending.is_empty() {
        if path.is_file() {
            std::fs::remove_file(path).map_err(|source| RegionalDecisionJournalOpenError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            sync_regional_journal_directory(parent)?;
        }
        return Ok(());
    }
    let temporary = temporary_write_path(path);
    let mut file =
        File::create(&temporary).map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: temporary.clone(),
            source,
        })?;
    file.write_all(REGIONAL_DECISION_JOURNAL_HEADER)
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: temporary.clone(),
            source,
        })?;
    write_regional_decision_group(&mut file, &temporary, pending)?;
    file.flush()
        .and_then(|()| file.sync_all())
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: temporary.clone(),
            source,
        })?;
    std::fs::rename(&temporary, path).map_err(|source| RegionalDecisionJournalOpenError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    sync_regional_journal_directory(parent)
}

fn journal_writer_closed(path: &Path) -> RegionalDecisionJournalOpenError {
    RegionalDecisionJournalOpenError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "regional journal writer closed",
        ),
    }
}

fn create_regional_journal_directory(path: &Path) -> Result<(), RegionalDecisionJournalOpenError> {
    let existed = path.is_dir();
    std::fs::create_dir_all(path).map_err(|source| RegionalDecisionJournalOpenError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !existed && let Some(parent) = path.parent() {
        sync_regional_journal_directory(parent)?;
    }
    Ok(())
}

fn read_appended_regional_decisions(
    path: &Path,
    bytes: &[u8],
) -> Result<(Vec<RegionalCommitDecision>, usize), RegionalDecisionJournalOpenError> {
    if !bytes.starts_with(REGIONAL_DECISION_JOURNAL_HEADER) {
        return Err(RegionalDecisionJournalOpenError::UnsupportedVersion(0));
    }
    let mut cursor = REGIONAL_DECISION_JOURNAL_HEADER.len();
    let mut pending = Vec::new();
    while cursor < bytes.len() {
        let frame_start = cursor;
        let remaining = bytes.len() - cursor;
        if remaining < REGIONAL_DECISION_JOURNAL_FRAME_HEADER_BYTES {
            return validate_regional_recovery_prefix(path, pending, frame_start);
        }
        let mut payload_len_bytes = [0_u8; 4];
        payload_len_bytes.copy_from_slice(&bytes[cursor..cursor + 4]);
        let payload_len = u32::from_be_bytes(payload_len_bytes) as u64;
        validate_regional_journal_frame_payload_len(payload_len).map_err(|reason| {
            RegionalDecisionJournalOpenError::Framing {
                path: path.to_path_buf(),
                reason,
            }
        })?;
        let payload_len = usize::try_from(payload_len).map_err(|_| {
            RegionalDecisionJournalOpenError::Framing {
                path: path.to_path_buf(),
                reason: "record payload does not fit this platform",
            }
        })?;
        let mut checksum_bytes = [0_u8; 4];
        checksum_bytes.copy_from_slice(
            &bytes[cursor + 4..cursor + REGIONAL_DECISION_JOURNAL_FRAME_HEADER_BYTES],
        );
        let expected_checksum = u32::from_be_bytes(checksum_bytes);
        cursor += REGIONAL_DECISION_JOURNAL_FRAME_HEADER_BYTES;
        let payload_end = cursor.checked_add(payload_len).ok_or_else(|| {
            RegionalDecisionJournalOpenError::Framing {
                path: path.to_path_buf(),
                reason: "record length overflow",
            }
        })?;
        let Some(payload) = bytes.get(cursor..payload_end) else {
            return validate_regional_recovery_prefix(path, pending, frame_start);
        };
        if crc32fast::hash(payload) != expected_checksum {
            return Err(RegionalDecisionJournalOpenError::Checksum {
                path: path.to_path_buf(),
            });
        }
        let encoded_group = serde_json::from_slice::<Vec<EncodedRegionalCommitDecision>>(payload)
            .map_err(|source| RegionalDecisionJournalOpenError::Json {
            path: path.to_path_buf(),
            source,
        })?;
        if encoded_group.is_empty() {
            return Err(RegionalDecisionJournalOpenError::Framing {
                path: path.to_path_buf(),
                reason: "empty commit group",
            });
        }
        let group = encoded_group
            .into_iter()
            .map(RegionalCommitDecision::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| regional_decision_validation_error(path, source))?;
        validate_regional_decision_group(&group)
            .map_err(|source| regional_decision_validation_error(path, source))?;
        pending.extend(group);
        cursor = payload_end;
    }
    validate_regional_recovery_prefix(path, pending, cursor)
}

fn validate_regional_recovery_prefix(
    path: &Path,
    pending: Vec<RegionalCommitDecision>,
    valid_len: usize,
) -> Result<(Vec<RegionalCommitDecision>, usize), RegionalDecisionJournalOpenError> {
    validate_regional_commit_decisions(&pending)
        .map_err(|source| regional_decision_validation_error(path, source))?;
    validate_regional_decision_group(&pending)
        .map_err(|source| regional_decision_validation_error(path, source))?;
    Ok((pending, valid_len))
}

fn write_regional_decision_group(
    file: &mut File,
    path: &Path,
    decisions: &[RegionalCommitDecision],
) -> Result<(), RegionalDecisionJournalOpenError> {
    validate_regional_decision_group(decisions)
        .map_err(|source| regional_decision_validation_error(path, source))?;
    let mut payload = BoundedRegionalDecisionPayload::new();
    let serialization = serde_json::to_writer(&mut payload, decisions);
    if payload.limit_exceeded {
        return Err(RegionalDecisionJournalOpenError::Framing {
            path: path.to_path_buf(),
            reason: "record payload exceeds operational limit",
        });
    }
    serialization.map_err(|source| RegionalDecisionJournalOpenError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    let payload = payload.bytes;
    let payload_len =
        u32::try_from(payload.len()).map_err(|_| RegionalDecisionJournalOpenError::Framing {
            path: path.to_path_buf(),
            reason: "record payload exceeds u32 length",
        })?;
    let current_len = file
        .metadata()
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    let next_len = current_len
        .checked_add(REGIONAL_DECISION_JOURNAL_FRAME_HEADER_BYTES as u64)
        .and_then(|length| length.checked_add(payload.len() as u64))
        .ok_or_else(|| RegionalDecisionJournalOpenError::Framing {
            path: path.to_path_buf(),
            reason: "journal file length overflow",
        })?;
    validate_regional_journal_file_len(next_len).map_err(|reason| {
        RegionalDecisionJournalOpenError::Framing {
            path: path.to_path_buf(),
            reason,
        }
    })?;
    file.write_all(&payload_len.to_be_bytes())
        .and_then(|()| file.write_all(&crc32fast::hash(&payload).to_be_bytes()))
        .and_then(|()| file.write_all(&payload))
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: path.to_path_buf(),
            source,
        })
}

struct BoundedRegionalDecisionPayload {
    bytes: Vec<u8>,
    limit_exceeded: bool,
}

impl BoundedRegionalDecisionPayload {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            limit_exceeded: false,
        }
    }
}

impl Write for BoundedRegionalDecisionPayload {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let next_len = self.bytes.len().checked_add(bytes.len()).ok_or_else(|| {
            self.limit_exceeded = true;
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "WAL payload length overflow",
            )
        })?;
        if u64::try_from(next_len).unwrap_or(u64::MAX) > MAX_REGIONAL_DECISION_FRAME_PAYLOAD_BYTES {
            self.limit_exceeded = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "WAL payload exceeds operational limit",
            ));
        }
        self.bytes.try_reserve(bytes.len()).map_err(|source| {
            std::io::Error::other(format!("WAL payload allocation failed: {source}"))
        })?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn read_regional_decision_journal_file(
    path: &Path,
) -> Result<Vec<u8>, RegionalDecisionJournalOpenError> {
    let file = File::open(path).map_err(|source| RegionalDecisionJournalOpenError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let file_len = file
        .metadata()
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    validate_regional_journal_file_len(file_len).map_err(|reason| {
        RegionalDecisionJournalOpenError::Framing {
            path: path.to_path_buf(),
            reason,
        }
    })?;
    let capacity =
        usize::try_from(file_len).map_err(|_| RegionalDecisionJournalOpenError::Framing {
            path: path.to_path_buf(),
            reason: "journal file length does not fit this platform",
        })?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::other(format!("journal allocation failed: {source}")),
        })?;
    file.take(MAX_REGIONAL_DECISION_JOURNAL_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    validate_regional_journal_file_len(bytes.len() as u64).map_err(|reason| {
        RegionalDecisionJournalOpenError::Framing {
            path: path.to_path_buf(),
            reason,
        }
    })?;
    Ok(bytes)
}

fn validate_regional_journal_file_len(length: u64) -> Result<(), &'static str> {
    if length > MAX_REGIONAL_DECISION_JOURNAL_BYTES {
        return Err("journal file exceeds operational limit");
    }
    Ok(())
}

fn validate_regional_journal_frame_payload_len(length: u64) -> Result<(), &'static str> {
    if length > MAX_REGIONAL_DECISION_FRAME_PAYLOAD_BYTES {
        return Err("record payload exceeds operational limit");
    }
    Ok(())
}

fn validate_regional_decision_group_shape(
    decision_count: usize,
    entity_mutation_count: usize,
) -> Result<(), RegionalDecisionReplayError> {
    if decision_count > MAX_REGIONAL_DECISIONS_PER_FRAME {
        return Err(RegionalDecisionReplayError::TooManyDecisions);
    }
    if entity_mutation_count > MAX_REGIONAL_ENTITY_MUTATIONS_PER_FRAME {
        return Err(RegionalDecisionReplayError::TooManyEntityMutations);
    }
    Ok(())
}

fn validate_regional_decision_group(
    decisions: &[RegionalCommitDecision],
) -> Result<(), RegionalDecisionReplayError> {
    let entity_mutation_count = decisions.iter().try_fold(0_usize, |count, decision| {
        count
            .checked_add(decision.upserts().len())
            .and_then(|count| count.checked_add(decision.removed().len()))
            .ok_or(RegionalDecisionReplayError::TooManyEntityMutations)
    })?;
    validate_regional_decision_group_shape(decisions.len(), entity_mutation_count)
}

fn regional_decision_validation_error(
    path: &Path,
    source: RegionalDecisionReplayError,
) -> RegionalDecisionJournalOpenError {
    RegionalDecisionJournalOpenError::Validation {
        path: path.to_path_buf(),
        source,
    }
}

fn validate_regional_commit_decisions(
    decisions: &[RegionalCommitDecision],
) -> Result<(), RegionalDecisionReplayError> {
    for pair in decisions.windows(2) {
        let [previous, current] = pair else {
            unreachable!("two-item decision window");
        };
        if current.phase() <= previous.phase()
            || current.sequence_watermark() <= previous.sequence_watermark()
            || current.lifecycle_epoch() < previous.lifecycle_epoch()
        {
            return Err(RegionalDecisionReplayError::InvalidOrdering);
        }
    }
    Ok(())
}

fn replayed_entity_snapshot_is_semantically_valid(
    snapshot: &EntitySnapshot,
    runtime_validator: &mut EntityStore,
) -> bool {
    static ENTITY_TYPES: std::sync::OnceLock<mc_data::entity_types::EntityTypeRegistry> =
        std::sync::OnceLock::new();
    let entity_types =
        ENTITY_TYPES.get_or_init(mc_data::entity_types::solaris_required_entity_types);
    let known_type = mc_data::Identifier::parse(snapshot.type_name.clone())
        .ok()
        .is_some_and(|name| entity_types.id_of(&name).is_some());
    if !known_type {
        return false;
    }

    let mut runtime_snapshot = snapshot.clone();
    runtime_snapshot.vehicle = None;
    runtime_validator.insert_snapshot(runtime_snapshot)
        && runtime_validator.remove(snapshot.id).is_some()
}

pub(crate) fn replay_regional_commit_decisions(
    checkpoint: PersistedEntityCheckpoint,
    decisions: &[RegionalCommitDecision],
) -> Result<PersistedEntityCheckpoint, RegionalDecisionReplayError> {
    validate_regional_commit_decisions(decisions)?;
    validate_regional_decision_group(decisions)?;
    let PersistedEntityCheckpoint {
        lifecycle_clock,
        regional_sequence_watermark,
        records,
    } = checkpoint;
    let mut entities = records
        .into_iter()
        .map(|record| (record.snapshot.id, record.snapshot))
        .collect::<BTreeMap<_, _>>();
    let checkpoint_boundary = (lifecycle_clock, regional_sequence_watermark);
    let mut replay_boundary = checkpoint_boundary;
    let mut runtime_validator = EntityStore::new();
    for decision in decisions {
        let predates_checkpoint = decision.lifecycle_epoch() < checkpoint_boundary.0
            || (decision.lifecycle_epoch() == checkpoint_boundary.0
                && decision.sequence_watermark() <= checkpoint_boundary.1);
        if predates_checkpoint {
            continue;
        }
        if decision.sequence_watermark() <= replay_boundary.1 {
            return Err(RegionalDecisionReplayError::InvalidOrdering);
        }
        for snapshot in decision.upserts() {
            if !replayed_entity_snapshot_is_semantically_valid(snapshot, &mut runtime_validator) {
                return Err(RegionalDecisionReplayError::InvalidSnapshot);
            }
        }
        for entity in decision.removed() {
            entities.remove(entity);
        }
        for snapshot in decision.upserts() {
            entities.insert(snapshot.id, snapshot.clone());
        }
        replay_boundary.0 = replay_boundary.0.max(decision.lifecycle_epoch());
        replay_boundary.1 = decision.sequence_watermark();
    }
    let restored = entities
        .into_values()
        .map(|snapshot| {
            PersistedEntityRecord::from_snapshot_at_lifecycle_clock(snapshot, replay_boundary.0)
        })
        .collect::<Vec<_>>();
    let mut uuids = std::collections::BTreeSet::new();
    for record in &restored {
        if !uuids.insert(record.snapshot.uuid) {
            return Err(RegionalDecisionReplayError::DuplicateEntityUuid(
                record.snapshot.uuid,
            ));
        }
    }
    Ok(PersistedEntityCheckpoint {
        lifecycle_clock: replay_boundary.0,
        regional_sequence_watermark: replay_boundary.1,
        records: restored,
    })
}

impl RegionalDecisionJournal for FileRegionalDecisionJournal {
    fn record_commit(
        &mut self,
        decision: &RegionalCommitDecision,
    ) -> Result<(), RegionalDecisionJournalError> {
        self.record_commits(std::slice::from_ref(decision))
    }

    fn record_commits(
        &mut self,
        decisions: &[RegionalCommitDecision],
    ) -> Result<(), RegionalDecisionJournalError> {
        validate_regional_decision_group(decisions)
            .map_err(|_| RegionalDecisionJournalError::SAFE)?;
        let mut pending = self.pending.clone();
        pending.extend_from_slice(decisions);
        if validate_regional_commit_decisions(&pending).is_err()
            || validate_regional_decision_group(&pending).is_err()
        {
            return Err(RegionalDecisionJournalError::SAFE);
        }
        match self.append_commits(decisions) {
            Ok(()) => {
                self.pending.extend_from_slice(decisions);
                Ok(())
            }
            Err(error) if error.outcome_unknown() => {
                self.pending.extend_from_slice(decisions);
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    fn clear_commit(&mut self, phase: RegionPhase) -> Result<(), RegionalDecisionJournalError> {
        self.clear_commits(&[phase])
    }

    fn clear_commits(
        &mut self,
        phases: &[RegionPhase],
    ) -> Result<(), RegionalDecisionJournalError> {
        let phases = phases
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        let identities = self
            .pending
            .iter()
            .filter(|decision| phases.contains(&decision.phase()))
            .map(RegionalCommitDecision::identity)
            .collect::<Vec<_>>();
        self.clear_commit_identities(&identities)
    }

    fn clear_commit_identities(
        &mut self,
        identities: &[(RegionPhase, u64, u64)],
    ) -> Result<(), RegionalDecisionJournalError> {
        let identities = identities
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        let retained = self
            .pending
            .iter()
            .filter(|decision| !identities.contains(&decision.identity()))
            .cloned()
            .collect::<Vec<_>>();
        if retained.len() != self.pending.len() {
            self.pending = retained;
            self.needs_compaction = true;
        }
        Ok(())
    }

    fn pending_phases(&self) -> Vec<RegionPhase> {
        self.pending
            .iter()
            .map(RegionalCommitDecision::phase)
            .collect()
    }

    fn pending_commit_identities(&self) -> Vec<(RegionPhase, u64, u64)> {
        self.pending
            .iter()
            .map(RegionalCommitDecision::identity)
            .collect()
    }

    fn recovery_watermark(&self) -> (RegionPhase, u64) {
        self.pending
            .iter()
            .fold((RegionPhase(0), 0), |(phase, sequence), decision| {
                (
                    phase.max(decision.phase()),
                    sequence.max(decision.sequence_watermark()),
                )
            })
    }
}

#[cfg(unix)]
fn sync_regional_journal_directory(path: &Path) -> Result<(), RegionalDecisionJournalOpenError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
fn sync_regional_journal_directory(_path: &Path) -> Result<(), RegionalDecisionJournalOpenError> {
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorldPersistedMetadata {
    pub(crate) world_time: u64,
    pub(crate) players_sleeping_percentage: u32,
    pub(crate) keep_inventory: bool,
    pub(crate) world_identity: String,
}

#[derive(Debug, Clone)]
pub(crate) struct PersistedEntityRecord {
    pub(crate) snapshot: EntitySnapshot,
    pub(crate) age: i32,
    pub(crate) pickup_delay: i32,
}

impl PersistedEntityRecord {
    pub(crate) fn from_snapshot_at_lifecycle_clock(
        snapshot: EntitySnapshot,
        lifecycle_clock: u64,
    ) -> Self {
        let age = lifecycle_clock
            .saturating_sub(snapshot.retained.spawn_tick)
            .min(i32::MAX as u64) as i32;
        let pickup_delay = snapshot
            .retained
            .item_pickup_ready_tick
            .map(|ready_tick| ready_tick.saturating_sub(lifecycle_clock))
            .unwrap_or(0)
            .min(i32::from(i16::MAX) as u64) as i32;
        Self {
            snapshot,
            age,
            pickup_delay,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PersistedEntityCheckpoint {
    pub(crate) lifecycle_clock: u64,
    pub(crate) regional_sequence_watermark: u64,
    pub(crate) records: Vec<PersistedEntityRecord>,
}

impl PersistedEntityCheckpoint {
    #[cfg(test)]
    pub(crate) fn new(
        lifecycle_clock: u64,
        records: impl IntoIterator<Item = impl Into<PersistedEntityRecord>>,
    ) -> Self {
        Self {
            lifecycle_clock,
            regional_sequence_watermark: 0,
            records: records.into_iter().map(Into::into).collect(),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_at_owner_sequence(
        lifecycle_clock: u64,
        regional_sequence_watermark: u64,
        records: impl IntoIterator<Item = impl Into<PersistedEntityRecord>>,
    ) -> Self {
        Self {
            lifecycle_clock,
            regional_sequence_watermark,
            records: records.into_iter().map(Into::into).collect(),
        }
    }

    pub(crate) fn has_valid_temporal_state(&self) -> bool {
        self.lifecycle_clock <= i64::MAX as u64
            && self.records.iter().all(|record| {
                let Ok(age) = u64::try_from(record.age) else {
                    return false;
                };
                let Ok(pickup_delay) = u64::try_from(record.pickup_delay) else {
                    return false;
                };
                age <= self.lifecycle_clock
                    && record.snapshot.retained.spawn_tick <= self.lifecycle_clock
                    && self
                        .lifecycle_clock
                        .saturating_sub(record.snapshot.retained.spawn_tick)
                        == age
                    && record
                        .snapshot
                        .retained
                        .item_pickup_ready_tick
                        .map(|ready_tick| ready_tick.saturating_sub(self.lifecycle_clock))
                        .unwrap_or(0)
                        == pickup_delay
                    && self
                        .lifecycle_clock
                        .checked_add(pickup_delay)
                        .is_some_and(|deadline| deadline <= i64::MAX as u64)
                    && entity_temporal_state_is_valid(self.lifecycle_clock, &record.snapshot)
            })
    }

    fn has_valid_vehicle_graph(&self) -> bool {
        let entities = self
            .records
            .iter()
            .map(|record| (record.snapshot.id, &record.snapshot))
            .collect::<BTreeMap<_, _>>();
        if entities.len() != self.records.len() {
            return false;
        }
        let mut vehicle_passengers = BTreeMap::new();
        let mut passenger_vehicles = BTreeMap::new();
        for snapshot in entities.values() {
            let Some(passenger) = snapshot.vehicle.and_then(|vehicle| vehicle.passenger) else {
                continue;
            };
            let Some(passenger_snapshot) = entities.get(&passenger) else {
                return false;
            };
            if passenger == snapshot.id
                || mc_entity::RegionKey::from_position(passenger_snapshot.position)
                    != mc_entity::RegionKey::from_position(snapshot.position)
                || vehicle_passengers.insert(snapshot.id, passenger).is_some()
                || passenger_vehicles.insert(passenger, snapshot.id).is_some()
            {
                return false;
            }
        }
        for &start in entities.keys() {
            let mut current = start;
            let mut visited = std::collections::BTreeSet::new();
            while let Some(&passenger) = vehicle_passengers.get(&current) {
                if !visited.insert(current) {
                    return false;
                }
                current = passenger;
            }
        }
        true
    }
}

impl From<EntitySnapshot> for PersistedEntityRecord {
    fn from(snapshot: EntitySnapshot) -> Self {
        Self {
            snapshot,
            age: 0,
            pickup_delay: 0,
        }
    }
}

impl std::ops::Deref for PersistedEntityRecord {
    type Target = EntitySnapshot;

    fn deref(&self) -> &Self::Target {
        &self.snapshot
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct XpState {
    pub(super) level: i32,
    pub(super) progress: f32,
    pub(super) total: i32,
    pub(super) seed: i32,
}

impl Default for XpState {
    fn default() -> Self {
        Self {
            level: 0,
            progress: 0.0,
            total: 0,
            seed: 0,
        }
    }
}

impl XpState {
    pub(super) fn reset(&mut self) -> bool {
        let changed = self.level != 0 || self.progress != 0.0 || self.total != 0;
        self.level = 0;
        self.progress = 0.0;
        self.total = 0;
        changed
    }

    pub(super) fn add_points(&mut self, points: i32) -> bool {
        if points <= 0 {
            return false;
        }
        self.total = self.total.saturating_add(points).max(0);
        self.progress += points as f32 / Self::points_to_next_level(self.level) as f32;
        while self.progress >= 1.0 {
            let points_above_level =
                (self.progress - 1.0) * Self::points_to_next_level(self.level) as f32;
            self.level = self.level.saturating_add(1);
            self.progress = points_above_level / Self::points_to_next_level(self.level) as f32;
        }
        true
    }

    pub(super) fn spend_enchantment_levels(&mut self, levels: i32, next_seed: i32) -> bool {
        if levels <= 0 || self.level < levels {
            return false;
        }
        self.level -= levels;
        self.seed = next_seed;
        true
    }

    fn points_to_next_level(level: i32) -> i32 {
        if level >= 30 {
            112_i32.saturating_add(level.saturating_sub(30).saturating_mul(9))
        } else if level >= 15 {
            37_i32.saturating_add(level.saturating_sub(15).saturating_mul(5))
        } else {
            7_i32.saturating_add(level.max(0).saturating_mul(2))
        }
    }

    pub(super) const fn as_packet(&self) -> ClientboundSetExperience {
        ClientboundSetExperience {
            experience_progress: self.progress,
            total_experience: self.total,
            experience_level: self.level,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct SpawnState {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) z: i32,
    pub(super) angle: f32,
}

impl SpawnState {
    pub(super) fn from_pose(pose: PlayerPose) -> Self {
        Self {
            x: pose.x.floor() as i32,
            y: pose.y.floor() as i32,
            z: pose.z.floor() as i32,
            angle: pose.yaw,
        }
    }

    pub(super) fn pose(&self) -> PlayerPose {
        let mut pose = PlayerPose::new(
            f64::from(self.x) + 0.5,
            f64::from(self.y),
            f64::from(self.z) + 0.5,
        );
        pose.yaw = self.angle;
        pose
    }
}

#[derive(Debug, Clone)]
pub(super) struct InventorySlotExtras {
    item_id: u32,
    damage: Option<i32>,
    enchantments: Vec<mc_data::ItemEnchantment>,
    custom_name: Option<String>,
    fields: Vec<(String, Tag)>,
}

#[derive(Debug, Clone)]
pub(crate) struct PlayerPersistedState {
    pub(super) pose: PlayerPose,
    pub(super) game_mode: GameMode,
    pub(super) survival: SurvivalState,
    pub(super) inventory: PlayerInventory,
    pub(super) carried_item: ItemStack,
    pub(super) crafting_table_input: Option<Box<[ItemStack; 9]>>,
    pub(super) enchanting_table_input: Option<Box<[ItemStack; 2]>>,
    pub(super) selected_hotbar_slot: u8,
    pub(super) spawn: SpawnState,
    pub(super) xp: XpState,
    inventory_extras: [Option<InventorySlotExtras>; 46],
}

impl PlayerPersistedState {
    pub(super) fn new_default(spawn: PlayerPose) -> Self {
        Self {
            pose: spawn,
            game_mode: GameMode::Survival,
            survival: SurvivalState::FULL,
            inventory: PlayerInventory::empty(),
            carried_item: ItemStack::EMPTY,
            crafting_table_input: None,
            enchanting_table_input: None,
            selected_hotbar_slot: 0,
            spawn: SpawnState::from_pose(spawn),
            xp: XpState::default(),
            inventory_extras: std::array::from_fn(|_| None),
        }
    }

    pub(super) fn replace_inventory(&mut self, inventory: PlayerInventory) {
        if self.inventory.slots != inventory.slots {
            self.inventory = inventory;
        }
    }

    pub(super) fn replace_container(
        &mut self,
        inventory: PlayerInventory,
        carried_item: ItemStack,
    ) {
        if self.inventory.slots != inventory.slots || self.carried_item != carried_item {
            self.inventory = inventory;
            self.carried_item = carried_item;
        }
    }

    pub(super) fn replace_xp(&mut self, xp: XpState) {
        if self.xp != xp {
            self.xp = xp;
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum PlayerPersistenceError {
    #[error("persistence I/O at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("persistence NBT at {path}: {source}")]
    Nbt {
        path: PathBuf,
        source: mc_nbt::NbtError,
    },
    #[error("persistence root at {path} is not a compound")]
    RootNotCompound { path: PathBuf },
    #[error("playerdata item id is invalid: {0}")]
    InvalidItemId(String),
    #[error("playerdata item is not in registry: {0}")]
    UnknownItem(String),
    #[error("playerdata enchantment is invalid or not in registry: {0}")]
    InvalidEnchantment(String),
    #[error("persistence numeric field {field} at {path} is not finite")]
    InvalidNumeric { path: PathBuf, field: &'static str },
    #[error("persistence field {field} at {path} has an invalid value")]
    InvalidValue { path: PathBuf, field: &'static str },
    #[error("duplicate entity UUID {uuid} at {path}")]
    DuplicateEntityUuid { path: PathBuf, uuid: uuid::Uuid },
    #[error("unsupported entity persistence format version {version} at {path}")]
    UnsupportedEntityFormatVersion { path: PathBuf, version: i32 },
}

fn validate_player_numeric_state(
    path: &Path,
    state: &PlayerPersistedState,
) -> Result<(), PlayerPersistenceError> {
    if !state.pose.x.is_finite() || !state.pose.y.is_finite() || !state.pose.z.is_finite() {
        return Err(PlayerPersistenceError::InvalidNumeric {
            path: path.to_path_buf(),
            field: "Pos",
        });
    }
    if !state.pose.yaw.is_finite() || !state.pose.pitch.is_finite() {
        return Err(PlayerPersistenceError::InvalidNumeric {
            path: path.to_path_buf(),
            field: "Rotation",
        });
    }
    if !state.survival.health.is_finite()
        || !state.survival.saturation.is_finite()
        || !state.survival.exhaustion.is_finite()
    {
        return Err(PlayerPersistenceError::InvalidNumeric {
            path: path.to_path_buf(),
            field: "survival",
        });
    }
    if !state.spawn.angle.is_finite() || !state.xp.progress.is_finite() {
        return Err(PlayerPersistenceError::InvalidNumeric {
            path: path.to_path_buf(),
            field: "spawn/xp",
        });
    }
    Ok(())
}

fn validate_entity_numeric_state(
    path: &Path,
    entity: &EntitySnapshot,
) -> Result<(), PlayerPersistenceError> {
    if !entity.position.x.is_finite()
        || !entity.position.y.is_finite()
        || !entity.position.z.is_finite()
        || !entity.velocity.x.is_finite()
        || !entity.velocity.y.is_finite()
        || !entity.velocity.z.is_finite()
        || !entity.rotation.yaw.is_finite()
        || !entity.rotation.pitch.is_finite()
        || !entity.rotation.head_yaw.is_finite()
        || !entity.health.is_finite()
        || !entity.retained.fall_distance.is_finite()
        || entity.retained.fall_distance < 0.0
    {
        return Err(PlayerPersistenceError::InvalidNumeric {
            path: path.to_path_buf(),
            field: "entity Pos/Motion/Rotation/Health/FallDistance",
        });
    }
    Ok(())
}

fn normalize_loaded_entity_kinematics(
    path: &Path,
    position: &mut Vec3,
    rotation: mc_entity::Rotation,
    velocity: &mut Vec3,
) -> Result<(), PlayerPersistenceError> {
    if !position.is_finite() {
        return Err(PlayerPersistenceError::InvalidNumeric {
            path: path.to_path_buf(),
            field: "Pos",
        });
    }
    if !rotation.is_finite() {
        return Err(PlayerPersistenceError::InvalidNumeric {
            path: path.to_path_buf(),
            field: "Rotation",
        });
    }
    if !velocity.is_finite() {
        return Err(PlayerPersistenceError::InvalidNumeric {
            path: path.to_path_buf(),
            field: "Motion",
        });
    }

    position.x = position.x.clamp(
        -ENTITY_HORIZONTAL_POSITION_LIMIT_26_1_2,
        ENTITY_HORIZONTAL_POSITION_LIMIT_26_1_2,
    );
    position.y = position.y.clamp(
        -ENTITY_VERTICAL_POSITION_LIMIT_26_1_2,
        ENTITY_VERTICAL_POSITION_LIMIT_26_1_2,
    );
    position.z = position.z.clamp(
        -ENTITY_HORIZONTAL_POSITION_LIMIT_26_1_2,
        ENTITY_HORIZONTAL_POSITION_LIMIT_26_1_2,
    );
    for component in [&mut velocity.x, &mut velocity.y, &mut velocity.z] {
        if component.abs() > ENTITY_VELOCITY_LIMIT_26_1_2 {
            *component = 0.0;
        }
        *component *= ENTITY_TICKS_PER_SECOND;
    }
    Ok(())
}

pub(super) fn load_player_state(
    world_root: &Path,
    uuid: uuid::Uuid,
    items: &ItemRegistry,
    default: PlayerPersistedState,
) -> Result<Option<PlayerPersistedState>, PlayerPersistenceError> {
    let path = playerdata_path(world_root, uuid);
    if !path.is_file() {
        return Ok(None);
    }

    let (root_name, root) = read_player_root(&path)?;
    let Tag::Compound(fields) = root else {
        return Err(PlayerPersistenceError::RootNotCompound { path });
    };
    let _ = root_name;

    let mut state = default;
    if let Some(pose) = read_pose(&fields) {
        state.pose = pose;
    }
    if let Some(game_mode) = int_field(&fields, "playerGameType") {
        state.game_mode = GameMode::from_id(game_mode);
    }
    if let Some(health) = float_field(&fields, "Health") {
        state.survival.health = health.clamp(0.0, SurvivalState::MAX_HEALTH);
    }
    if let Some(food) = int_field(&fields, "foodLevel") {
        state.survival.food = food.clamp(0, SurvivalState::MAX_FOOD);
    }
    if let Some(saturation) = float_field(&fields, "foodSaturationLevel") {
        state.survival.saturation = saturation.max(0.0);
    }
    if let Some(exhaustion) = float_field(&fields, "foodExhaustionLevel") {
        state.survival.exhaustion = exhaustion.max(0.0);
    }
    if let Some(slot) = int_field(&fields, "SelectedItemSlot") {
        state.selected_hotbar_slot = slot.clamp(0, 8) as u8;
    }
    if let Some(spawn) = read_spawn(&fields) {
        state.spawn = spawn;
    }
    state.xp.level = int_field(&fields, "XpLevel")
        .unwrap_or(state.xp.level)
        .max(0);
    state.xp.progress = float_field(&fields, "XpP")
        .unwrap_or(state.xp.progress)
        .clamp(0.0, 1.0);
    state.xp.total = int_field(&fields, "XpTotal")
        .unwrap_or(state.xp.total)
        .max(0);
    state.xp.seed = int_field(&fields, "XpSeed").unwrap_or(state.xp.seed);

    if let Some(Tag::List(list)) = field(&fields, "Inventory") {
        for element in &list.elements {
            let Tag::Compound(item_fields) = element else {
                continue;
            };
            let Some(slot) = slot_field(item_fields) else {
                continue;
            };
            if slot >= state.inventory.slots.len() {
                continue;
            }
            let Some(stack) = item_stack_from_fields(items, item_fields)? else {
                continue;
            };
            state.inventory.slots[slot] = stack.clone();
            let extras = item_fields
                .iter()
                .filter(|(name, _)| !is_modelled_item_key(name))
                .cloned()
                .collect::<Vec<_>>();
            if !extras.is_empty() {
                state.inventory_extras[slot] = Some(InventorySlotExtras {
                    item_id: stack.item_id,
                    damage: stack.damage,
                    enchantments: stack.enchantments,
                    custom_name: stack.custom_name,
                    fields: extras,
                });
            }
        }
    }
    if let Some(Tag::Compound(item_fields)) = field(&fields, CARRIED_ITEM_FIELD) {
        state.carried_item = item_stack_from_fields(items, item_fields)?.unwrap_or_default();
    }
    state.crafting_table_input =
        item_stack_projection_from_field::<9>(items, &fields, CRAFTING_TABLE_INPUT_FIELD)?;
    state.enchanting_table_input =
        item_stack_projection_from_field::<2>(items, &fields, ENCHANTING_TABLE_INPUT_FIELD)?;

    validate_player_numeric_state(&path, &state)?;
    Ok(Some(state))
}

pub(crate) fn save_player_state(
    world_root: &Path,
    uuid: uuid::Uuid,
    items: &ItemRegistry,
    state: &PlayerPersistedState,
) -> Result<(), PlayerPersistenceError> {
    let path = playerdata_path(world_root, uuid);
    validate_player_numeric_state(&path, state)?;
    let (root_name, root) = if path.is_file() {
        read_player_root(&path)?
    } else {
        (String::new(), Tag::Compound(Vec::new()))
    };
    let Tag::Compound(mut fields) = root else {
        return Err(PlayerPersistenceError::RootNotCompound { path });
    };

    set_field(&mut fields, "Pos", pose_position_tag(state.pose));
    set_field(&mut fields, "Rotation", pose_rotation_tag(state.pose));
    set_field(
        &mut fields,
        "OnGround",
        Tag::Byte(i8::from(state.pose.flags.on_ground)),
    );
    set_field(
        &mut fields,
        "playerGameType",
        Tag::Int(state.game_mode.id()),
    );
    set_field(&mut fields, "Health", Tag::Float(state.survival.health));
    set_field(&mut fields, "foodLevel", Tag::Int(state.survival.food));
    set_field(
        &mut fields,
        "foodSaturationLevel",
        Tag::Float(state.survival.saturation),
    );
    set_field(
        &mut fields,
        "foodExhaustionLevel",
        Tag::Float(state.survival.exhaustion),
    );
    set_field(
        &mut fields,
        "SelectedItemSlot",
        Tag::Int(i32::from(state.selected_hotbar_slot)),
    );
    set_field(&mut fields, "SpawnX", Tag::Int(state.spawn.x));
    set_field(&mut fields, "SpawnY", Tag::Int(state.spawn.y));
    set_field(&mut fields, "SpawnZ", Tag::Int(state.spawn.z));
    set_field(&mut fields, "SpawnAngle", Tag::Float(state.spawn.angle));
    set_field(&mut fields, "XpLevel", Tag::Int(state.xp.level));
    set_field(&mut fields, "XpP", Tag::Float(state.xp.progress));
    set_field(&mut fields, "XpTotal", Tag::Int(state.xp.total));
    set_field(&mut fields, "XpSeed", Tag::Int(state.xp.seed));
    set_field(&mut fields, "Inventory", inventory_tag(items, state)?);
    set_field(
        &mut fields,
        CARRIED_ITEM_FIELD,
        item_stack_tag(items, &state.carried_item)?,
    );
    set_field(
        &mut fields,
        CRAFTING_TABLE_INPUT_FIELD,
        item_stack_projection_tag(items, state.crafting_table_input.as_deref())?,
    );
    set_field(
        &mut fields,
        ENCHANTING_TABLE_INPUT_FIELD,
        item_stack_projection_tag(items, state.enchanting_table_input.as_deref())?,
    );

    write_player_root(&path, &root_name, &Tag::Compound(fields))
}

pub(crate) fn load_persisted_entities(
    world_root: &Path,
    items: &ItemRegistry,
    entity_types: &EntityTypeRegistry,
) -> Result<PersistedEntityCheckpoint, PlayerPersistenceError> {
    let path = entities_path(world_root);
    if !path.is_file() {
        return Ok(PersistedEntityCheckpoint {
            lifecycle_clock: 0,
            regional_sequence_watermark: 0,
            records: Vec::new(),
        });
    }
    let (_, root) = read_player_root(&path)?;
    let Tag::Compound(fields) = root else {
        return Err(PlayerPersistenceError::RootNotCompound { path });
    };
    let mut format_versions = fields
        .iter()
        .filter(|(name, _)| name == ENTITY_FORMAT_VERSION_FIELD);
    let format_version = format_versions.next().map(|(_, value)| value);
    if format_versions.next().is_some() {
        return Err(PlayerPersistenceError::InvalidValue {
            path,
            field: ENTITY_FORMAT_VERSION_FIELD,
        });
    }
    match format_version {
        None => {
            return Err(PlayerPersistenceError::InvalidValue {
                path,
                field: ENTITY_FORMAT_VERSION_FIELD,
            });
        }
        Some(Tag::Int(ENTITY_FORMAT_VERSION)) => {}
        Some(Tag::Int(version)) => {
            return Err(PlayerPersistenceError::UnsupportedEntityFormatVersion {
                path,
                version: *version,
            });
        }
        Some(_) => {
            return Err(PlayerPersistenceError::InvalidValue {
                path,
                field: ENTITY_FORMAT_VERSION_FIELD,
            });
        }
    }
    let mut lifecycle_ticks = fields
        .iter()
        .filter(|(name, _)| name == ENTITY_LIFECYCLE_TICK_FIELD);
    let lifecycle_tick = match lifecycle_ticks.next().map(|(_, value)| value) {
        Some(Tag::Long(value)) if *value >= 0 && lifecycle_ticks.next().is_none() => *value as u64,
        _ => {
            return Err(PlayerPersistenceError::InvalidValue {
                path,
                field: ENTITY_LIFECYCLE_TICK_FIELD,
            });
        }
    };
    let mut regional_sequences = fields
        .iter()
        .filter(|(name, _)| name == ENTITY_REGIONAL_SEQUENCE_FIELD);
    let regional_sequence_watermark = match regional_sequences.next().map(|(_, value)| value) {
        Some(Tag::Long(value)) if *value >= 0 && regional_sequences.next().is_none() => {
            *value as u64
        }
        _ => {
            return Err(PlayerPersistenceError::InvalidValue {
                path,
                field: ENTITY_REGIONAL_SEQUENCE_FIELD,
            });
        }
    };
    let Some(Tag::List(list)) = field(&fields, "Entities") else {
        return Err(PlayerPersistenceError::InvalidValue {
            path,
            field: "Entities",
        });
    };
    let mut entities = Vec::new();
    let mut entity_uuids = std::collections::BTreeSet::new();
    for element in &list.elements {
        let Tag::Compound(fields) = element else {
            continue;
        };
        let Some(type_name) = string_field(fields, "id") else {
            continue;
        };
        let parsed = mc_data::Identifier::parse(type_name.to_string())
            .map_err(|_| PlayerPersistenceError::InvalidItemId(type_name.to_string()))?;
        let type_id = entity_types
            .id_of(&parsed)
            .ok_or_else(|| PlayerPersistenceError::UnknownItem(type_name.to_string()))?
            as i32;
        let pos = double_list::<3>(
            field(fields, "Pos").unwrap_or(&Tag::List(ListTag::empty())),
            3,
        )
        .unwrap_or([0.0, 0.0, 0.0]);
        let motion = double_list::<3>(
            field(fields, "Motion").unwrap_or(&Tag::List(ListTag::empty())),
            3,
        )
        .unwrap_or([0.0, 0.0, 0.0]);
        let rotation = float_list::<2>(
            field(fields, "Rotation").unwrap_or(&Tag::List(ListTag::empty())),
            2,
        )
        .unwrap_or([0.0, 0.0]);
        let item_stack = if let Some(Tag::Compound(item)) = field(fields, "Item") {
            read_entity_item_stack(item, items)?
        } else {
            None
        };
        let experience_value = int_field(fields, "Value").filter(|value| *value > 0);
        let block_state =
            int_field(fields, "BlockState").and_then(|value| u32::try_from(value).ok());
        let health = float_field(fields, "Health").unwrap_or(20.0).max(0.0);
        let attributes = string_field(fields, ENTITY_ATTRIBUTES_FIELD)
            .ok_or_else(|| PlayerPersistenceError::InvalidValue {
                path: path.clone(),
                field: ENTITY_ATTRIBUTES_FIELD,
            })
            .and_then(|encoded| {
                serde_json::from_str(encoded).map_err(|_| PlayerPersistenceError::InvalidValue {
                    path: path.clone(),
                    field: ENTITY_ATTRIBUTES_FIELD,
                })
            })?;
        let id = EntityId(int_field(fields, "SolarisEntityId").unwrap_or(0).max(0));
        let uuid = uuid_field(fields).unwrap_or_else(|| {
            let id = int_field(fields, "SolarisEntityId").unwrap_or(0) as u32 as u128;
            uuid::Uuid::from_u128(0x5f1a_0000_0000_0000_0000_0000_0000_0000 | id)
        });
        let aquatic = persisted_entity_type_is_aquatic(type_name);
        let has_lifetime_age = field(fields, "SolarisLifetimeAge").is_some();
        let animal = persisted_entity_type_is_supported_breeding_animal(type_name).then(|| {
            mc_entity::AnimalBreedingState {
                age_ticks: if has_lifetime_age {
                    int_field(fields, "Age").unwrap_or(0)
                } else {
                    0
                },
                love_ticks: int_field(fields, "InLove")
                    .unwrap_or(0)
                    .clamp(0, i32::from(u16::MAX)) as u16,
                sheep_wool: (type_name == "minecraft:sheep").then(|| {
                    let color = int_field(fields, "Color")
                        .and_then(|value| u8::try_from(value).ok())
                        .and_then(mc_entity::SheepColor::from_id)
                        .unwrap_or_default();
                    mc_entity::SheepWoolState {
                        color,
                        sheared: byte_field(fields, "Sheared").is_some_and(|value| value != 0),
                    }
                }),
            }
        });
        let age = int_field(fields, "SolarisLifetimeAge")
            .or_else(|| animal.is_none().then(|| int_field(fields, "Age")).flatten())
            .unwrap_or(0);
        let pickup_delay = int_field(fields, "PickupDelay").unwrap_or(0);
        if age < 0
            || age as u64 > lifecycle_tick
            || pickup_delay < 0
            || lifecycle_tick
                .checked_add(pickup_delay as u64)
                .is_none_or(|deadline| deadline > i64::MAX as u64)
        {
            return Err(PlayerPersistenceError::InvalidValue {
                path: path.clone(),
                field: ENTITY_LIFECYCLE_TICK_FIELD,
            });
        }
        let lifecycle = match byte_field(fields, ENTITY_LIFECYCLE_FIELD) {
            Some(0) => EntityLifecycle::Alive,
            Some(1) => EntityLifecycle::Despawning,
            _ => {
                return Err(PlayerPersistenceError::InvalidValue {
                    path: path.clone(),
                    field: ENTITY_LIFECYCLE_FIELD,
                });
            }
        };
        let mut retained: mc_entity::EntityRetainedState =
            string_field(fields, ENTITY_RETAINED_STATE_FIELD)
                .ok_or_else(|| PlayerPersistenceError::InvalidValue {
                    path: path.clone(),
                    field: ENTITY_RETAINED_STATE_FIELD,
                })
                .and_then(|encoded| {
                    serde_json::from_str(encoded).map_err(|_| {
                        PlayerPersistenceError::InvalidValue {
                            path: path.clone(),
                            field: ENTITY_RETAINED_STATE_FIELD,
                        }
                    })
                })?;
        if let Some(fall_distance) = float_field(fields, "FallDistance") {
            retained.fall_distance = f64::from(fall_distance);
        }
        let head_yaw = float_field(fields, ENTITY_HEAD_YAW_FIELD).ok_or_else(|| {
            PlayerPersistenceError::InvalidValue {
                path: path.clone(),
                field: ENTITY_HEAD_YAW_FIELD,
            }
        })?;
        let goal = string_field(fields, ENTITY_GOAL_STATE_FIELD)
            .ok_or_else(|| PlayerPersistenceError::InvalidValue {
                path: path.clone(),
                field: ENTITY_GOAL_STATE_FIELD,
            })
            .and_then(|encoded| {
                serde_json::from_str(encoded).map_err(|_| PlayerPersistenceError::InvalidValue {
                    path: path.clone(),
                    field: ENTITY_GOAL_STATE_FIELD,
                })
            })?;
        let vehicle = string_field(fields, ENTITY_VEHICLE_STATE_FIELD)
            .ok_or_else(|| PlayerPersistenceError::InvalidValue {
                path: path.clone(),
                field: ENTITY_VEHICLE_STATE_FIELD,
            })
            .and_then(|encoded| {
                serde_json::from_str(encoded).map_err(|_| PlayerPersistenceError::InvalidValue {
                    path: path.clone(),
                    field: ENTITY_VEHICLE_STATE_FIELD,
                })
            })?;
        let mut snapshot = EntitySnapshot {
            id,
            uuid,
            type_id,
            type_name: type_name.to_string(),
            position: Vec3::new(pos[0], pos[1], pos[2]),
            rotation: mc_entity::Rotation {
                yaw: rotation[0],
                pitch: rotation[1],
                head_yaw,
            },
            velocity: Vec3::new(motion[0], motion[1], motion[2]),
            on_ground: byte_field(fields, "OnGround").unwrap_or(0) != 0 && !aquatic,
            item_stack,
            experience_value,
            block_state,
            lifecycle,
            health,
            attributes,
            goal,
            vehicle,
            animal,
            retained,
        };
        normalize_loaded_entity_kinematics(
            &path,
            &mut snapshot.position,
            snapshot.rotation,
            &mut snapshot.velocity,
        )?;
        validate_entity_numeric_state(&path, &snapshot)?;
        validate_entity_temporal_state(&path, lifecycle_tick, &snapshot)?;
        if !entity_uuids.insert(snapshot.uuid) {
            return Err(PlayerPersistenceError::DuplicateEntityUuid {
                path,
                uuid: snapshot.uuid,
            });
        }
        entities.push(PersistedEntityRecord {
            snapshot,
            age,
            pickup_delay,
        });
    }
    let checkpoint = PersistedEntityCheckpoint {
        lifecycle_clock: lifecycle_tick,
        regional_sequence_watermark,
        records: entities,
    };
    if !checkpoint.has_valid_temporal_state() {
        return Err(PlayerPersistenceError::InvalidValue {
            path: path.to_path_buf(),
            field: ENTITY_RETAINED_STATE_FIELD,
        });
    }
    if !checkpoint.has_valid_vehicle_graph() {
        return Err(PlayerPersistenceError::InvalidValue {
            path: path.to_path_buf(),
            field: ENTITY_VEHICLE_STATE_FIELD,
        });
    }
    Ok(checkpoint)
}

fn validate_entity_temporal_state(
    path: &Path,
    lifecycle_tick: u64,
    snapshot: &EntitySnapshot,
) -> Result<(), PlayerPersistenceError> {
    if !entity_temporal_state_is_valid(lifecycle_tick, snapshot) {
        return Err(PlayerPersistenceError::InvalidValue {
            path: path.to_path_buf(),
            field: ENTITY_RETAINED_STATE_FIELD,
        });
    }
    Ok(())
}

fn entity_temporal_state_is_valid(lifecycle_tick: u64, snapshot: &EntitySnapshot) -> bool {
    if snapshot
        .retained
        .last_damage_tick
        .is_some_and(|tick| tick > lifecycle_tick)
    {
        return false;
    }
    let death_state_valid = match (snapshot.lifecycle, snapshot.retained.death_remove_tick) {
        (EntityLifecycle::Alive, None) => true,
        (EntityLifecycle::Despawning, Some(deadline)) => deadline
            .checked_sub(lifecycle_tick)
            .is_some_and(|remaining| remaining <= super::session::ENTITY_DEATH_TICKS),
        _ => false,
    };
    death_state_valid
        && !snapshot
            .retained
            .sheep_grazing_ticks
            .is_some_and(|remaining| {
                remaining == 0 || remaining > super::session::SHEEP_GRAZING_ANIMATION_TICKS
            })
}

fn persisted_entity_type_is_aquatic(type_name: &str) -> bool {
    matches!(
        type_name,
        "minecraft:cod"
            | "minecraft:salmon"
            | "minecraft:tropical_fish"
            | "minecraft:pufferfish"
            | "minecraft:squid"
            | "minecraft:glow_squid"
            | "minecraft:dolphin"
            | "minecraft:axolotl"
            | "minecraft:turtle"
    )
}

fn persisted_entity_type_is_supported_breeding_animal(type_name: &str) -> bool {
    matches!(
        type_name,
        "minecraft:cow" | "minecraft:sheep" | "minecraft:chicken"
    )
}

#[cfg(test)]
pub(crate) fn save_persisted_entities(
    world_root: &Path,
    items: &ItemRegistry,
    entities: &[EntitySnapshot],
) -> Result<(), PlayerPersistenceError> {
    let records = entities
        .iter()
        .cloned()
        .map(PersistedEntityRecord::from)
        .collect::<Vec<_>>();
    save_persisted_entity_records(
        world_root,
        items,
        &PersistedEntityCheckpoint {
            lifecycle_clock: 0,
            regional_sequence_watermark: 0,
            records,
        },
    )
}

pub(crate) fn save_persisted_entity_records(
    world_root: &Path,
    items: &ItemRegistry,
    checkpoint: &PersistedEntityCheckpoint,
) -> Result<(), PlayerPersistenceError> {
    let path = entities_path(world_root);
    let lifecycle_tick = i64::try_from(checkpoint.lifecycle_clock).map_err(|_| {
        PlayerPersistenceError::InvalidValue {
            path: path.clone(),
            field: ENTITY_LIFECYCLE_TICK_FIELD,
        }
    })?;
    let regional_sequence =
        i64::try_from(checkpoint.regional_sequence_watermark).map_err(|_| {
            PlayerPersistenceError::InvalidValue {
                path: path.clone(),
                field: ENTITY_REGIONAL_SEQUENCE_FIELD,
            }
        })?;
    if !checkpoint.has_valid_temporal_state() {
        return Err(PlayerPersistenceError::InvalidValue {
            path,
            field: ENTITY_RETAINED_STATE_FIELD,
        });
    }
    if !checkpoint.has_valid_vehicle_graph() {
        return Err(PlayerPersistenceError::InvalidValue {
            path,
            field: ENTITY_VEHICLE_STATE_FIELD,
        });
    }
    let mut elements = Vec::new();
    for record in &checkpoint.records {
        validate_entity_numeric_state(&path, &record.snapshot)?;
        validate_entity_temporal_state(&path, checkpoint.lifecycle_clock, &record.snapshot)?;
        elements.push(entity_tag(&path, items, record)?);
    }
    let root = Tag::Compound(vec![
        (
            ENTITY_FORMAT_VERSION_FIELD.into(),
            Tag::Int(ENTITY_FORMAT_VERSION),
        ),
        (
            ENTITY_LIFECYCLE_TICK_FIELD.into(),
            Tag::Long(lifecycle_tick),
        ),
        (
            ENTITY_REGIONAL_SEQUENCE_FIELD.into(),
            Tag::Long(regional_sequence),
        ),
        (
            "Entities".into(),
            Tag::List(ListTag {
                element_type: if elements.is_empty() {
                    tag_type::END
                } else {
                    tag_type::COMPOUND
                },
                elements,
            }),
        ),
    ]);
    write_player_root(&path, "", &root)
}

pub(crate) fn load_world_metadata(
    world_root: &Path,
) -> Result<Option<WorldPersistedMetadata>, PlayerPersistenceError> {
    let path = world_metadata_path(world_root);
    if !path.is_file() {
        return Ok(None);
    }
    let (_, root) = read_player_root(&path)?;
    let Tag::Compound(fields) = root else {
        return Err(PlayerPersistenceError::RootNotCompound { path });
    };
    let players_sleeping_percentage = match long_field(&fields, "SolarisPlayersSleepingPercentage")
    {
        Some(value) => u32::try_from(value).map_err(|_| PlayerPersistenceError::InvalidValue {
            path: path.clone(),
            field: "SolarisPlayersSleepingPercentage",
        })?,
        None => 100,
    };
    let keep_inventory = match field(&fields, "SolarisKeepInventory") {
        Some(Tag::Byte(0)) | None => false,
        Some(Tag::Byte(1)) => true,
        _ => {
            return Err(PlayerPersistenceError::InvalidValue {
                path: path.clone(),
                field: "SolarisKeepInventory",
            });
        }
    };
    Ok(Some(WorldPersistedMetadata {
        world_time: long_field(&fields, "SolarisWorldTime").unwrap_or(0) as u64,
        players_sleeping_percentage,
        keep_inventory,
        world_identity: string_field(&fields, "SolarisWorldIdentity")
            .unwrap_or_default()
            .to_string(),
    }))
}

pub(crate) fn save_world_metadata(
    world_root: &Path,
    metadata: &WorldPersistedMetadata,
) -> Result<(), PlayerPersistenceError> {
    let path = world_metadata_path(world_root);
    let root = Tag::Compound(vec![
        (
            "SolarisWorldTime".into(),
            Tag::Long(metadata.world_time as i64),
        ),
        (
            "SolarisWorldIdentity".into(),
            Tag::String(metadata.world_identity.clone()),
        ),
        (
            "SolarisPlayersSleepingPercentage".into(),
            Tag::Long(i64::from(metadata.players_sleeping_percentage)),
        ),
        (
            "SolarisKeepInventory".into(),
            Tag::Byte(i8::from(metadata.keep_inventory)),
        ),
    ]);
    write_player_root(&path, "", &root)
}

pub(crate) fn world_identity(world_root: &Path) -> String {
    world_root.to_string_lossy().into_owned()
}

fn world_metadata_path(world_root: &Path) -> PathBuf {
    world_root.join(SOLARIS_DIR).join(WORLD_FILE)
}

fn entities_path(world_root: &Path) -> PathBuf {
    world_root.join(SOLARIS_DIR).join(ENTITIES_FILE)
}

fn entity_tag(
    path: &Path,
    items: &ItemRegistry,
    record: &PersistedEntityRecord,
) -> Result<Tag, PlayerPersistenceError> {
    let entity = &record.snapshot;
    let attributes = serde_json::to_string(&entity.attributes).map_err(|_| {
        PlayerPersistenceError::InvalidValue {
            path: path.to_path_buf(),
            field: ENTITY_ATTRIBUTES_FIELD,
        }
    })?;
    let retained = serde_json::to_string(&entity.retained).map_err(|_| {
        PlayerPersistenceError::InvalidValue {
            path: path.to_path_buf(),
            field: ENTITY_RETAINED_STATE_FIELD,
        }
    })?;
    let goal =
        serde_json::to_string(&entity.goal).map_err(|_| PlayerPersistenceError::InvalidValue {
            path: path.to_path_buf(),
            field: ENTITY_GOAL_STATE_FIELD,
        })?;
    let vehicle = serde_json::to_string(&entity.vehicle).map_err(|_| {
        PlayerPersistenceError::InvalidValue {
            path: path.to_path_buf(),
            field: ENTITY_VEHICLE_STATE_FIELD,
        }
    })?;
    let lifecycle = match entity.lifecycle {
        EntityLifecycle::Alive => 0,
        EntityLifecycle::Despawning => 1,
    };
    let mut fields = vec![
        ("id".into(), Tag::String(entity.type_name.clone())),
        ("SolarisEntityId".into(), Tag::Int(entity.id.0)),
        ("UUID".into(), Tag::IntArray(uuid_to_int_array(entity.uuid))),
        ("Pos".into(), vec3_double_list(entity.position)),
        (
            "Motion".into(),
            vec3_double_list(Vec3::new(
                entity.velocity.x / ENTITY_TICKS_PER_SECOND,
                entity.velocity.y / ENTITY_TICKS_PER_SECOND,
                entity.velocity.z / ENTITY_TICKS_PER_SECOND,
            )),
        ),
        (
            "Rotation".into(),
            Tag::List(ListTag {
                element_type: tag_type::FLOAT,
                elements: vec![
                    Tag::Float(entity.rotation.yaw),
                    Tag::Float(entity.rotation.pitch),
                ],
            }),
        ),
        ("OnGround".into(), Tag::Byte(i8::from(entity.on_ground))),
        (
            "FallDistance".into(),
            Tag::Float(entity.retained.fall_distance as f32),
        ),
        ("Health".into(), Tag::Float(entity.health)),
        (ENTITY_ATTRIBUTES_FIELD.into(), Tag::String(attributes)),
        (ENTITY_LIFECYCLE_FIELD.into(), Tag::Byte(lifecycle)),
        (ENTITY_RETAINED_STATE_FIELD.into(), Tag::String(retained)),
        (
            ENTITY_HEAD_YAW_FIELD.into(),
            Tag::Float(entity.rotation.head_yaw),
        ),
        (ENTITY_GOAL_STATE_FIELD.into(), Tag::String(goal)),
        (ENTITY_VEHICLE_STATE_FIELD.into(), Tag::String(vehicle)),
        ("SolarisLifetimeAge".into(), Tag::Int(record.age.max(0))),
    ];
    if let Some(animal) = entity.animal {
        fields.push(("Age".into(), Tag::Int(animal.age_ticks)));
        fields.push(("InLove".into(), Tag::Int(i32::from(animal.love_ticks))));
        if let Some(wool) = animal.sheep_wool {
            fields.push(("Color".into(), Tag::Byte(wool.color.id() as i8)));
            fields.push(("Sheared".into(), Tag::Byte(i8::from(wool.sheared))));
        }
    } else {
        fields.push(("Age".into(), Tag::Int(record.age.max(0))));
    }
    if let Some(ref stack) = entity.item_stack {
        let item = entity_item_stack_tag(items, stack)?;
        fields.push(("Item".into(), item));
        fields.push((
            "PickupDelay".into(),
            Tag::Short(record.pickup_delay.clamp(0, i32::from(i16::MAX)) as i16),
        ));
    }
    if let Some(value) = entity.experience_value {
        fields.push(("Value".into(), Tag::Int(value.max(0))));
    }
    if let Some(block_state) = entity.block_state {
        fields.push(("BlockState".into(), Tag::Int(block_state as i32)));
    }
    Ok(Tag::Compound(fields))
}

pub(super) fn entity_item_stack_tag(
    items: &ItemRegistry,
    stack: &EntityItemStack,
) -> Result<Tag, PlayerPersistenceError> {
    let name = items
        .name_of(stack.item_id)
        .ok_or_else(|| PlayerPersistenceError::UnknownItem(stack.item_id.to_string()))?;
    let mut fields = vec![
        ("id".into(), Tag::String(name.as_str().to_string())),
        ("count".into(), Tag::Int(stack.count)),
    ];
    if let Some(damage) = stack.damage {
        set_damage_component(&mut fields, damage);
    }
    if !stack.enchantments.is_empty() {
        set_enchantments_component(&mut fields, &stack.enchantments);
    }
    Ok(Tag::Compound(fields))
}

pub(super) fn read_entity_item_stack(
    fields: &[(String, Tag)],
    items: &ItemRegistry,
) -> Result<Option<EntityItemStack>, PlayerPersistenceError> {
    let Some(item_name) = string_field(fields, "id") else {
        return Ok(None);
    };
    let parsed = mc_data::Identifier::parse(item_name.to_string())
        .map_err(|_| PlayerPersistenceError::InvalidItemId(item_name.to_string()))?;
    let Some(item_id) = items.id_of(&parsed) else {
        return Err(PlayerPersistenceError::UnknownItem(item_name.to_string()));
    };
    let count = int_field(fields, "count").unwrap_or(1).max(0);
    Ok((count > 0).then_some(EntityItemStack {
        item_id,
        count,
        damage: damage_component(fields),
        enchantments: enchantments_component(fields)?,
    }))
}

fn vec3_double_list(vec: Vec3) -> Tag {
    Tag::List(ListTag {
        element_type: tag_type::DOUBLE,
        elements: vec![Tag::Double(vec.x), Tag::Double(vec.y), Tag::Double(vec.z)],
    })
}

fn uuid_to_int_array(uuid: uuid::Uuid) -> Vec<i32> {
    let bytes = uuid.as_u128().to_be_bytes();
    bytes
        .chunks_exact(4)
        .map(|chunk| i32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn uuid_field(fields: &[(String, Tag)]) -> Option<uuid::Uuid> {
    let Tag::IntArray(values) = field(fields, "UUID")? else {
        return None;
    };
    if values.len() != 4 {
        return None;
    }
    let mut bytes = [0_u8; 16];
    for (idx, value) in values.iter().enumerate() {
        bytes[idx * 4..idx * 4 + 4].copy_from_slice(&value.to_be_bytes());
    }
    Some(uuid::Uuid::from_u128(u128::from_be_bytes(bytes)))
}

fn playerdata_path(world_root: &Path, uuid: uuid::Uuid) -> PathBuf {
    world_root
        .join(PLAYERDATA_DIR)
        .join(format!("{}.dat", uuid.hyphenated()))
}

fn read_player_root(path: &Path) -> Result<(String, Tag), PlayerPersistenceError> {
    let file = File::open(path).map_err(|source| PlayerPersistenceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut decoder = GzDecoder::new(file);
    let mut bytes = Vec::new();
    decoder
        .read_to_end(&mut bytes)
        .map_err(|source| PlayerPersistenceError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let mut slice = bytes.as_slice();
    mc_nbt::read_named(&mut slice).map_err(|source| PlayerPersistenceError::Nbt {
        path: path.to_path_buf(),
        source,
    })
}

fn write_player_root(
    path: &Path,
    root_name: &str,
    root: &Tag,
) -> Result<(), PlayerPersistenceError> {
    if let Some(parent) = path.parent() {
        create_persistence_directory(parent)?;
    }
    let tmp_path = temporary_write_path(path);
    let file = File::create(&tmp_path).map_err(|source| PlayerPersistenceError::Io {
        path: tmp_path.clone(),
        source,
    })?;
    let mut encoder = GzEncoder::new(file, GzipCompression::default());
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, root_name, root).map_err(|source| {
        PlayerPersistenceError::Nbt {
            path: path.to_path_buf(),
            source,
        }
    })?;
    encoder
        .write_all(&bytes)
        .map_err(|source| PlayerPersistenceError::Io {
            path: tmp_path.clone(),
            source,
        })?;
    let file = encoder
        .finish()
        .map_err(|source| PlayerPersistenceError::Io {
            path: tmp_path.clone(),
            source,
        })?;
    file.sync_all()
        .map_err(|source| PlayerPersistenceError::Io {
            path: tmp_path.clone(),
            source,
        })?;
    std::fs::rename(&tmp_path, path).map_err(|source| PlayerPersistenceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn create_persistence_directory(path: &Path) -> Result<(), PlayerPersistenceError> {
    let existed = path.is_dir();
    std::fs::create_dir_all(path).map_err(|source| PlayerPersistenceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !existed && let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), PlayerPersistenceError> {
    let dir = File::open(path).map_err(|source| PlayerPersistenceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    dir.sync_all().map_err(|source| PlayerPersistenceError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), PlayerPersistenceError> {
    Ok(())
}

fn temporary_write_path(path: &Path) -> PathBuf {
    let counter = TMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("persisted.dat");
    path.with_file_name(format!("{file_name}.{pid}.{counter}.tmp"))
}

fn read_pose(fields: &[(String, Tag)]) -> Option<PlayerPose> {
    let pos = double_list::<3>(field(fields, "Pos")?, 3)?;
    let rotation = float_list::<2>(field(fields, "Rotation")?, 2)?;
    let mut pose = PlayerPose::new(pos[0], pos[1], pos[2]);
    pose.yaw = rotation[0];
    pose.pitch = rotation[1];
    pose.flags = MovePlayerFlags::new(byte_field(fields, "OnGround").unwrap_or(0) != 0, false);
    Some(pose)
}

fn read_spawn(fields: &[(String, Tag)]) -> Option<SpawnState> {
    Some(SpawnState {
        x: int_field(fields, "SpawnX")?,
        y: int_field(fields, "SpawnY")?,
        z: int_field(fields, "SpawnZ")?,
        angle: float_field(fields, "SpawnAngle").unwrap_or(0.0),
    })
}

fn pose_position_tag(pose: PlayerPose) -> Tag {
    Tag::List(ListTag {
        element_type: tag_type::DOUBLE,
        elements: vec![
            Tag::Double(pose.x),
            Tag::Double(pose.y),
            Tag::Double(pose.z),
        ],
    })
}

fn pose_rotation_tag(pose: PlayerPose) -> Tag {
    Tag::List(ListTag {
        element_type: tag_type::FLOAT,
        elements: vec![Tag::Float(pose.yaw), Tag::Float(pose.pitch)],
    })
}

fn inventory_tag(
    items: &ItemRegistry,
    state: &PlayerPersistedState,
) -> Result<Tag, PlayerPersistenceError> {
    let mut elements = Vec::new();
    for (slot, stack) in state.inventory.slots.iter().enumerate() {
        if stack.is_empty() {
            continue;
        }
        let name = items
            .name_of(stack.item_id)
            .ok_or_else(|| PlayerPersistenceError::UnknownItem(stack.item_id.to_string()))?;
        let mut fields = state.inventory_extras[slot]
            .as_ref()
            .filter(|extras| {
                extras.item_id == stack.item_id
                    && extras.damage == stack.damage
                    && extras.enchantments == stack.enchantments
                    && extras.custom_name == stack.custom_name
            })
            .map(|extras| extras.fields.clone())
            .unwrap_or_default();
        set_field(&mut fields, "Slot", Tag::Byte(slot as i8));
        set_item_stack_fields(&mut fields, name, stack);
        elements.push(Tag::Compound(fields));
    }
    Ok(Tag::List(ListTag {
        element_type: if elements.is_empty() {
            tag_type::END
        } else {
            tag_type::COMPOUND
        },
        elements,
    }))
}

fn item_stack_from_fields(
    items: &ItemRegistry,
    fields: &[(String, Tag)],
) -> Result<Option<ItemStack>, PlayerPersistenceError> {
    let Some(item_name) = string_field(fields, "id") else {
        return Ok(None);
    };
    let parsed = mc_data::Identifier::parse(item_name.to_string())
        .map_err(|_| PlayerPersistenceError::InvalidItemId(item_name.to_string()))?;
    let item_id = items
        .id_of(&parsed)
        .ok_or_else(|| PlayerPersistenceError::UnknownItem(item_name.to_string()))?;
    Ok(Some(ItemStack {
        count: int_field(fields, "count").unwrap_or(1).max(0),
        item_id,
        damage: damage_component(fields),
        enchantments: enchantments_component(fields)?,
        custom_name: custom_name_component(fields),
    }))
}

fn item_stack_tag(items: &ItemRegistry, stack: &ItemStack) -> Result<Tag, PlayerPersistenceError> {
    if stack.is_empty() {
        return Ok(Tag::Compound(Vec::new()));
    }
    let name = items
        .name_of(stack.item_id)
        .ok_or_else(|| PlayerPersistenceError::UnknownItem(stack.item_id.to_string()))?;
    let mut fields = Vec::new();
    set_item_stack_fields(&mut fields, name, stack);
    Ok(Tag::Compound(fields))
}

fn item_stack_projection_from_field<const N: usize>(
    items: &ItemRegistry,
    fields: &[(String, Tag)],
    name: &str,
) -> Result<Option<Box<[ItemStack; N]>>, PlayerPersistenceError> {
    let Some(Tag::List(list)) = field(fields, name) else {
        return Ok(None);
    };
    let mut stacks = std::array::from_fn(|_| ItemStack::EMPTY);
    for element in &list.elements {
        let Tag::Compound(item_fields) = element else {
            continue;
        };
        let Some(slot) = slot_field(item_fields).filter(|slot| *slot < N) else {
            continue;
        };
        if let Some(stack) = item_stack_from_fields(items, item_fields)? {
            stacks[slot] = stack;
        }
    }
    if stacks.iter().all(ItemStack::is_empty) {
        Ok(None)
    } else {
        Ok(Some(Box::new(stacks)))
    }
}

fn item_stack_projection_tag<const N: usize>(
    items: &ItemRegistry,
    projection: Option<&[ItemStack; N]>,
) -> Result<Tag, PlayerPersistenceError> {
    let mut elements = Vec::new();
    if let Some(projection) = projection {
        for (slot, stack) in projection.iter().enumerate() {
            if stack.is_empty() {
                continue;
            }
            let name = items
                .name_of(stack.item_id)
                .ok_or_else(|| PlayerPersistenceError::UnknownItem(stack.item_id.to_string()))?;
            let mut fields = vec![("Slot".into(), Tag::Byte(slot as i8))];
            set_item_stack_fields(&mut fields, name, stack);
            elements.push(Tag::Compound(fields));
        }
    }
    Ok(Tag::List(ListTag {
        element_type: if elements.is_empty() {
            tag_type::END
        } else {
            tag_type::COMPOUND
        },
        elements,
    }))
}

fn set_item_stack_fields(fields: &mut Vec<(String, Tag)>, name: &Identifier, stack: &ItemStack) {
    set_field(fields, "id", Tag::String(name.as_str().to_string()));
    set_field(fields, "count", Tag::Int(stack.count));
    if let Some(damage) = stack.damage {
        set_damage_component(fields, damage);
    }
    if let Some(custom_name) = &stack.custom_name {
        set_custom_name_component(fields, custom_name);
    }
    if !stack.enchantments.is_empty() {
        set_enchantments_component(fields, &stack.enchantments);
    }
}

fn damage_component(fields: &[(String, Tag)]) -> Option<i32> {
    let Tag::Compound(components) = field(fields, "components")? else {
        return None;
    };
    int_field(components, DAMAGE_COMPONENT)
}

fn set_damage_component(fields: &mut Vec<(String, Tag)>, damage: i32) {
    let components = field_mut(fields, "components").and_then(|tag| match tag {
        Tag::Compound(fields) => Some(fields),
        _ => None,
    });
    if let Some(components) = components {
        set_field(components, DAMAGE_COMPONENT, Tag::Int(damage));
    } else {
        set_field(
            fields,
            "components",
            Tag::Compound(vec![(DAMAGE_COMPONENT.into(), Tag::Int(damage))]),
        );
    }
}

/// The local 26.1.2 `DataComponents.CUSTOM_NAME` type is a Component. Solaris
/// emits and restores the literal text-component form used by script labels;
/// richer component data remains in `inventory_extras` untouched.
fn custom_name_component(fields: &[(String, Tag)]) -> Option<String> {
    let Tag::Compound(components) = field(fields, "components")? else {
        return None;
    };
    let Tag::Compound(component) = field(components, CUSTOM_NAME_COMPONENT)? else {
        return None;
    };
    let [(name, Tag::String(value))] = component.as_slice() else {
        return None;
    };
    (name == "text").then(|| value.clone())
}

fn set_custom_name_component(fields: &mut Vec<(String, Tag)>, custom_name: &str) {
    let value = Tag::Compound(vec![("text".into(), Tag::String(custom_name.to_owned()))]);
    let components = field_mut(fields, "components").and_then(|tag| match tag {
        Tag::Compound(fields) => Some(fields),
        _ => None,
    });
    if let Some(components) = components {
        set_field(components, CUSTOM_NAME_COMPONENT, value);
    } else {
        set_field(
            fields,
            "components",
            Tag::Compound(vec![(CUSTOM_NAME_COMPONENT.into(), value)]),
        );
    }
}

fn enchantments_component(
    fields: &[(String, Tag)],
) -> Result<Vec<mc_data::ItemEnchantment>, PlayerPersistenceError> {
    let Some(Tag::Compound(components)) = field(fields, "components") else {
        return Ok(Vec::new());
    };
    let Some(Tag::Compound(enchantments)) = field(components, ENCHANTMENTS_COMPONENT) else {
        return Ok(Vec::new());
    };
    let mut result = Vec::with_capacity(enchantments.len());
    for (id, level) in enchantments {
        let Tag::Int(level) = level else {
            return Err(PlayerPersistenceError::InvalidEnchantment(id.clone()));
        };
        let parsed = Identifier::parse(id.clone())
            .map_err(|_| PlayerPersistenceError::InvalidEnchantment(id.clone()))?;
        if !(1..=255).contains(level)
            || mc_data::required_registry_entry_id("enchantment", &parsed).is_none()
        {
            return Err(PlayerPersistenceError::InvalidEnchantment(id.clone()));
        }
        result.push(mc_data::ItemEnchantment {
            id: parsed,
            level: *level,
        });
    }
    result.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    Ok(result)
}

fn set_enchantments_component(
    fields: &mut Vec<(String, Tag)>,
    enchantments: &[mc_data::ItemEnchantment],
) {
    let value = Tag::Compound(
        enchantments
            .iter()
            .map(|enchantment| {
                (
                    enchantment.id.as_str().to_string(),
                    Tag::Int(enchantment.level),
                )
            })
            .collect(),
    );
    let components = field_mut(fields, "components").and_then(|tag| match tag {
        Tag::Compound(fields) => Some(fields),
        _ => None,
    });
    if let Some(components) = components {
        set_field(components, ENCHANTMENTS_COMPONENT, value);
    } else {
        set_field(
            fields,
            "components",
            Tag::Compound(vec![(ENCHANTMENTS_COMPONENT.into(), value)]),
        );
    }
}

fn is_modelled_item_key(name: &str) -> bool {
    matches!(name, "Slot" | "id" | "count")
}

fn slot_field(fields: &[(String, Tag)]) -> Option<usize> {
    match field(fields, "Slot")? {
        Tag::Byte(value) => Some(*value as u8 as usize),
        Tag::Short(value) => usize::try_from(*value).ok(),
        Tag::Int(value) => usize::try_from(*value).ok(),
        _ => None,
    }
}

fn field<'a>(fields: &'a [(String, Tag)], name: &str) -> Option<&'a Tag> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, tag)| tag)
}

fn field_mut<'a>(fields: &'a mut [(String, Tag)], name: &str) -> Option<&'a mut Tag> {
    fields
        .iter_mut()
        .find(|(key, _)| key == name)
        .map(|(_, tag)| tag)
}

fn set_field(fields: &mut Vec<(String, Tag)>, name: &str, value: Tag) {
    if let Some((_, existing)) = fields.iter_mut().find(|(key, _)| key == name) {
        *existing = value;
    } else {
        fields.push((name.into(), value));
    }
}

fn int_field(fields: &[(String, Tag)], name: &str) -> Option<i32> {
    match field(fields, name)? {
        Tag::Byte(value) => Some(i32::from(*value)),
        Tag::Short(value) => Some(i32::from(*value)),
        Tag::Int(value) => Some(*value),
        _ => None,
    }
}

fn long_field(fields: &[(String, Tag)], name: &str) -> Option<i64> {
    match field(fields, name)? {
        Tag::Byte(value) => Some(i64::from(*value)),
        Tag::Short(value) => Some(i64::from(*value)),
        Tag::Int(value) => Some(i64::from(*value)),
        Tag::Long(value) => Some(*value),
        _ => None,
    }
}

fn byte_field(fields: &[(String, Tag)], name: &str) -> Option<i8> {
    match field(fields, name)? {
        Tag::Byte(value) => Some(*value),
        _ => None,
    }
}

fn float_field(fields: &[(String, Tag)], name: &str) -> Option<f32> {
    match field(fields, name)? {
        Tag::Float(value) => Some(*value),
        Tag::Double(value) => Some(*value as f32),
        _ => None,
    }
}

fn string_field<'a>(fields: &'a [(String, Tag)], name: &str) -> Option<&'a str> {
    match field(fields, name)? {
        Tag::String(value) => Some(value),
        _ => None,
    }
}

fn double_list<const N: usize>(tag: &Tag, len: usize) -> Option<[f64; N]> {
    let Tag::List(list) = tag else {
        return None;
    };
    if list.elements.len() != len {
        return None;
    }
    let mut values = [0.0; N];
    for (idx, element) in list.elements.iter().enumerate() {
        values[idx] = match element {
            Tag::Double(value) => *value,
            Tag::Float(value) => f64::from(*value),
            _ => return None,
        };
    }
    Some(values)
}

fn float_list<const N: usize>(tag: &Tag, len: usize) -> Option<[f32; N]> {
    let Tag::List(list) = tag else {
        return None;
    };
    if list.elements.len() != len {
        return None;
    }
    let mut values = [0.0; N];
    for (idx, element) in list.elements.iter().enumerate() {
        values[idx] = match element {
            Tag::Float(value) => *value,
            Tag::Double(value) => *value as f32,
            _ => return None,
        };
    }
    Some(values)
}

impl fmt::Display for PlayerPersistedState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pos=({:.2},{:.2},{:.2}) mode={:?} health={:.1} food={} selected_slot={}",
            self.pose.x,
            self.pose.y,
            self.pose.z,
            self.game_mode,
            self.survival.health,
            self.survival.food,
            self.selected_hotbar_slot,
        )
    }
}

#[cfg(test)]
#[path = "persistence_entity_load_tests.rs"]
mod entity_load_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use mc_entity::{
        EntityStore, RegionPhase, RegionalCommitDecision, RegionalDecisionJournal, SpawnEntity,
        VehicleKind, VehicleState,
    };
    use mc_nbt::Tag;

    fn items() -> ItemRegistry {
        let reports = vec![
            mc_data::items::ItemReport {
                id: mc_data::Identifier::parse("minecraft:stone").unwrap(),
                protocol_id: 1,
            },
            mc_data::items::ItemReport {
                id: mc_data::Identifier::parse("minecraft:iron_pickaxe").unwrap(),
                protocol_id: 2,
            },
        ];
        ItemRegistry::from_report(&reports)
    }

    fn entity_types() -> EntityTypeRegistry {
        mc_data::entity_types::solaris_required_entity_types()
    }

    fn replay_entity(type_name: &str, spawn_tick: u64) -> EntitySnapshot {
        let mut store = EntityStore::new();
        let mut entity = SpawnEntity::new(1, type_name, Vec3::ZERO);
        entity.retained.spawn_tick = spawn_tick;
        let id = store.spawn(entity);
        store.snapshot(id).expect("spawned replay fixture")
    }

    #[test]
    fn regional_decision_journal_round_trips_complete_delta_and_clears() {
        let tmp = tempfile::tempdir().unwrap();
        let mut attributes = mc_entity::AttributeSet::vanilla_mob_defaults();
        attributes.set_base(
            mc_entity::AttributeKind::Custom("solaris:test".to_string()),
            7.25,
        );
        let snapshot = EntitySnapshot {
            id: EntityId(41),
            uuid: uuid::Uuid::from_u128(41),
            type_id: 30,
            type_name: "minecraft:cow".into(),
            position: Vec3::new(-3.5, 70.0, 8.25),
            rotation: mc_entity::Rotation {
                yaw: 12.5,
                pitch: -3.0,
                head_yaw: 9.0,
            },
            velocity: Vec3::new(0.1, -0.2, 0.3),
            on_ground: false,
            item_stack: None,
            experience_value: None,
            block_state: None,
            lifecycle: EntityLifecycle::Alive,
            health: 17.5,
            attributes,
            goal: GoalState::FollowPosition {
                target: Vec3::new(5.0, 71.0, -2.0),
                speed: 0.45,
            },
            vehicle: Some(VehicleState {
                kind: VehicleKind::Boat,
                passenger: Some(EntityId(42)),
            }),
            animal: Some(mc_entity::AnimalBreedingState::baby()),
            retained: mc_entity::EntityRetainedState::default(),
        };
        let decision = RegionalCommitDecision::from_parts_at_lifecycle_epoch(
            RegionPhase(7),
            91,
            13,
            vec![snapshot],
            vec![EntityId(99)],
        )
        .expect("valid decision");

        let (mut journal, pending) =
            FileRegionalDecisionJournal::open(tmp.path()).expect("open empty journal");
        assert!(pending.is_empty());
        serde_json::to_string(&decision).expect("decision is JSON serializable");
        journal.record_commit(&decision).expect("record decision");
        drop(journal);

        let (mut reopened, pending) =
            FileRegionalDecisionJournal::open(tmp.path()).expect("reopen journal");
        assert_eq!(pending, vec![decision.clone()]);
        let mut stale = decision.upserts()[0].clone();
        stale.position = Vec3::ZERO;
        let removed = EntitySnapshot {
            id: EntityId(99),
            uuid: uuid::Uuid::from_u128(99),
            ..stale.clone()
        };
        let replayed = replay_regional_commit_decisions(
            PersistedEntityCheckpoint::new(
                12,
                vec![
                    PersistedEntityRecord {
                        snapshot: stale,
                        age: 12,
                        pickup_delay: 4,
                    },
                    PersistedEntityRecord::from(removed),
                ],
            ),
            &pending,
        )
        .expect("valid regional decision replay");
        assert_eq!(replayed.lifecycle_clock, 13);
        assert_eq!(replayed.regional_sequence_watermark, 91);
        assert_eq!(replayed.records.len(), 1);
        assert_eq!(replayed.records[0].snapshot, decision.upserts()[0]);
        assert_eq!(replayed.records[0].age, 13);
        assert_eq!(replayed.records[0].pickup_delay, 0);
        reopened
            .clear_commit(decision.phase())
            .expect("clear decision");
        drop(reopened);
        let (_, pending) =
            FileRegionalDecisionJournal::open(tmp.path()).expect("reopen cleared journal");
        assert!(pending.is_empty());
    }

    #[test]
    fn replay_ignores_pre_checkpoint_decision_that_would_revert_spawn_and_timer() {
        let spawned = replay_entity("minecraft:item", 20);
        let mut checkpoint_timer = spawned.clone();
        checkpoint_timer.id = EntityId(2);
        checkpoint_timer.uuid = uuid::Uuid::from_u128(2);
        checkpoint_timer.retained.item_pickup_ready_tick = Some(30);
        let mut stale_timer = checkpoint_timer.clone();
        stale_timer.retained.item_pickup_ready_tick = Some(21);
        let stale_timer_decision = RegionalCommitDecision::from_parts_at_lifecycle_epoch(
            RegionPhase(1),
            9,
            19,
            vec![stale_timer],
            Vec::new(),
        )
        .expect("stale timer decision");
        let stale_spawn_removal = RegionalCommitDecision::from_parts_at_lifecycle_epoch(
            RegionPhase(2),
            10,
            19,
            Vec::new(),
            vec![spawned.id],
        )
        .expect("stale spawn removal");
        let checkpoint = PersistedEntityCheckpoint::new_at_owner_sequence(
            20,
            10,
            [
                PersistedEntityRecord::from(spawned),
                PersistedEntityRecord::from(checkpoint_timer.clone()),
            ],
        );

        let replayed = replay_regional_commit_decisions(
            checkpoint,
            &[stale_timer_decision, stale_spawn_removal],
        )
        .expect("stale decisions are ignored");

        assert_eq!(replayed.records.len(), 2);
        assert_eq!(replayed.records[1].snapshot, checkpoint_timer);
    }

    #[test]
    fn replay_applies_post_checkpoint_decision_from_the_same_lifecycle_epoch() {
        let checkpoint_entity = replay_entity("minecraft:item", 20);
        let decision = RegionalCommitDecision::from_parts_at_lifecycle_epoch(
            RegionPhase(1),
            11,
            20,
            Vec::new(),
            vec![checkpoint_entity.id],
        )
        .expect("post-checkpoint removal");
        let checkpoint = PersistedEntityCheckpoint::new_at_owner_sequence(
            20,
            10,
            [PersistedEntityRecord::from(checkpoint_entity)],
        );

        let replayed = replay_regional_commit_decisions(checkpoint, &[decision])
            .expect("newer same-epoch decision replays");

        assert!(replayed.records.is_empty());
        assert_eq!(replayed.lifecycle_clock, 20);
        assert_eq!(replayed.regional_sequence_watermark, 11);
    }

    #[test]
    fn regional_decision_journal_rejects_legacy_json_format() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join(SOLARIS_DIR)
            .join(REGIONAL_DECISION_JOURNAL_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, br#"{"version":1,"pending":[]}"#).unwrap();

        let Err(error) = FileRegionalDecisionJournal::open(tmp.path()) else {
            panic!("unreleased JSON journal files must fail closed");
        };

        assert!(matches!(
            error,
            RegionalDecisionJournalOpenError::UnsupportedVersion(0)
        ));
    }

    #[test]
    fn regional_decision_journal_compaction_retains_unacknowledged_append() {
        let tmp = tempfile::tempdir().unwrap();
        let first = RegionalCommitDecision::from_parts(RegionPhase(11), 11, Vec::new(), Vec::new())
            .expect("first decision");
        let later = RegionalCommitDecision::from_parts(RegionPhase(12), 12, Vec::new(), Vec::new())
            .expect("later decision");
        let (mut journal, pending) =
            FileRegionalDecisionJournal::open(tmp.path()).expect("open journal");
        assert!(pending.is_empty());
        journal
            .record_commit(&first)
            .expect("append first decision");
        journal
            .record_commit(&later)
            .expect("append later decision");

        journal
            .clear_commit(first.phase())
            .expect("compact acknowledged checkpoint decision");
        drop(journal);

        let (_, pending) = FileRegionalDecisionJournal::open(tmp.path()).expect("reopen journal");
        assert_eq!(pending, vec![later]);
    }

    #[test]
    fn regional_decision_journal_cleanup_requires_exact_durable_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let decision =
            RegionalCommitDecision::from_parts(RegionPhase(11), 41, Vec::new(), Vec::new())
                .expect("decision");
        let wrong_identity = RegionalCommitDecision::from_parts_at_lifecycle_epoch(
            RegionPhase(11),
            42,
            7,
            Vec::new(),
            Vec::new(),
        )
        .expect("different durable identity")
        .identity();
        let (mut journal, _) = FileRegionalDecisionJournal::open(tmp.path()).unwrap();
        journal.record_commit(&decision).unwrap();

        journal
            .clear_commit_identities(&[wrong_identity])
            .expect("non-matching cleanup is a no-op");
        assert_eq!(journal.pending, vec![decision.clone()]);
        journal
            .clear_commit_identities(&[decision.identity()])
            .expect("matching cleanup removes the decision");
        assert!(journal.pending.is_empty());
    }

    #[test]
    fn regional_decision_checkpoint_cleanup_does_not_queue_a_rewrite_before_next_append() {
        let checkpointed =
            RegionalCommitDecision::from_parts(RegionPhase(11), 41, Vec::new(), Vec::new())
                .expect("checkpointed decision");
        let later = RegionalCommitDecision::from_parts(RegionPhase(12), 42, Vec::new(), Vec::new())
            .expect("later decision");
        let expected_later = later.clone();
        let (requests, receiver) = std::sync::mpsc::sync_channel(2);
        let writer = std::thread::spawn(move || {
            match receiver.recv().expect("first writer request after cleanup") {
                RegionalJournalWriteRequest::Append { decisions, reply } => {
                    assert_eq!(decisions, vec![expected_later]);
                    reply.send(Ok(())).expect("append completion");
                }
                RegionalJournalWriteRequest::Replace { reply, .. } => {
                    reply.send(Ok(())).expect("rewrite completion");
                    panic!("checkpoint cleanup queued a WAL rewrite before the next append");
                }
                RegionalJournalWriteRequest::Shutdown { reply } => {
                    reply.send(()).expect("shutdown completion");
                    panic!("checkpoint cleanup shut down the writer");
                }
            }
        });
        let mut journal = FileRegionalDecisionJournal {
            path: PathBuf::from("memory-only-checkpoint-cleanup"),
            pending: vec![checkpointed.clone()],
            needs_compaction: false,
            requests,
            worker: None,
        };

        journal
            .clear_commit_identities(&[checkpointed.identity()])
            .expect("acknowledge durable checkpoint");
        journal
            .record_commit(&later)
            .expect("append later decision");
        writer.join().expect("writer assertion");
    }

    #[test]
    fn crash_before_shutdown_compaction_replays_old_wal_through_checkpoint_watermark() {
        let tmp = tempfile::tempdir().unwrap();
        let decision =
            RegionalCommitDecision::from_parts(RegionPhase(11), 41, Vec::new(), Vec::new())
                .expect("checkpointed decision");
        let (mut journal, _) = FileRegionalDecisionJournal::open(tmp.path()).expect("open journal");
        journal.record_commit(&decision).expect("append decision");
        journal
            .clear_commit_identities(&[decision.identity()])
            .expect("acknowledge durable checkpoint");

        let worker = journal.worker.take().expect("journal writer");
        let (reply, completion) = std::sync::mpsc::channel();
        journal
            .requests
            .send(RegionalJournalWriteRequest::Shutdown { reply })
            .expect("stop writer without compaction");
        completion.recv().expect("writer stopped");
        worker.join().expect("join writer");
        drop(journal);

        let (reopened, pending) =
            FileRegionalDecisionJournal::open(tmp.path()).expect("reopen uncompacted journal");
        assert_eq!(pending, vec![decision]);
        let checkpoint = PersistedEntityCheckpoint::new_at_owner_sequence(
            41,
            41,
            Vec::<PersistedEntityRecord>::new(),
        );
        let replayed = replay_regional_commit_decisions(checkpoint, &pending)
            .expect("checkpoint watermark ignores old WAL record");
        assert!(replayed.records.is_empty());
        assert_eq!(replayed.regional_sequence_watermark, 41);
        drop(reopened);
    }

    #[test]
    fn regional_decision_journal_group_commit_persists_every_decision() {
        let tmp = tempfile::tempdir().unwrap();
        let decisions = [
            RegionalCommitDecision::from_parts(RegionPhase(11), 11, Vec::new(), Vec::new())
                .expect("first grouped decision"),
            RegionalCommitDecision::from_parts(RegionPhase(12), 12, Vec::new(), Vec::new())
                .expect("second grouped decision"),
        ];
        let (mut journal, _) =
            FileRegionalDecisionJournal::open(tmp.path()).expect("open grouped journal");

        journal
            .record_commits(&decisions)
            .expect("durable grouped decisions");
        drop(journal);

        let (_, pending) =
            FileRegionalDecisionJournal::open(tmp.path()).expect("reopen grouped journal");
        assert_eq!(pending, decisions);
    }

    #[test]
    fn regional_decision_journal_preserves_unknown_append_outcome() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("journal-test");
        let (requests, receiver) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let RegionalJournalWriteRequest::Append { reply, .. } =
                receiver.recv().expect("append request")
            else {
                panic!("expected append request");
            };
            reply
                .send(Err(RegionalDecisionJournalError::OUTCOME_UNKNOWN))
                .expect("append completion");
        });
        let mut journal = FileRegionalDecisionJournal {
            path,
            pending: Vec::new(),
            needs_compaction: false,
            requests,
            worker: Some(worker),
        };
        let decision =
            RegionalCommitDecision::from_parts(RegionPhase(1), 1, Vec::new(), Vec::new())
                .expect("decision");

        let error = journal
            .record_commit(&decision)
            .expect_err("unknown durability outcome");
        assert!(error.outcome_unknown());
        assert_eq!(journal.pending, vec![decision]);
    }

    #[test]
    #[ignore = "explicit local filesystem durability latency benchmark"]
    fn regional_decision_journal_fsync_latency_report() {
        const ITERATIONS: usize = 40;

        fn percentile(samples: &[u128], percentile: usize) -> u128 {
            samples[(samples.len() * percentile).div_ceil(100).saturating_sub(1)]
        }

        let tmp = tempfile::tempdir().expect("journal benchmark directory");
        let (mut journal, pending) =
            FileRegionalDecisionJournal::open(tmp.path()).expect("open benchmark journal");
        assert!(pending.is_empty());
        let snapshot = EntitySnapshot {
            id: EntityId(1),
            uuid: uuid::Uuid::from_u128(1),
            type_id: 30,
            type_name: "minecraft:cow".into(),
            position: Vec3::new(0.5, 64.0, 0.5),
            rotation: mc_entity::Rotation::ZERO,
            velocity: Vec3::ZERO,
            on_ground: true,
            item_stack: None,
            experience_value: None,
            block_state: None,
            lifecycle: EntityLifecycle::Alive,
            health: 20.0,
            attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
            goal: GoalState::Idle,
            vehicle: None,
            animal: Some(mc_entity::AnimalBreedingState::adult()),
            retained: mc_entity::EntityRetainedState::default(),
        };
        let mut record_samples = Vec::with_capacity(ITERATIONS);
        let mut clear_samples = Vec::with_capacity(ITERATIONS);
        let mut total_samples = Vec::with_capacity(ITERATIONS);
        for iteration in 0..ITERATIONS {
            let decision = RegionalCommitDecision::from_parts(
                RegionPhase(iteration as u64 + 1),
                iteration as u64 + 1,
                vec![snapshot.clone()],
                Vec::new(),
            )
            .expect("benchmark decision");
            let total_started = std::time::Instant::now();
            let record_started = std::time::Instant::now();
            journal
                .record_commit(&decision)
                .expect("durable benchmark commit");
            record_samples.push(record_started.elapsed().as_micros());
            let clear_started = std::time::Instant::now();
            journal
                .clear_commit(decision.phase())
                .expect("durable benchmark clear");
            clear_samples.push(clear_started.elapsed().as_micros());
            total_samples.push(total_started.elapsed().as_micros());
        }
        record_samples.sort_unstable();
        clear_samples.sort_unstable();
        total_samples.sort_unstable();
        println!(
            "REGIONAL_JOURNAL_FSYNC_BENCH iterations={ITERATIONS} record_p50_us={} record_p95_us={} record_p99_us={} clear_p50_us={} clear_p95_us={} clear_p99_us={} total_p50_us={} total_p95_us={} total_p99_us={} total_max_us={}",
            percentile(&record_samples, 50),
            percentile(&record_samples, 95),
            percentile(&record_samples, 99),
            percentile(&clear_samples, 50),
            percentile(&clear_samples, 95),
            percentile(&clear_samples, 99),
            percentile(&total_samples, 50),
            percentile(&total_samples, 95),
            percentile(&total_samples, 99),
            total_samples.last().copied().unwrap_or_default(),
        );
    }

    #[test]
    fn player_state_round_trips_through_real_playerdata_path() {
        let tmp = tempfile::tempdir().unwrap();
        let items = items();
        let uuid = uuid::Uuid::from_u128(0x1234);
        let mut state = PlayerPersistedState::new_default(PlayerPose::new(1.5, 65.0, -2.5));
        state.pose.yaw = 90.0;
        state.pose.pitch = 12.0;
        state.game_mode = GameMode::Adventure;
        state.survival.health = 7.5;
        state.survival.food = 9;
        state.survival.saturation = 2.5;
        state.selected_hotbar_slot = 3;
        state
            .inventory
            .set_hotbar(3, ItemStack::new(1, 17))
            .unwrap();
        state.carried_item = ItemStack::new(1, 3);
        let mut crafting_table_input = std::array::from_fn(|_| ItemStack::EMPTY);
        crafting_table_input[0] = ItemStack::new(1, 4);
        crafting_table_input[8] = ItemStack::new(2, 1);
        state.crafting_table_input = Some(Box::new(crafting_table_input.clone()));
        let mut enchanting_table_input = std::array::from_fn(|_| ItemStack::EMPTY);
        enchanting_table_input[0] = ItemStack::new(2, 1);
        enchanting_table_input[1] = ItemStack::new(1, 7);
        state.enchanting_table_input = Some(Box::new(enchanting_table_input.clone()));
        let efficiency = Identifier::parse("minecraft:efficiency").unwrap();
        state.inventory.slots[9] = ItemStack::new(2, 1)
            .with_damage(11)
            .with_enchantment(efficiency.clone(), 1)
            .with_custom_name("Named Pickaxe");

        save_player_state(tmp.path(), uuid, &items, &state).unwrap();

        let loaded = load_player_state(
            tmp.path(),
            uuid,
            &items,
            PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5)),
        )
        .unwrap()
        .unwrap();

        assert_eq!(loaded.pose.x, 1.5);
        assert_eq!(loaded.pose.z, -2.5);
        assert_eq!(loaded.pose.yaw, 90.0);
        assert_eq!(loaded.game_mode, GameMode::Adventure);
        assert_eq!(loaded.survival.health, 7.5);
        assert_eq!(loaded.survival.food, 9);
        assert_eq!(loaded.selected_hotbar_slot, 3);
        assert_eq!(loaded.inventory.held(3), Some(&ItemStack::new(1, 17)));
        assert_eq!(loaded.carried_item, ItemStack::new(1, 3));
        assert_eq!(
            loaded.crafting_table_input.as_deref(),
            Some(&crafting_table_input)
        );
        assert_eq!(
            loaded.enchanting_table_input.as_deref(),
            Some(&enchanting_table_input)
        );
        assert_eq!(
            loaded.inventory.slots[9],
            ItemStack::new(2, 1)
                .with_damage(11)
                .with_enchantment(efficiency, 1)
                .with_custom_name("Named Pickaxe")
        );
    }

    #[test]
    fn player_state_preserves_unknown_root_and_item_payloads() {
        let tmp = tempfile::tempdir().unwrap();
        let items = items();
        let uuid = uuid::Uuid::from_u128(0x5678);
        let path = playerdata_path(tmp.path(), uuid);
        let root = Tag::Compound(vec![
            ("SolarisUnknownRoot".into(), Tag::String("keep".into())),
            (
                "Inventory".into(),
                Tag::List(ListTag {
                    element_type: tag_type::COMPOUND,
                    elements: vec![Tag::Compound(vec![
                        ("Slot".into(), Tag::Byte(36)),
                        ("id".into(), Tag::String("minecraft:stone".into())),
                        ("count".into(), Tag::Int(4)),
                        ("SolarisUnknownItem".into(), Tag::Long(99)),
                    ])],
                }),
            ),
        ]);
        write_player_root(&path, "", &root).unwrap();

        let mut loaded = load_player_state(
            tmp.path(),
            uuid,
            &items,
            PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5)),
        )
        .unwrap()
        .unwrap();
        loaded
            .inventory
            .set_hotbar(0, ItemStack::new(1, 5))
            .unwrap();
        save_player_state(tmp.path(), uuid, &items, &loaded).unwrap();

        let (_, saved) = read_player_root(&path).unwrap();
        let Tag::Compound(fields) = saved else {
            panic!("root compound");
        };
        assert_eq!(string_field(&fields, "SolarisUnknownRoot"), Some("keep"));
        let Some(Tag::List(list)) = field(&fields, "Inventory") else {
            panic!("inventory list");
        };
        let Tag::Compound(slot) = &list.elements[0] else {
            panic!("inventory item");
        };
        assert_eq!(int_field(slot, "count"), Some(5));
        assert_eq!(field(slot, "SolarisUnknownItem"), Some(&Tag::Long(99)));
    }

    #[test]
    fn player_state_rejects_non_finite_numeric_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let uuid = uuid::Uuid::from_u128(0x9911);
        let path = playerdata_path(tmp.path(), uuid);
        let root = Tag::Compound(vec![
            (
                "Pos".into(),
                Tag::List(ListTag {
                    element_type: tag_type::DOUBLE,
                    elements: vec![Tag::Double(f64::NAN), Tag::Double(64.0), Tag::Double(0.0)],
                }),
            ),
            (
                "Rotation".into(),
                Tag::List(ListTag {
                    element_type: tag_type::FLOAT,
                    elements: vec![Tag::Float(0.0), Tag::Float(0.0)],
                }),
            ),
            (
                ENTITY_ATTRIBUTES_FIELD.into(),
                Tag::String(
                    serde_json::to_string(&mc_entity::AttributeSet::vanilla_mob_defaults())
                        .unwrap(),
                ),
            ),
            (ENTITY_LIFECYCLE_FIELD.into(), Tag::Byte(0)),
            (
                ENTITY_RETAINED_STATE_FIELD.into(),
                Tag::String(
                    serde_json::to_string(&mc_entity::EntityRetainedState::default()).unwrap(),
                ),
            ),
        ]);
        write_player_root(&path, "", &root).unwrap();

        let error = load_player_state(
            tmp.path(),
            uuid,
            &items(),
            PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5)),
        )
        .expect_err("non-finite player pose must fail closed");

        assert!(matches!(
            error,
            PlayerPersistenceError::InvalidNumeric { .. }
        ));
    }

    #[test]
    fn persisted_entities_reject_non_finite_numeric_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let path = entities_path(tmp.path());
        let mut item = replay_entity("minecraft:item", 0);
        item.type_id = 71;
        item.item_stack = Some(EntityItemStack::new(1, 1));
        save_persisted_entities(tmp.path(), &items(), &[item]).unwrap();
        let (_, mut root) = read_player_root(&path).unwrap();
        let Tag::Compound(root_fields) = &mut root else {
            panic!("saved entity root must be a compound");
        };
        let Some(Tag::List(entities)) = field_mut(root_fields, "Entities") else {
            panic!("saved entity root must contain entities");
        };
        let Tag::Compound(entity_fields) = &mut entities.elements[0] else {
            panic!("saved entity must be a compound");
        };
        set_field(
            entity_fields,
            "Pos",
            Tag::List(ListTag {
                element_type: tag_type::DOUBLE,
                elements: vec![
                    Tag::Double(0.0),
                    Tag::Double(f64::INFINITY),
                    Tag::Double(0.0),
                ],
            }),
        );
        write_player_root(&path, "", &root).unwrap();

        let error = load_persisted_entities(tmp.path(), &items(), &entity_types())
            .expect_err("non-finite entity pose must fail closed");

        assert!(matches!(
            error,
            PlayerPersistenceError::InvalidNumeric { .. }
        ));
    }

    #[test]
    fn entities_round_trip_through_real_storage_path() {
        let tmp = tempfile::tempdir().unwrap();
        let items = items();
        let entity_types = entity_types();
        let item = EntitySnapshot {
            id: EntityId(100),
            uuid: uuid::Uuid::from_u128(100),
            type_id: 71,
            type_name: "minecraft:item".into(),
            position: Vec3::new(1.0, 65.0, 2.0),
            rotation: mc_entity::Rotation::ZERO,
            velocity: Vec3::new(0.1, 0.2, 0.3),
            on_ground: false,
            item_stack: Some(EntityItemStack::new(1, 3)),
            experience_value: None,
            block_state: None,
            lifecycle: EntityLifecycle::Alive,
            health: 20.0,
            attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
            goal: GoalState::Idle,
            vehicle: None,
            animal: None,
            retained: mc_entity::EntityRetainedState::default(),
        };
        let cow = EntitySnapshot {
            id: EntityId(101),
            uuid: uuid::Uuid::from_u128(101),
            type_id: 30,
            type_name: "minecraft:cow".into(),
            position: Vec3::new(-4.0, 64.0, 8.0),
            rotation: mc_entity::Rotation {
                yaw: 45.0,
                pitch: 0.0,
                head_yaw: 45.0,
            },
            velocity: Vec3::ZERO,
            on_ground: true,
            item_stack: None,
            experience_value: None,
            block_state: None,
            lifecycle: EntityLifecycle::Alive,
            health: 13.0,
            attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
            goal: GoalState::Wander {
                speed: 0.8,
                period_ticks: 80,
            },
            vehicle: None,
            animal: Some(mc_entity::AnimalBreedingState {
                age_ticks: mc_entity::BABY_START_AGE_TICKS,
                love_ticks: 321,
                sheep_wool: None,
            }),
            retained: mc_entity::EntityRetainedState::default(),
        };
        let falling_block = EntitySnapshot {
            id: EntityId(102),
            uuid: uuid::Uuid::from_u128(102),
            type_id: 51,
            type_name: "minecraft:falling_block".into(),
            position: Vec3::new(3.5, 70.0, 4.5),
            rotation: mc_entity::Rotation::ZERO,
            velocity: Vec3::ZERO,
            on_ground: false,
            item_stack: None,
            experience_value: None,
            block_state: Some(1234),
            lifecycle: EntityLifecycle::Alive,
            health: 20.0,
            attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
            goal: GoalState::Idle,
            vehicle: None,
            animal: None,
            retained: mc_entity::EntityRetainedState::default(),
        };
        let chicken = EntitySnapshot {
            id: EntityId(103),
            uuid: uuid::Uuid::from_u128(103),
            type_id: 26,
            type_name: "minecraft:chicken".into(),
            position: Vec3::new(6.0, 64.0, -2.0),
            rotation: mc_entity::Rotation::ZERO,
            velocity: Vec3::ZERO,
            on_ground: true,
            item_stack: None,
            experience_value: None,
            block_state: None,
            lifecycle: EntityLifecycle::Alive,
            health: 4.0,
            attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
            goal: GoalState::Wander {
                speed: 0.8,
                period_ticks: 80,
            },
            vehicle: None,
            animal: Some(mc_entity::AnimalBreedingState {
                age_ticks: -120,
                love_ticks: 87,
                sheep_wool: None,
            }),
            retained: mc_entity::EntityRetainedState::default(),
        };

        save_persisted_entities(
            tmp.path(),
            &items,
            &[
                item.clone(),
                cow.clone(),
                falling_block.clone(),
                chicken.clone(),
            ],
        )
        .unwrap();
        let loaded = load_persisted_entities(tmp.path(), &items, &entity_types)
            .unwrap()
            .records;

        assert_eq!(loaded.len(), 4);
        assert_eq!(loaded[0].id, item.id);
        assert_eq!(loaded[0].uuid, item.uuid);
        assert_eq!(loaded[0].position, item.position);
        assert_eq!(loaded[1].animal, cow.animal);
        assert_eq!(loaded[0].velocity, item.velocity);
        assert_eq!(loaded[0].item_stack, item.item_stack);
        assert_eq!(loaded[1].id, cow.id);
        assert_eq!(loaded[1].type_name, cow.type_name);
        assert_eq!(loaded[1].health, cow.health);
        assert_eq!(loaded[1].position, cow.position);
        assert_eq!(
            loaded[1].attributes.base(&AttributeKind::MovementSpeed),
            cow.attributes.base(&AttributeKind::MovementSpeed)
        );
        assert_eq!(loaded[2].type_name, falling_block.type_name);
        assert_eq!(loaded[2].block_state, falling_block.block_state);
        assert!(matches!(loaded[2].goal, GoalState::Idle));
        assert_eq!(loaded[3].type_name, chicken.type_name);
        assert_eq!(loaded[3].animal, chicken.animal);
    }

    #[test]
    fn sheep_wool_state_round_trips_through_entity_storage() {
        let tmp = tempfile::tempdir().unwrap();
        let items = items();
        let mut animal = mc_entity::AnimalBreedingState::adult_sheep(mc_entity::SheepColor::Brown);
        animal.sheep_wool.as_mut().unwrap().sheared = true;
        let sheep = EntitySnapshot {
            id: EntityId(104),
            uuid: uuid::Uuid::from_u128(104),
            type_id: 111,
            type_name: "minecraft:sheep".into(),
            position: Vec3::new(4.0, 64.0, 5.0),
            rotation: mc_entity::Rotation::ZERO,
            velocity: Vec3::ZERO,
            on_ground: true,
            item_stack: None,
            experience_value: None,
            block_state: None,
            lifecycle: EntityLifecycle::Alive,
            health: 8.0,
            attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
            goal: GoalState::Idle,
            vehicle: None,
            animal: Some(animal),
            retained: mc_entity::EntityRetainedState::default(),
        };

        save_persisted_entities(tmp.path(), &items, std::slice::from_ref(&sheep)).unwrap();
        let loaded = load_persisted_entities(tmp.path(), &items, &entity_types())
            .unwrap()
            .records;

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].animal, sheep.animal);
    }

    #[test]
    fn entity_item_stack_persistence_preserves_modelled_components() {
        let items = items();
        let efficiency = Identifier::parse("minecraft:efficiency").unwrap();
        let stack = EntityItemStack::new(2, 1)
            .with_damage(17)
            .with_enchantment(efficiency.clone(), 1);

        let tag = entity_item_stack_tag(&items, &stack).unwrap();
        let Tag::Compound(fields) = tag else {
            panic!("item stack compound");
        };
        let Some(Tag::Compound(components)) = field(&fields, "components") else {
            panic!("components compound");
        };
        assert_eq!(int_field(components, DAMAGE_COMPONENT), Some(17));
        let Some(Tag::Compound(enchantments)) = field(components, ENCHANTMENTS_COMPONENT) else {
            panic!("enchantments compound");
        };
        assert_eq!(int_field(enchantments, efficiency.as_str()), Some(1));

        let loaded = read_entity_item_stack(&fields, &items).unwrap().unwrap();
        assert_eq!(loaded, stack);
    }

    #[test]
    fn item_entity_lifecycle_fields_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let items = items();
        let entity_types = entity_types();
        let mut retained = mc_entity::EntityRetainedState::default();
        retained.item_pickup_ready_tick = Some(130);
        let record = PersistedEntityRecord {
            snapshot: EntitySnapshot {
                id: EntityId(104),
                uuid: uuid::Uuid::from_u128(104),
                type_id: 71,
                type_name: "minecraft:item".into(),
                position: Vec3::new(1.0, 65.0, 2.0),
                rotation: mc_entity::Rotation::ZERO,
                velocity: Vec3::ZERO,
                on_ground: false,
                item_stack: Some(EntityItemStack::new(1, 3)),
                experience_value: None,
                block_state: None,
                lifecycle: EntityLifecycle::Alive,
                health: 20.0,
                attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
                goal: GoalState::Idle,
                vehicle: None,
                animal: None,
                retained,
            },
            age: 123,
            pickup_delay: 7,
        };

        save_persisted_entity_records(
            tmp.path(),
            &items,
            &PersistedEntityCheckpoint::new(123, vec![record.clone()]),
        )
        .unwrap();
        let loaded = load_persisted_entities(tmp.path(), &items, &entity_types)
            .unwrap()
            .records;

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, record.id);
        assert_eq!(loaded[0].age, 123);
        assert_eq!(loaded[0].pickup_delay, 7);
    }

    #[test]
    fn restored_aquatic_entities_keep_persisted_goal() {
        let tmp = tempfile::tempdir().unwrap();
        let items = items();
        let entity_types = entity_types();
        let cod = EntitySnapshot {
            id: EntityId(102),
            uuid: uuid::Uuid::from_u128(102),
            type_id: 27,
            type_name: "minecraft:cod".into(),
            position: Vec3::new(1.0, 50.0, 2.0),
            rotation: mc_entity::Rotation::ZERO,
            velocity: Vec3::ZERO,
            on_ground: true,
            item_stack: None,
            experience_value: None,
            block_state: None,
            lifecycle: EntityLifecycle::Alive,
            health: 3.0,
            attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
            goal: GoalState::Idle,
            vehicle: None,
            animal: None,
            retained: mc_entity::EntityRetainedState::default(),
        };

        save_persisted_entities(tmp.path(), &items, &[cod]).unwrap();
        let loaded = load_persisted_entities(tmp.path(), &items, &entity_types)
            .unwrap()
            .records;

        assert_eq!(loaded[0].goal, GoalState::Idle);
        assert!(!loaded[0].on_ground);
    }

    #[test]
    fn concurrent_entity_saves_use_distinct_temp_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let items = items();
        let entity_types = entity_types();
        let item = EntitySnapshot {
            id: EntityId(100),
            uuid: uuid::Uuid::from_u128(100),
            type_id: 71,
            type_name: "minecraft:item".into(),
            position: Vec3::new(1.0, 65.0, 2.0),
            rotation: mc_entity::Rotation::ZERO,
            velocity: Vec3::ZERO,
            on_ground: false,
            item_stack: Some(EntityItemStack::new(1, 3)),
            experience_value: None,
            block_state: None,
            lifecycle: EntityLifecycle::Alive,
            health: 20.0,
            attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
            goal: GoalState::Idle,
            vehicle: None,
            animal: None,
            retained: mc_entity::EntityRetainedState::default(),
        };

        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for _ in 0..8 {
                let root = &root;
                let items = &items;
                let item = &item;
                handles.push(scope.spawn(move || {
                    for _ in 0..25 {
                        save_persisted_entities(root, items, std::slice::from_ref(item)).unwrap();
                    }
                }));
            }
            for handle in handles {
                handle.join().unwrap();
            }
        });

        let loaded = load_persisted_entities(&root, &items, &entity_types)
            .unwrap()
            .records;
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, item.id);
    }

    #[test]
    fn xp_state_adds_points_and_maps_to_wire_packet() {
        let mut xp = XpState::default();

        assert!(!xp.add_points(0));
        assert!(xp.add_points(9));

        assert_eq!(xp.total, 9);
        assert_eq!(xp.level, 1);
        assert!((xp.progress - (2.0 / 9.0)).abs() < f32::EPSILON);
        assert_eq!(
            xp.as_packet(),
            ClientboundSetExperience {
                experience_progress: xp.progress,
                total_experience: 9,
                experience_level: 1,
            }
        );
    }

    #[test]
    fn xp_state_uses_vanilla_level_costs_across_multiple_levels() {
        let mut xp = XpState::default();

        assert!(xp.add_points(30));

        assert_eq!(xp.total, 30);
        assert_eq!(xp.level, 3);
        assert!((xp.progress - (3.0 / 13.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn enchanting_spends_levels_without_reducing_total_experience() {
        let mut xp = XpState::default();
        assert!(xp.add_points(30));

        assert!(xp.spend_enchantment_levels(2, 0x1234_5678));

        assert_eq!(xp.level, 1);
        assert!((xp.progress - (3.0 / 13.0)).abs() < f32::EPSILON);
        assert_eq!(xp.total, 30);
        assert_eq!(xp.seed, 0x1234_5678);
        assert!(!xp.spend_enchantment_levels(2, 7));
    }

    #[test]
    fn world_metadata_round_trips_through_real_storage_path() {
        let tmp = tempfile::tempdir().unwrap();
        let metadata = WorldPersistedMetadata {
            world_time: 12345,
            players_sleeping_percentage: 50,
            keep_inventory: true,
            world_identity: world_identity(tmp.path()),
        };

        save_world_metadata(tmp.path(), &metadata).unwrap();
        let loaded = load_world_metadata(tmp.path()).unwrap().unwrap();

        assert_eq!(loaded, metadata);
    }

    #[test]
    fn legacy_world_metadata_defaults_sleeping_percentage_to_vanilla_value() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Tag::Compound(vec![
            ("SolarisWorldTime".into(), Tag::Long(77)),
            (
                "SolarisWorldIdentity".into(),
                Tag::String(world_identity(tmp.path())),
            ),
        ]);
        write_player_root(&world_metadata_path(tmp.path()), "", &root).unwrap();

        let loaded = load_world_metadata(tmp.path()).unwrap().unwrap();

        assert_eq!(loaded.players_sleeping_percentage, 100);
        assert!(!loaded.keep_inventory);
    }
}
