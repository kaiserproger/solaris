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
    RegionPhase, RegionalCommitDecision, RegionalDecisionJournal, RegionalDecisionJournalError,
};
use mc_nbt::{ListTag, Tag, tag_type};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::*;

const PLAYERDATA_DIR: &str = "playerdata";
const SOLARIS_DIR: &str = "solaris";
const ENTITIES_FILE: &str = "entities.dat";
const WORLD_FILE: &str = "world.dat";
const REGIONAL_DECISION_JOURNAL_FILE: &str = "entity-owner-journal.json";
const LEGACY_REGIONAL_DECISION_JOURNAL_VERSION: u32 = 1;
const REGIONAL_DECISION_JOURNAL_HEADER: &[u8] = b"SOLARIS_ENTITY_OWNER_JOURNAL 2\n";
const DAMAGE_COMPONENT: &str = "minecraft:damage";
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegionalDecisionJournalFile {
    version: u32,
    pending: Vec<RegionalCommitDecision>,
}

pub(crate) struct FileRegionalDecisionJournal {
    path: PathBuf,
    pending: Vec<RegionalCommitDecision>,
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
        let (pending, migrate_legacy) = if path.is_file() {
            let bytes =
                std::fs::read(&path).map_err(|source| RegionalDecisionJournalOpenError::Io {
                    path: path.clone(),
                    source,
                })?;
            if bytes.starts_with(b"{") {
                let file: RegionalDecisionJournalFile =
                    serde_json::from_slice(&bytes).map_err(|source| {
                        RegionalDecisionJournalOpenError::Json {
                            path: path.clone(),
                            source,
                        }
                    })?;
                if file.version != LEGACY_REGIONAL_DECISION_JOURNAL_VERSION {
                    return Err(RegionalDecisionJournalOpenError::UnsupportedVersion(
                        file.version,
                    ));
                }
                (file.pending, true)
            } else {
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
                (pending, false)
            }
        } else {
            (Vec::new(), false)
        };
        if migrate_legacy {
            persist_regional_decisions(&path, &pending)?;
        }
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
    for decision in decisions {
        write_regional_decision_line(&mut file, path, decision)?;
    }
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
    for decision in pending {
        write_regional_decision_line(&mut file, &temporary, decision)?;
    }
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
    let body = &bytes[REGIONAL_DECISION_JOURNAL_HEADER.len()..];
    let complete_len = body
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |last_newline| last_newline + 1);
    let complete = &body[..complete_len];
    let mut pending = Vec::new();
    for line in complete
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        pending.push(serde_json::from_slice(line).map_err(|source| {
            RegionalDecisionJournalOpenError::Json {
                path: path.to_path_buf(),
                source,
            }
        })?);
    }
    Ok((
        pending,
        REGIONAL_DECISION_JOURNAL_HEADER.len() + complete_len,
    ))
}

fn write_regional_decision_line(
    file: &mut File,
    path: &Path,
    decision: &RegionalCommitDecision,
) -> Result<(), RegionalDecisionJournalOpenError> {
    serde_json::to_writer(&mut *file, decision).map_err(|source| {
        RegionalDecisionJournalOpenError::Json {
            path: path.to_path_buf(),
            source,
        }
    })?;
    file.write_all(b"\n")
        .map_err(|source| RegionalDecisionJournalOpenError::Io {
            path: path.to_path_buf(),
            source,
        })
}

pub(crate) fn replay_regional_commit_decisions(
    entities: Vec<PersistedEntityRecord>,
    decisions: &[RegionalCommitDecision],
) -> Vec<PersistedEntityRecord> {
    let mut entities = entities
        .into_iter()
        .map(|record| (record.snapshot.id, record))
        .collect::<BTreeMap<_, _>>();
    for decision in decisions {
        for entity in decision.removed() {
            entities.remove(entity);
        }
        for snapshot in decision.upserts() {
            let (age, pickup_delay) = entities
                .get(&snapshot.id)
                .map_or((0, 0), |record| (record.age, record.pickup_delay));
            entities.insert(
                snapshot.id,
                PersistedEntityRecord {
                    snapshot: snapshot.clone(),
                    age,
                    pickup_delay,
                },
            );
        }
    }
    entities.into_values().collect()
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
        self.append_commits(decisions)?;
        self.pending.extend_from_slice(decisions);
        Ok(())
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
        let retained = self
            .pending
            .iter()
            .filter(|decision| !phases.contains(&decision.phase()))
            .cloned()
            .collect::<Vec<_>>();
        let previous = std::mem::replace(&mut self.pending, retained);
        if self.persist().is_err() {
            self.pending = previous;
            return Err(RegionalDecisionJournalError::SAFE);
        }
        Ok(())
    }

    fn pending_phases(&self) -> Vec<RegionPhase> {
        self.pending
            .iter()
            .map(RegionalCommitDecision::phase)
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
    pub(crate) world_identity: String,
}

#[derive(Debug, Clone)]
pub(crate) struct PersistedEntityRecord {
    pub(crate) snapshot: EntitySnapshot,
    pub(crate) age: i32,
    pub(crate) pickup_delay: i32,
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
    {
        return Err(PlayerPersistenceError::InvalidNumeric {
            path: path.to_path_buf(),
            field: "entity Pos/Motion/Rotation/Health",
        });
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
) -> Result<Vec<PersistedEntityRecord>, PlayerPersistenceError> {
    let path = entities_path(world_root);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let (_, root) = read_player_root(&path)?;
    let Tag::Compound(fields) = root else {
        return Err(PlayerPersistenceError::RootNotCompound { path });
    };
    let Some(Tag::List(list)) = field(&fields, "Entities") else {
        return Ok(Vec::new());
    };
    let mut entities = Vec::new();
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
        let mut attributes = attributes_from_entity_facts(&parsed, type_id as u32);
        let health = float_field(fields, "Health").unwrap_or(20.0).max(0.0);
        attributes.set_base(AttributeKind::MaxHealth, health.max(1.0) as f64);
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
            .unwrap_or(0)
            .max(0);
        let pickup_delay = int_field(fields, "PickupDelay").unwrap_or(0).max(0);
        let snapshot = EntitySnapshot {
            id,
            uuid,
            type_id,
            type_name: type_name.to_string(),
            position: Vec3::new(pos[0], pos[1], pos[2]),
            rotation: mc_entity::Rotation {
                yaw: rotation[0],
                pitch: rotation[1],
                head_yaw: rotation[0],
            },
            velocity: Vec3::new(motion[0], motion[1], motion[2]),
            on_ground: byte_field(fields, "OnGround").unwrap_or(0) != 0 && !aquatic,
            item_stack,
            experience_value,
            block_state,
            lifecycle: EntityLifecycle::Alive,
            health,
            attributes,
            goal: if type_name == "minecraft:item"
                || type_name == "minecraft:falling_block"
                || experience_value.is_some()
            {
                GoalState::Idle
            } else if aquatic {
                GoalState::AquaticWander {
                    speed: 0.72,
                    vertical_speed: 0.18,
                    period_ticks: 45,
                }
            } else {
                GoalState::Wander {
                    speed: 0.8,
                    period_ticks: 80,
                }
            },
            vehicle: None,
            animal,
        };
        validate_entity_numeric_state(&path, &snapshot)?;
        entities.push(PersistedEntityRecord {
            snapshot,
            age,
            pickup_delay,
        });
    }
    Ok(entities)
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

fn attributes_from_entity_facts(
    id: &mc_data::Identifier,
    protocol_id: u32,
) -> mc_entity::AttributeSet {
    let facts = mc_data::entity_types::fallback_entity_type_facts(id.clone(), protocol_id);
    let mut attributes = mc_entity::AttributeSet::vanilla_mob_defaults();
    if let Some(value) = facts.attributes.max_health {
        attributes.set_base(AttributeKind::MaxHealth, value);
    }
    if let Some(value) = facts.attributes.movement_speed {
        attributes.set_base(AttributeKind::MovementSpeed, value);
    }
    if let Some(value) = facts.attributes.follow_range {
        attributes.set_base(AttributeKind::FollowRange, value);
    }
    if let Some(value) = facts.attributes.attack_damage {
        attributes.set_base(AttributeKind::AttackDamage, value);
    }
    attributes
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
    save_persisted_entity_records(world_root, items, &records)
}

pub(crate) fn save_persisted_entity_records(
    world_root: &Path,
    items: &ItemRegistry,
    entities: &[PersistedEntityRecord],
) -> Result<(), PlayerPersistenceError> {
    let path = entities_path(world_root);
    let mut elements = Vec::new();
    for record in entities
        .iter()
        .filter(|record| record.snapshot.lifecycle == EntityLifecycle::Alive)
    {
        validate_entity_numeric_state(&path, &record.snapshot)?;
        elements.push(entity_tag(items, record)?);
    }
    let root = Tag::Compound(vec![(
        "Entities".into(),
        Tag::List(ListTag {
            element_type: if elements.is_empty() {
                tag_type::END
            } else {
                tag_type::COMPOUND
            },
            elements,
        }),
    )]);
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
    Ok(Some(WorldPersistedMetadata {
        world_time: long_field(&fields, "SolarisWorldTime").unwrap_or(0) as u64,
        players_sleeping_percentage,
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
    items: &ItemRegistry,
    record: &PersistedEntityRecord,
) -> Result<Tag, PlayerPersistenceError> {
    let entity = &record.snapshot;
    let mut fields = vec![
        ("id".into(), Tag::String(entity.type_name.clone())),
        ("SolarisEntityId".into(), Tag::Int(entity.id.0)),
        ("UUID".into(), Tag::IntArray(uuid_to_int_array(entity.uuid))),
        ("Pos".into(), vec3_double_list(entity.position)),
        ("Motion".into(), vec3_double_list(entity.velocity)),
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
        ("Health".into(), Tag::Float(entity.health)),
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
mod tests {
    use super::*;
    use mc_entity::{
        RegionPhase, RegionalCommitDecision, RegionalDecisionJournal, VehicleKind, VehicleState,
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
        let reports = vec![
            mc_data::entity_types::EntityTypeReport {
                id: mc_data::Identifier::parse("minecraft:item").unwrap(),
                protocol_id: 1,
            },
            mc_data::entity_types::EntityTypeReport {
                id: mc_data::Identifier::parse("minecraft:cow").unwrap(),
                protocol_id: 2,
            },
            mc_data::entity_types::EntityTypeReport {
                id: mc_data::Identifier::parse("minecraft:cod").unwrap(),
                protocol_id: 3,
            },
            mc_data::entity_types::EntityTypeReport {
                id: mc_data::Identifier::parse("minecraft:falling_block").unwrap(),
                protocol_id: 4,
            },
            mc_data::entity_types::EntityTypeReport {
                id: mc_data::Identifier::parse("minecraft:chicken").unwrap(),
                protocol_id: 5,
            },
            mc_data::entity_types::EntityTypeReport {
                id: mc_data::Identifier::parse("minecraft:sheep").unwrap(),
                protocol_id: 6,
            },
        ];
        EntityTypeRegistry::from_report(&reports)
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
            type_id: 2,
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
        };
        let decision = RegionalCommitDecision::from_parts(
            RegionPhase(7),
            91,
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
            vec![
                PersistedEntityRecord {
                    snapshot: stale,
                    age: 12,
                    pickup_delay: 4,
                },
                PersistedEntityRecord::from(removed),
            ],
            &pending,
        );
        assert_eq!(replayed.len(), 1);
        assert_eq!(replayed[0].snapshot, decision.upserts()[0]);
        assert_eq!(replayed[0].age, 12);
        assert_eq!(replayed[0].pickup_delay, 4);
        reopened
            .clear_commit(decision.phase())
            .expect("clear decision");
        drop(reopened);
        let (_, pending) =
            FileRegionalDecisionJournal::open(tmp.path()).expect("reopen cleared journal");
        assert!(pending.is_empty());
    }

    #[test]
    fn regional_decision_journal_appends_and_ignores_only_a_truncated_tail() {
        let tmp = tempfile::tempdir().unwrap();
        let first = RegionalCommitDecision::from_parts(RegionPhase(1), 1, Vec::new(), Vec::new())
            .expect("first decision");
        let second = RegionalCommitDecision::from_parts(RegionPhase(2), 2, Vec::new(), Vec::new())
            .expect("second decision");
        let path = tmp
            .path()
            .join(SOLARIS_DIR)
            .join(REGIONAL_DECISION_JOURNAL_FILE);
        let (mut journal, pending) =
            FileRegionalDecisionJournal::open(tmp.path()).expect("open journal");
        assert!(pending.is_empty());

        journal.record_commit(&first).expect("append first");
        let first_bytes = std::fs::read(&path).expect("read first append");
        journal.record_commit(&second).expect("append second");
        let second_bytes = std::fs::read(&path).expect("read second append");
        assert!(
            second_bytes.starts_with(&first_bytes),
            "a durable decision must append instead of rewriting prior decisions"
        );
        drop(journal);

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open journal tail");
        file.write_all(br#"{"phase":"#)
            .expect("write simulated crash tail");
        file.sync_all().expect("persist simulated crash tail");
        drop(file);

        let (mut recovered, pending) =
            FileRegionalDecisionJournal::open(tmp.path()).expect("ignore partial tail");
        assert_eq!(pending, vec![first.clone(), second.clone()]);
        let third = RegionalCommitDecision::from_parts(RegionPhase(3), 3, Vec::new(), Vec::new())
            .expect("post-recovery decision");
        recovered
            .record_commit(&third)
            .expect("append after truncated-tail recovery");
        drop(recovered);
        let (_, pending) =
            FileRegionalDecisionJournal::open(tmp.path()).expect("reopen repaired journal");
        assert_eq!(pending, vec![first, second, third]);
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
        assert!(journal.pending.is_empty());
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
            type_id: 2,
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
        state.inventory.set_hotbar(3, ItemStack::new(1, 17));
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
            .with_enchantment(efficiency.clone(), 1);

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
        assert_eq!(loaded.inventory.held(3), &ItemStack::new(1, 17));
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
        loaded.inventory.set_hotbar(0, ItemStack::new(1, 5));
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
        let entity = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:item".into())),
            (
                "Pos".into(),
                Tag::List(ListTag {
                    element_type: tag_type::DOUBLE,
                    elements: vec![
                        Tag::Double(0.0),
                        Tag::Double(f64::INFINITY),
                        Tag::Double(0.0),
                    ],
                }),
            ),
            (
                "Motion".into(),
                Tag::List(ListTag {
                    element_type: tag_type::DOUBLE,
                    elements: vec![Tag::Double(0.0), Tag::Double(0.0), Tag::Double(0.0)],
                }),
            ),
            (
                "Rotation".into(),
                Tag::List(ListTag {
                    element_type: tag_type::FLOAT,
                    elements: vec![Tag::Float(0.0), Tag::Float(0.0)],
                }),
            ),
        ]);
        let root = Tag::Compound(vec![(
            "Entities".into(),
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: vec![entity],
            }),
        )]);
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
            type_id: 1,
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
        };
        let cow = EntitySnapshot {
            id: EntityId(101),
            uuid: uuid::Uuid::from_u128(101),
            type_id: 2,
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
        };
        let falling_block = EntitySnapshot {
            id: EntityId(102),
            uuid: uuid::Uuid::from_u128(102),
            type_id: 4,
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
        };
        let chicken = EntitySnapshot {
            id: EntityId(103),
            uuid: uuid::Uuid::from_u128(103),
            type_id: 3,
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
        let loaded = load_persisted_entities(tmp.path(), &items, &entity_types).unwrap();

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
            Some(0.2)
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
            type_id: 6,
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
        };

        save_persisted_entities(tmp.path(), &items, std::slice::from_ref(&sheep)).unwrap();
        let loaded = load_persisted_entities(tmp.path(), &items, &entity_types()).unwrap();

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
        let record = PersistedEntityRecord {
            snapshot: EntitySnapshot {
                id: EntityId(104),
                uuid: uuid::Uuid::from_u128(104),
                type_id: 1,
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
            },
            age: 123,
            pickup_delay: 7,
        };

        save_persisted_entity_records(tmp.path(), &items, std::slice::from_ref(&record)).unwrap();
        let loaded = load_persisted_entities(tmp.path(), &items, &entity_types).unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, record.id);
        assert_eq!(loaded[0].age, 123);
        assert_eq!(loaded[0].pickup_delay, 7);
    }

    #[test]
    fn restored_aquatic_entities_keep_aquatic_wander_goal() {
        let tmp = tempfile::tempdir().unwrap();
        let items = items();
        let entity_types = entity_types();
        let cod = EntitySnapshot {
            id: EntityId(102),
            uuid: uuid::Uuid::from_u128(102),
            type_id: 3,
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
        };

        save_persisted_entities(tmp.path(), &items, &[cod]).unwrap();
        let loaded = load_persisted_entities(tmp.path(), &items, &entity_types).unwrap();

        assert!(matches!(loaded[0].goal, GoalState::AquaticWander { .. }));
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
            type_id: 1,
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

        let loaded = load_persisted_entities(&root, &items, &entity_types).unwrap();
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
    }
}
