//! Tick journal: framed batches of absolute post-images + tombstones,
//! native-transaction checkpoints, recovery by replay (ADR 0013 "Durability").
//!
//! Layout under `<world_dir>/`:
//! - `journal/batch-<first_tick>-<last_tick>.bin`: framed batches. One batch
//!   holds whole ticks: `[magic u32][version u32][tick_lo u64][tick_hi u64][len u32][crc32 u32][payload]`.
//!   Payload is JSON lines of [`JournalEntry`]. Unpublished `.tmp` files are ignored.
//! - `native/state.redb`: one atomic checkpoint of chunks, entities, players,
//!   globals and the committed tick across all dimensions.
//!
//! Write-ahead rule: batches through checkpoint tick `c` are durable before
//! the native transaction starts. Only its Immediate commit makes pruning
//! batches at or below `c` safe; recovery folds only batches above that tick.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::boundary::{BlockPos, ChunkPos, EntityId};
use crate::config::CoreError;
mod player_nbt;
pub(crate) use player_nbt::*;
mod replay;
pub use replay::*;

const BATCH_MAGIC: u32 = 0x534F_4C4A;
const BATCH_VERSION: u32 = 1;
const HEADER_LEN: usize = 4 + 4 + 8 + 8 + 4 + 4;

/// One absolute post-image inside a batch. Replay is idempotent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum JournalEntry {
    /// Native plugin custody, KV tombstones and exactly-once operation receipts.
    Plugins {
        value: Box<crate::plugin::PluginDelta>,
    },
    /// Dimension-qualified local post-images in the same global tick batch.
    Dimension {
        dimension: crate::dimensions::Dimension,
        entries: Vec<JournalEntry>,
    },
    EndFight {
        value: crate::end_fight::Fight,
    },
    Raids {
        #[serde(deserialize_with = "crate::raids::deserialize")]
        value: crate::raids::Raids,
    },
    WanderingTrader {
        value: crate::mob_lanes::TraderSpawner,
    },
    /// Absolute block state after the tick.
    Block {
        pos: [i32; 3],
        state: u32,
    },
    /// Absolute native scheduled work for one chunk, including empty clears.
    BlockTicks {
        image: crate::block_ticks::BlockTickChunkImage,
        next_sequence: u64,
    },
    FluidTicks {
        image: crate::fluids::FluidTicksImage,
    },
    /// A chunk entered/left the player-ticking set; unchanged intervals need
    /// no further records because the batch clock supplies elapsed ticks.
    InhabitedTime {
        image: InhabitedTimeImage,
    },
    /// 001 slice0 active-only debt clock (001-d/e, roadmap C/G5).
    /// Absolute per-chunk catch-up debt. Scope0 consumes with zero effects;
    /// the zero image is still journaled for exact-once durability.
    SimDebt {
        image: SimDebtImage,
    },
    /// Absolute world clock, weather duration and retained fade levels.
    Climate {
        value: crate::weather::ClimateImage,
    },
    Border {
        value: crate::command_border::Border,
    },
    Difficulty {
        value: crate::survival::Difficulty,
    },
    /// Canonical gamerule overrides, including rules without an active subsystem.
    GameRules {
        values: BTreeMap<String, String>,
    },
    /// Absolute scoreboard post-image after a scoreboard mutation.
    /// Once-only structure marker claim; retained even when its entity dies.
    GeneratedEntityClaim {
        claim: String,
    },
    Scoreboard {
        value: crate::scoreboard::Scoreboard,
    },
    /// World-global command storage as a named NBT compound.
    CommandStorage {
        value: Vec<u8>,
    },
    SavedMap {
        id: i32,
        value: Arc<crate::maps::SavedMap>,
    },
    /// Absolute bossbar post-image after a bossbar mutation: named-NBT bytes
    /// of the vanilla saved-data envelope (Tag has no serde); decoded on
    /// replay, fail-closed like checkpoints.
    Bossbars {
        value: Vec<u8>,
    },
    /// Player post-image; omitted heavy fields retain the previous image.
    /// Inventory 41 + cursor + open crafting grid ride as separate stacks
    /// (real custody; never folded). Containers ride as block-entity
    /// post-images, not here.
    Player {
        uuid: String,
        #[serde(default)]
        opaque_player_fields: Vec<u8>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_advancement_progress"
        )]
        advancement_progress: Option<Arc<serde_json::Value>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unlocked_recipes: Option<Arc<BTreeSet<String>>>,
        #[serde(default)]
        statistics: Box<mc_data::statistics::PlayerStatistics>,
        #[serde(default)]
        dimension: crate::dimensions::Dimension,
        pos: [f64; 3],
        yaw: f32,
        pitch: f32,
        health: f32,
        food: i32,
        saturation: f32,
        exhaustion: f32,
        air: i32,
        experience_level: i32,
        experience_progress: f32,
        experience_total: i32,
        game_mode: u8,
        selected: u8,
        inventory: Vec<mc_data::ItemStack>,
        carried: mc_data::ItemStack,
        grid: Vec<mc_data::ItemStack>,
        /// Bed head or forced command-selected respawn cell.
        #[serde(default)]
        respawn: Option<[i32; 3]>,
        #[serde(default)]
        respawn_dimension: crate::dimensions::Dimension,
        #[serde(default)]
        respawn_forced: bool,
        #[serde(default)]
        respawn_yaw: f32,
        #[serde(default)]
        respawn_pitch: f32,
        #[serde(default)]
        active_effects: crate::effects::ActiveEffects,
        #[serde(default)]
        enchantment_seed: i32,
        enchantment_offer_seed: i32,
        #[serde(default)]
        warden_warning: crate::shrieker::Warning,
        #[serde(default, deserialize_with = "crate::mace::deserialize_impact")]
        current_impulse_impact_pos: Option<[f64; 3]>,
        #[serde(default)]
        fall_flying: bool,
        #[serde(default)]
        burn_ticks: u32,
        #[serde(default)]
        portal: crate::portals::PortalState,
        tick: u64,
    },
    /// Statistics-only absolute changes; idle timer progression needs no entry.
    Statistics {
        uuid: String,
        #[serde(deserialize_with = "deserialize_statistics_changes")]
        changes: Vec<(u32, u32, i32)>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timers: Option<mc_data::statistics::StatisticsTimers>,
        tick: u64,
    },
    /// Block-entity post-image (chest/furnace slots + furnace progress).
    BlockEntity {
        pos: [i32; 3],
        slots: Vec<mc_data::ItemStack>,
        furnace: Option<FurnaceImage>,
        tick: u64,
    },
    /// Block-entity removal tombstone (container block broken): deletes the
    /// checkpoint base record on replay so the next checkpoint's injection
    /// (which clears first) cannot resurrect it.
    RemoveBlockEntity {
        pos: [i32; 3],
        tick: u64,
    },
    /// Opaque vanilla NBT (signs), or a removal tombstone.
    OpaqueBlockEntity {
        pos: [i32; 3],
        #[serde(deserialize_with = "crate::conduit::deserialize_nbt")]
        nbt: Option<Vec<u8>>,
    },
    /// Full mob/item/projectile post-image. Mob motion remains checkpoint-only
    /// (ADR 0013), except birth coordinates. Projectile motion is journaled
    /// every flight tick together with its custody/payload, so replay cannot
    /// rewind a missile across a hit or resurrect ammunition.
    /// Retained state shares its immutable allocation with the journal baseline;
    /// other large payloads are boxed. Serde keeps the existing representation.
    Entity {
        id: i32,
        uuid: String,
        type_id: u32,
        health: f32,
        alive: bool,
        age: u32,
        breed_cooldown: u32,
        pickup_delay: u32,
        item: Option<mc_data::ItemStack>,
        appearance: EntityAppearanceImage,
        #[serde(default)]
        active_effects: crate::effects::ActiveEffects,
        #[serde(default, deserialize_with = "deserialize_optional_villager_image")]
        villager: Option<Box<crate::villager::VillagerState>>,
        #[serde(default)]
        state: Arc<EntityStateImage>,
        #[serde(default, deserialize_with = "deserialize_optional_projectile_image")]
        projectile: Option<Box<crate::projectiles::ProjectileState>>,
        tick: u64,
        /// Birth coordinates for mobs; current coordinates for projectiles.
        pos: [f64; 3],
        /// Core blocks/second. Only projectile replay advances this value.
        #[serde(default)]
        vel: [f64; 3],
        /// Spawn yaw (same rule as `pos`).
        yaw: f32,
        /// Spawn pitch (same rule as `pos`).
        pitch: f32,
    },
    /// Compact absolute non-wire state; keeps timers out of appearance post-images.
    /// Shared with the comparison baseline, with unchanged serde representation.
    EntityState {
        id: i32,
        state: Arc<EntityStateImage>,
        tick: u64,
    },
    /// Health-only update (hurt, no spawn/appearance change): the full
    /// record (appearance, item, uuid) rides the spawn entry + checkpoint
    /// base. Keeps per-tick journal volume proportional to damage, not to
    /// appearance image size.
    EntityVitals {
        id: i32,
        health: f32,
        alive: bool,
        tick: u64,
    },
    /// Removal tombstone.
    RemoveEntity {
        id: i32,
        tick: u64,
    },
    /// Remote ticket state after the tick.
    Ticket {
        owner: String,
        chunk: [i32; 2],
        tick: u64,
        /// Chunk radius around the centre (default 0 = centre only; keeps
        /// pre-radius journals readable).
        #[serde(default)]
        radius: i32,
    },
    /// Absolute dimension-local settlement post-image plus monotonic high water.
    Settlement {
        image: Box<crate::settlements::RecordImage>,
        high_water: u64,
        tick: u64,
    },
    /// Tombstone for a durable ticket that no longer exists (`/forceload remove`,
    /// plugin/API release). Appended last: earlier variant indices are unchanged.
    TicketRemoved {
        owner: String,
        tick: u64,
    },
    /// Dimension-local block-log records and rollback marks (`block_log`).
    BlockLog {
        value: Box<crate::block_log::Delta>,
    },
    /// Absolute schedule image; appended to preserve older variant indices.
    ScheduledFunctions {
        value: Vec<(u64, String)>,
    },
    /// Datapack enable/disable selection; appended to preserve variant order.
    DatapackSelection {
        value: crate::datapacks::Selection,
    },
}

fn deserialize_statistics_changes<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<Vec<(u32, u32, i32)>, D::Error> {
    use mc_data::statistics::{CustomStat, StatType};
    let changes = Vec::<(u32, u32, i32)>::deserialize(decoder)?;
    if changes.iter().any(|&(kind, id, _)| {
        StatType::from_id(kind).is_none()
            || kind == StatType::Custom.id() && CustomStat::from_id(id).is_none()
            || id > i32::MAX as u32
    }) {
        return Err(serde::de::Error::custom("invalid statistics registry id"));
    }
    Ok(changes)
}

fn deserialize_optional_villager_image<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<Option<Box<crate::villager::VillagerState>>, D::Error> {
    crate::villager::persistence::deserialize_optional_state(decoder)
        .map(|state| state.map(Box::new))
}

fn deserialize_optional_projectile_image<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<Option<Box<crate::projectiles::ProjectileState>>, D::Error> {
    crate::projectiles::persistence::deserialize_optional_state(decoder)
        .map(|state| state.map(Box::new))
}

/// Fsync one file path (file must exist).
pub fn fsync_file(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

/// Fsync a directory entry. On Linux opening a directory as a `File` and
/// syncing it fsyncs the directory entry; elsewhere this is a no-op.
pub fn fsync_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Append batches to the journal on the journal thread. Every batch file is
/// fsynced when written; the directory every `fsync_every` appends (call
/// [`JournalWriter::flush`] to force).
pub struct JournalWriter {
    dir: PathBuf,
    pending_fsync: u32,
    fsync_every: u32,
    /// Ticks `lo..=hi` appended contiguously since open: the WAL gate
    /// checks coverage here instead of re-reading batch files.
    appended: Option<(u64, u64)>,
    /// Immutable pre-open coverage `(checkpoint_tick, recovered_tick]`, decoded
    /// either by startup recovery or the first checkpoint gate.
    pre_open_verified: Option<(u64, u64)>,
}

/// Test-only rendezvous inside the unpublished WAL write, before file fsync.
#[cfg(test)]
type WalWriteObserver = (PathBuf, Box<dyn Fn(&mut File, &[u8]) + Send>);
#[cfg(test)]
pub(crate) static WAL_WRITE_OBSERVER: parking_lot::Mutex<Option<WalWriteObserver>> =
    parking_lot::const_mutex(None);

impl JournalWriter {
    pub fn open(world_dir: &Path, fsync_every: u32) -> Result<Self, CoreError> {
        let dir = world_dir.join("journal");
        std::fs::create_dir_all(&dir).map_err(|err| CoreError::Storage(err.to_string()))?;
        Ok(Self {
            dir,
            pending_fsync: 0,
            fsync_every: fsync_every.max(1),
            appended: None,
            pre_open_verified: None,
        })
    }

    /// Reuse the contiguous tail already validated while holding the world lease.
    /// Published batches are immutable; the checkpoint gate still fsyncs the
    /// directory and verifies every tick appended by this writer.
    pub(crate) fn open_recovered(
        world_dir: &Path,
        fsync_every: u32,
        checkpoint_tick: u64,
        recovered_tick: u64,
    ) -> Result<Self, CoreError> {
        let mut writer = Self::open(world_dir, fsync_every)?;
        writer.pre_open_verified = Some((checkpoint_tick, recovered_tick));
        Ok(writer)
    }

    /// Write-ahead gate for checkpoint `through`: every tick in
    /// `(prev_tick, through]` is in a durable batch. Appends since open
    /// are contiguous (an append failure ends the journal), so coverage is
    /// tracked in process; batches from before open are verified on disk
    /// once. Batch files are fsynced at append, so only the directory
    /// needs a final fsync.
    pub fn gate(&mut self, prev_tick: u64, through: u64) -> Result<(), CoreError> {
        // Recovery may already cover the whole save without a new tick/append.
        if through == prev_tick
            || self
                .pre_open_verified
                .is_some_and(|(from, hi)| from <= prev_tick && hi >= through)
        {
            return self.flush();
        }
        let hole = |from: u64| {
            Err(CoreError::Storage(format!(
                "journal hole: ticks {from}..={through} have no batch"
            )))
        };
        let first_appended = match self.appended {
            Some((lo, hi)) if hi >= through => lo,
            Some((_, hi)) => return hole(hi + 1),
            None => return hole(prev_tick + 1),
        };
        if first_appended > prev_tick + 1
            && !self
                .pre_open_verified
                .is_some_and(|(from, through)| from <= prev_tick && through >= first_appended - 1)
        {
            verify_batches_through(&self.dir, prev_tick, first_appended - 1)?;
            self.pre_open_verified = Some((prev_tick, first_appended - 1));
        }
        self.flush()
    }

    /// Append one batch covering ticks `tick_lo..=tick_hi`.
    pub fn append(
        &mut self,
        tick_lo: u64,
        tick_hi: u64,
        entries: &[JournalEntry],
    ) -> Result<(), CoreError> {
        if let Some((_, hi)) = self.appended
            && tick_lo != hi + 1
        {
            return Err(CoreError::Storage(format!(
                "journal append {tick_lo} does not follow {hi}"
            )));
        }
        // Test hook (unit tests only): fail EVERY append for the armed
        // world dir while armed — per-dir so parallel tests never poison
        // each other's journals (process-global hook, same pattern as
        // FAIL_CHECKPOINT_AFTER_REGIONS).
        #[cfg(test)]
        if crate::core::FAIL_JOURNAL_APPEND
            .lock()
            .ok()
            .and_then(|armed| armed.clone())
            .is_some_and(|dir| *dir == self.dir.parent().unwrap_or(&self.dir).to_path_buf())
        {
            return Err(CoreError::Storage(
                "injected journal append failure".to_owned(),
            ));
        }
        let mut payload = String::new();
        for entry in entries {
            if let JournalEntry::Entity {
                villager: Some(state),
                ..
            } = entry
            {
                crate::villager::persistence::validate_state(state)
                    .map_err(|error| CoreError::Storage(error.to_string()))?;
            }
            if let JournalEntry::Entity {
                projectile: Some(state),
                pos,
                vel,
                yaw,
                pitch,
                ..
            } = entry
            {
                crate::projectiles::persistence::validate_state(state)
                    .map_err(|error| CoreError::Storage(error.to_owned()))?;
                if pos.iter().chain(vel.iter()).any(|value| !value.is_finite())
                    || !yaw.is_finite()
                    || !pitch.is_finite()
                {
                    return Err(CoreError::Storage("nonfinite projectile motion".into()));
                }
            }
            payload.push_str(
                &serde_json::to_string(entry).map_err(|err| CoreError::Storage(err.to_string()))?,
            );
            payload.push('\n');
        }
        let bytes = payload.into_bytes();
        let crc = crc32fast::hash(&bytes);
        let mut frame = Vec::with_capacity(HEADER_LEN + bytes.len());
        frame.extend_from_slice(&BATCH_MAGIC.to_le_bytes());
        frame.extend_from_slice(&BATCH_VERSION.to_le_bytes());
        frame.extend_from_slice(&tick_lo.to_le_bytes());
        frame.extend_from_slice(&tick_hi.to_le_bytes());
        frame.extend_from_slice(
            &u32::try_from(bytes.len())
                .map_err(|_| CoreError::Storage("batch too large".to_owned()))?
                .to_le_bytes(),
        );
        frame.extend_from_slice(&crc.to_le_bytes());
        frame.extend_from_slice(&bytes);
        let name = format!("batch-{tick_lo:020}-{tick_hi:020}.bin");
        let tmp = self.dir.join(format!("{name}.tmp"));
        let final_path = self.dir.join(&name);
        {
            let mut file = File::create(&tmp).map_err(|err| CoreError::Storage(err.to_string()))?;
            #[cfg(test)]
            if let Some((_, observe)) = WAL_WRITE_OBSERVER
                .lock()
                .as_ref()
                .filter(|(dir, _)| dir == &self.dir)
            {
                observe(&mut file, &frame);
            }
            file.write_all(&frame)
                .map_err(|err| CoreError::Storage(err.to_string()))?;
            file.sync_all()
                .map_err(|err| CoreError::Storage(err.to_string()))?;
        }
        std::fs::rename(&tmp, &final_path).map_err(|err| CoreError::Storage(err.to_string()))?;
        self.appended = Some((self.appended.map_or(tick_lo, |(lo, _)| lo), tick_hi));
        self.pending_fsync += 1;
        if self.pending_fsync >= self.fsync_every {
            self.flush()?;
        }
        Ok(())
    }

    /// Fsync the journal directory so all appended batches are durable.
    pub fn flush(&mut self) -> Result<(), CoreError> {
        fsync_dir(&self.dir).map_err(|err| CoreError::Storage(err.to_string()))?;
        self.pending_fsync = 0;
        Ok(())
    }
}

impl JournalWriter {
    /// Highest tick durable in the WAL from this writer's point of view:
    /// appended since open or verified at startup. `None` before either.
    pub(crate) fn covered_through(&self) -> Option<u64> {
        self.appended
            .map(|(_, hi)| hi)
            .into_iter()
            .chain(self.pre_open_verified.map(|(_, hi)| hi))
            .max()
    }
}

/// Parse `batch-<lo>-<hi>.bin`.
pub(crate) fn parse_batch_name(name: &str) -> Option<(u64, u64)> {
    let rest = name.strip_prefix("batch-")?.strip_suffix(".bin")?;
    let (lo, hi) = rest.split_once('-')?;
    Some((lo.parse().ok()?, hi.parse().ok()?))
}

/// Read complete published batches after the manifest, failing closed on damage.
/// Incomplete, unpublished `.tmp` files are ignored.
pub fn read_complete_batches_after(
    journal_dir: &Path,
    manifest_tick: u64,
) -> Result<Vec<(u64, u64, Vec<JournalEntry>)>, CoreError> {
    let mut out = Vec::new();
    visit_complete_batches_after(journal_dir, manifest_tick, |batch| {
        out.push(batch);
        Ok(())
    })?;
    Ok(out)
}

/// Decode and consume one published batch at a time. Recovery memory is bounded
/// by one batch and the latest recovered images, never the complete WAL history.
pub(crate) fn visit_complete_batches_after(
    journal_dir: &Path,
    manifest_tick: u64,
    mut visit: impl FnMut((u64, u64, Vec<JournalEntry>)) -> Result<(), CoreError>,
) -> Result<u64, CoreError> {
    let entries = match std::fs::read_dir(journal_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(manifest_tick),
        Err(err) => return Err(CoreError::Storage(err.to_string())),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| CoreError::Storage(err.to_string()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some((lo, hi)) = parse_batch_name(&name) {
            names.push((lo, hi, name));
        }
    }
    names.sort();
    let mut previous_tick = manifest_tick;
    for (lo, hi, name) in &names {
        if *hi <= manifest_tick {
            continue;
        }
        let path = journal_dir.join(name);
        let expected = previous_tick.checked_add(1).ok_or_else(|| {
            CoreError::Storage(format!("journal {}: tick range overflow", path.display()))
        })?;
        if *lo > expected {
            return Err(CoreError::Storage(format!(
                "journal {}: journal hole: ticks {expected}..={} have no batch",
                path.display(),
                lo - 1,
            )));
        }
        if *lo != expected {
            return Err(CoreError::Storage(format!(
                "journal {}: noncontiguous batch starts at {lo}, expected {expected}",
                path.display(),
            )));
        }
        let bytes = std::fs::read(&path)
            .map_err(|err| CoreError::Storage(format!("journal {}: {err}", path.display())))?;
        let entries = decode_batch(&bytes, *lo, *hi).map_err(|err| err.at_path(&path))?;
        drop(bytes);
        visit((*lo, *hi, entries))?;
        previous_tick = *hi;
    }
    Ok(previous_tick)
}
/// Verify the WAL covers `(prev_tick, through]` contiguously: every tick in
/// the range must sit inside some complete batch file. A missing batch (a
/// failed append the tick thread never retried) is a hole no fsync can
/// fill — the caller must refuse the checkpoint ack. Torn tails fail too.
pub fn verify_batches_through(
    journal_dir: &Path,
    prev_tick: u64,
    through: u64,
) -> Result<(), CoreError> {
    let mut intervals: Vec<(u64, u64)> = Vec::new();
    let entries =
        std::fs::read_dir(journal_dir).map_err(|err| CoreError::Storage(err.to_string()))?;
    for entry in entries {
        let entry = entry.map_err(|err| CoreError::Storage(err.to_string()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !(name.starts_with("batch-") && name.ends_with(".bin")) {
            continue;
        }
        let Some((lo, hi)) = parse_batch_name(&name) else {
            continue;
        };
        if hi <= prev_tick || lo > through {
            continue;
        }
        let bytes = std::fs::read(journal_dir.join(&name))
            .map_err(|err| CoreError::Storage(err.to_string()))?;
        decode_batch(&bytes, lo, hi).map_err(|err| err.at_path(&journal_dir.join(&name)))?;
        intervals.push((lo, hi));
    }
    intervals.sort();
    let mut covered = prev_tick;
    for (lo, hi) in intervals {
        if lo > covered + 1 {
            return Err(CoreError::Storage(format!(
                "journal hole: ticks {}..={} have no batch",
                covered + 1,
                lo - 1
            )));
        }
        covered = covered.max(hi);
        if covered >= through {
            break;
        }
    }
    if covered < through {
        return Err(CoreError::Storage(format!(
            "journal hole: ticks {}..={through} have no batch",
            covered + 1
        )));
    }
    Ok(())
}
#[derive(Debug)]
enum BatchDecodeError {
    Framing(String),
    Record {
        index: usize,
        offset: usize,
        kind: Option<String>,
        source: serde_json::Error,
    },
}

impl BatchDecodeError {
    fn at_path(self, path: &Path) -> CoreError {
        let detail = match self {
            Self::Framing(reason) => {
                format!("record 0 offset 0: {reason}")
            }
            Self::Record {
                index,
                offset,
                kind,
                source,
            } => format!(
                "record {index} offset {offset} kind {}: {source}",
                kind.as_deref().unwrap_or("unknown")
            ),
        };
        CoreError::Storage(format!("journal {}: {detail}", path.display()))
    }
}

fn decode_batch(
    bytes: &[u8],
    expect_lo: u64,
    expect_hi: u64,
) -> Result<Vec<JournalEntry>, BatchDecodeError> {
    if bytes.len() < HEADER_LEN {
        return Err(BatchDecodeError::Framing("incomplete batch header".into()));
    }
    let magic = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let lo = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
    let hi = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
    let len = u32::from_le_bytes(bytes[24..28].try_into().unwrap()) as usize;
    let crc = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
    if magic != BATCH_MAGIC || version != BATCH_VERSION || lo != expect_lo || hi != expect_hi {
        return Err(BatchDecodeError::Framing(format!(
            "invalid header: magic {magic:#x}, version {version}, ticks {lo}..={hi}, expected {expect_lo}..={expect_hi}"
        )));
    }
    let actual_len = bytes.len() - HEADER_LEN;
    if actual_len < len {
        return Err(BatchDecodeError::Framing(format!(
            "incomplete payload: expected {len} bytes, found {actual_len}"
        )));
    }
    if actual_len > len {
        return Err(BatchDecodeError::Framing(format!(
            "trailing bytes: expected {len} payload bytes, found {actual_len}"
        )));
    }
    let payload = &bytes[HEADER_LEN..];
    let actual_crc = crc32fast::hash(payload);
    if actual_crc != crc {
        return Err(BatchDecodeError::Framing(format!(
            "CRC mismatch: expected {crc:#x}, found {actual_crc:#x}"
        )));
    }
    let mut entries = Vec::new();
    let mut offset = HEADER_LEN;
    for (index, line) in payload.split(|byte| *byte == b'\n').enumerate() {
        if !line.is_empty() {
            let entry = serde_json::from_slice(line).map_err(|source| {
                let kind = serde_json::from_slice::<serde_json::Value>(line)
                    .ok()
                    .and_then(|value| match value {
                        serde_json::Value::Object(object) if object.len() == 1 => {
                            object.into_iter().next().map(|(kind, _)| kind)
                        }
                        serde_json::Value::String(kind) => Some(kind),
                        _ => None,
                    })
                    .or_else(|| {
                        // An incomplete object may still expose its enum tag.
                        let colon = line.iter().position(|byte| *byte == b':')?;
                        let key = std::str::from_utf8(&line[..colon]).ok()?.trim();
                        serde_json::from_str::<String>(key.strip_prefix('{')?.trim()).ok()
                    });
                BatchDecodeError::Record {
                    index,
                    offset,
                    kind,
                    source,
                }
            })?;
            entries.push(entry);
        }
        offset += line.len() + 1;
    }
    Ok(entries)
}

/// Absolute counter at an interval boundary. Active intervals advance through
/// the recovered WAL tick, not wall time or the next player's join tick.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct InhabitedTimeImage {
    pub chunk: [i32; 2],
    pub value: i64,
    pub tick: u64,
    pub active: bool,
}

impl InhabitedTimeImage {
    pub(crate) fn value_at(self, tick: u64) -> i64 {
        let elapsed = if self.active {
            tick.saturating_sub(self.tick)
        } else {
            0
        };
        self.value
            .saturating_add(i64::try_from(elapsed).unwrap_or(i64::MAX))
    }

    pub(crate) fn apply(self, chunk: &mut mc_world::Chunk, tick: u64) {
        let value = self.value_at(tick);
        chunk.extras.retain(|(key, _)| key != "InhabitedTime");
        chunk
            .extras
            .push(("InhabitedTime".into(), mc_nbt::Tag::Long(value)));
    }
}

/// 001 slice0 active-only per-chunk debt clock (001-d/e).
/// Absolute catch-up debt at a WAL/checkpoint tick. `last_sim` advances only
/// while the chunk is active+resident; `remaining` holds pre-existing debt
/// consumed on activation (scope0: zero effects, durable zero image).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct SimDebtImage {
    pub chunk: [i32; 2],
    pub last_sim: u64,
    pub remaining: u64,
    pub tick: u64,
    /// Open interval at this image's tick mirrors
    /// [`InhabitedTimeImage::active`]: the recovered batch clock supplies
    /// elapsed ticks (`last_sim_at`), never wall time or re-accrual.
    pub active: bool,
}

impl SimDebtImage {
    pub fn last_sim_at(self, tick: u64) -> u64 {
        if self.active {
            self.last_sim.max(tick)
        } else {
            self.last_sim
        }
    }
}

/// Native checkpoint image and durable player record ticks.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct CheckpointManifest {
    pub tick: u64,
    /// Required native checkpoint subdomain schema versions.
    pub domain_versions: BTreeMap<String, u32>,
    #[serde(default)]
    pub dimensions: BTreeMap<crate::dimensions::Dimension, Box<CheckpointManifest>>,
    #[serde(default)]
    pub end_fight: Option<crate::end_fight::Fight>,
    #[serde(default)]
    pub raids: Option<crate::raids::Raids>,
    #[serde(default)]
    pub wandering_trader: Option<crate::mob_lanes::TraderSpawner>,
    #[serde(default)]
    pub scheduled_functions: Vec<(u64, String)>,
    #[serde(default)]
    pub datapack_selection: Option<crate::datapacks::Selection>,
    /// Durable ticket post-images by owner: centre, (capture tick, radius).
    /// Portal transfer tickets are transient and excluded from checkpoint captures.
    #[serde(default)]
    pub tickets: BTreeMap<String, ([i32; 2], (u64, i32))>,
    /// Player UUID -> durable native record tick, including offline players.
    #[serde(default)]
    pub players: std::collections::BTreeMap<String, u64>,
    /// Generated world spawn block coordinates; absent only in old manifests.
    #[serde(default)]
    pub world_spawn: Option<[i32; 3]>,
    #[serde(default)]
    pub world_spawn_rotation: [f32; 2],
    #[serde(default)]
    pub generated_entity_claims: std::collections::BTreeSet<String>,
    /// Captured together with the checkpoint tick; never rebuilt from it.
    #[serde(default)]
    pub climate: Option<crate::weather::ClimateImage>,
    #[serde(default)]
    pub border: crate::command_border::Border,
    #[serde(default)]
    pub difficulty: crate::survival::Difficulty,
    /// Absolute deadlines keep the scheduler clock stable across restart.
    #[serde(default)]
    pub block_ticks: crate::block_ticks::BlockTicksImage,
    #[serde(default)]
    pub fluid_ticks: crate::fluids::FluidTicksImage,
    /// Open inhabited-time intervals at the same tick as the chunk capture.
    #[serde(default)]
    pub inhabited_time: Vec<InhabitedTimeImage>,
    /// 001 slice0 per-chunk debt images at the checkpoint tick; only chunks
    /// with pending (dirty) or nonzero debt are listed.
    #[serde(default)]
    pub debt: Vec<SimDebtImage>,
    #[serde(default)]
    pub gamerules: BTreeMap<String, String>,
}

/// Prune batches fully at or below `manifest_tick`.
pub fn prune_batches_through(journal_dir: &Path, manifest_tick: u64) {
    let Ok(entries) = std::fs::read_dir(journal_dir) else {
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !(name.starts_with("batch-") && name.ends_with(".bin")) {
            continue;
        }
        if let Some((_, hi)) = parse_batch_name(&name)
            && hi <= manifest_tick
        {
            let _ = std::fs::remove_file(journal_dir.join(&name));
        }
    }
}

/// Compact player post-image shared by the journal and checkpoints.
pub type PlayerImage = ([f64; 3], f32, f32, f32, u8, u64);

/// Furnace progress image for block-entity post-images.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Default)]
pub struct FurnaceImage {
    pub lit_remaining: i32,
    pub lit_total: i32,
    pub cook_progress: i32,
    pub cook_total: i32,
    pub recipes_used: Arc<BTreeMap<String, i32>>,
}

/// Replayed full player record.
#[derive(Debug, Clone)]
pub struct ReplayPlayer {
    /// Unknown player root fields, encoded as a named NBT compound.
    pub opaque_player_fields: Vec<u8>,
    pub statistics: mc_data::statistics::PlayerStatistics,
    /// Exact vanilla advancements document, including unknown keys.
    pub advancement_progress: Arc<serde_json::Value>,
    pub unlocked_recipes: Arc<BTreeSet<String>>,
    pub dimension: crate::dimensions::Dimension,
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub health: f32,
    pub food: i32,
    pub saturation: f32,
    pub exhaustion: f32,
    pub air: i32,
    pub experience_level: i32,
    pub experience_progress: f32,
    pub experience_total: i32,
    pub game_mode: u8,
    pub selected: u8,
    pub inventory: Vec<mc_data::ItemStack>,
    /// Cursor stack custody (never folded away).
    pub carried: mc_data::ItemStack,
    /// Open menu crafting grid custody (never folded away).
    pub grid: Vec<mc_data::ItemStack>,
    /// Bed head or forced command-selected respawn cell.
    pub respawn: Option<[i32; 3]>,
    pub respawn_dimension: crate::dimensions::Dimension,
    pub respawn_forced: bool,
    pub respawn_yaw: f32,
    pub respawn_pitch: f32,
    pub active_effects: crate::effects::ActiveEffects,
    pub enchantment_seed: i32,
    pub enchantment_offer_seed: i32,
    pub warden_warning: crate::shrieker::Warning,
    pub current_impulse_impact_pos: Option<[f64; 3]>,
    pub fall_flying: bool,
    pub burn_ticks: u32,
    pub portal: crate::portals::PortalState,
    pub tick: u64,
}

impl PartialEq for ReplayPlayer {
    fn eq(&self, other: &Self) -> bool {
        self.eq_except_statistics(other) && self.statistics == other.statistics
    }
}

impl ReplayPlayer {
    /// Statistics have their own compact WAL path; every other durable field
    /// still selects the complete Player postimage.
    pub(crate) fn eq_except_statistics(&self, other: &Self) -> bool {
        // No `..`: adding a durable field must also update this comparison.
        let Self {
            opaque_player_fields: _,
            statistics: _,
            advancement_progress: _,
            unlocked_recipes: _,
            dimension: _,
            pos: _,
            yaw: _,
            pitch: _,
            health: _,
            food: _,
            saturation: _,
            exhaustion: _,
            air: _,
            experience_level: _,
            experience_progress: _,
            experience_total: _,
            game_mode: _,
            selected: _,
            inventory: _,
            carried: _,
            grid: _,
            respawn: _,
            respawn_dimension: _,
            respawn_forced: _,
            respawn_yaw: _,
            respawn_pitch: _,
            active_effects: _,
            enchantment_seed: _,
            enchantment_offer_seed: _,
            warden_warning: _,
            current_impulse_impact_pos: _,
            fall_flying: _,
            burn_ticks: _,
            portal: _,
            tick: _,
        } = self;
        (Arc::ptr_eq(&self.advancement_progress, &other.advancement_progress)
            || self.advancement_progress == other.advancement_progress)
            && (Arc::ptr_eq(&self.unlocked_recipes, &other.unlocked_recipes)
                || self.unlocked_recipes == other.unlocked_recipes)
            && self.opaque_player_fields == other.opaque_player_fields
            && self.dimension == other.dimension
            && self.pos == other.pos
            && self.yaw == other.yaw
            && self.pitch == other.pitch
            && self.health == other.health
            && self.food == other.food
            && self.saturation == other.saturation
            && self.exhaustion == other.exhaustion
            && self.air == other.air
            && self.experience_level == other.experience_level
            && self.experience_progress == other.experience_progress
            && self.experience_total == other.experience_total
            && self.game_mode == other.game_mode
            && self.selected == other.selected
            && self.inventory == other.inventory
            && self.carried == other.carried
            && self.grid == other.grid
            && self.respawn == other.respawn
            && self.respawn_dimension == other.respawn_dimension
            && self.respawn_forced == other.respawn_forced
            && self.respawn_yaw == other.respawn_yaw
            && self.respawn_pitch == other.respawn_pitch
            && self.active_effects == other.active_effects
            && self.enchantment_seed == other.enchantment_seed
            && self.enchantment_offer_seed == other.enchantment_offer_seed
            && self.warden_warning == other.warden_warning
            && self.current_impulse_impact_pos == other.current_impulse_impact_pos
            && self.fall_flying == other.fall_flying
            && self.burn_ticks == other.burn_ticks
            && self.portal == other.portal
            && self.tick == other.tick
    }
}

pub(crate) fn empty_advancement_progress() -> Arc<serde_json::Value> {
    Arc::new(serde_json::Value::Object(serde_json::Map::new()))
}

// A present JSON null is retained document data, not an omitted post-image.
fn deserialize_advancement_progress<'de, D>(
    deserializer: D,
) -> Result<Option<Arc<serde_json::Value>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde_json::Value::deserialize(deserializer).map(|value| Some(Arc::new(value)))
}
/// Replayed block-entity record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ReplayBlockEntity {
    pub slots: Vec<mc_data::ItemStack>,
    pub furnace: Option<FurnaceImage>,
    pub tick: u64,
}

/// Journal-safe image of [`EntityAppearance`](crate::boundary::EntityAppearance):
/// all non-motion entity state (carried item, equipment, baby, pose, flags,
/// health, custom name, variant data). `ItemStack` round-trips through JSON.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Default)]
pub struct EntityAppearanceImage {
    pub item: Option<mc_data::ItemStack>,
    #[serde(default)]
    pub experience_orb: Option<(i32, i32)>,
    #[serde(default)]
    pub spawn_data: i32,
    pub equipment: Vec<Option<mc_data::ItemStack>>,
    pub baby: bool,
    pub pose: u8,
    pub shared_flags: u8,
    pub health: f32,
    pub custom_name: Option<String>,
    #[serde(default)]
    pub custom_name_visible: bool,
    pub extra: Vec<(u8, EntityDataValueImage)>,
}

/// Journal-safe image of [`EntityDataValue`](crate::boundary::EntityDataValue).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum EntityDataValueImage {
    Byte(i8),
    VarInt(i32),
    Float(f32),
    Rotations([f32; 3]),
    VarLong(i64),
    Boolean(bool),
    String(String),
    BlockState(u32),
    WeatheringCopper(u8),
    CopperGolemState(u8),
    Direction(u8),
    PaintingVariant(u32),
    Item(Option<mc_data::ItemStack>),
    OptionalBlockPos(Option<[i32; 3]>),
    EntityEffectParticle(i32),
    OptionalUnsignedInt(Option<u32>),
    OptionalLivingEntityReference(Option<uuid::Uuid>),
    VillagerData {
        kind: i32,
        profession: i32,
        level: i32,
    },
}

impl From<&crate::boundary::EntityAppearance> for EntityAppearanceImage {
    fn from(appearance: &crate::boundary::EntityAppearance) -> Self {
        Self {
            item: appearance.item.clone(),
            experience_orb: appearance.experience_orb,
            spawn_data: appearance.spawn_data,
            equipment: appearance.equipment.to_vec(),
            baby: appearance.baby,
            pose: appearance.pose,
            shared_flags: appearance.shared_flags,
            health: appearance.health,
            custom_name: appearance.custom_name.clone(),
            custom_name_visible: appearance.custom_name_visible,
            extra: appearance
                .extra
                .iter()
                .map(|(index, value)| (*index, EntityDataValueImage::from(value)))
                .collect(),
        }
    }
}

impl From<&crate::boundary::EntityDataValue> for EntityDataValueImage {
    fn from(value: &crate::boundary::EntityDataValue) -> Self {
        match value {
            crate::boundary::EntityDataValue::Byte(v) => Self::Byte(*v),
            crate::boundary::EntityDataValue::VarInt(v) => Self::VarInt(*v),
            crate::boundary::EntityDataValue::VarLong(v) => Self::VarLong(*v),
            crate::boundary::EntityDataValue::Float(v) => Self::Float(*v),
            crate::boundary::EntityDataValue::Rotations(v) => Self::Rotations(*v),
            crate::boundary::EntityDataValue::Boolean(v) => Self::Boolean(*v),
            crate::boundary::EntityDataValue::WeatheringCopper(v) => Self::WeatheringCopper(*v),
            crate::boundary::EntityDataValue::CopperGolemState(v) => Self::CopperGolemState(*v),
            crate::boundary::EntityDataValue::Direction(v) => Self::Direction(*v),
            crate::boundary::EntityDataValue::PaintingVariant(v) => Self::PaintingVariant(*v),
            crate::boundary::EntityDataValue::String(v) => Self::String(v.clone()),
            crate::boundary::EntityDataValue::BlockState(v) => Self::BlockState(*v),
            crate::boundary::EntityDataValue::Item(v) => Self::Item(v.clone()),
            crate::boundary::EntityDataValue::OptionalBlockPos(v) => Self::OptionalBlockPos(*v),
            crate::boundary::EntityDataValue::EntityEffectParticle(v) => {
                Self::EntityEffectParticle(*v)
            }
            crate::boundary::EntityDataValue::OptionalUnsignedInt(v) => {
                Self::OptionalUnsignedInt(*v)
            }
            crate::boundary::EntityDataValue::OptionalLivingEntityReference(v) => {
                Self::OptionalLivingEntityReference(*v)
            }
            crate::boundary::EntityDataValue::VillagerData {
                kind,
                profession,
                level,
            } => Self::VillagerData {
                kind: *kind,
                profession: *profession,
                level: *level,
            },
        }
    }
}

/// Open (and truncate) a file for the journal thread's ownership.
pub fn open_journal_stream(path: &Path) -> Result<File, CoreError> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| CoreError::Storage(err.to_string()))
}

/// Drain helper for tests: read all entries of one batch file.
pub fn read_batch_file(path: &Path) -> Option<Vec<JournalEntry>> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let (lo, hi) = parse_batch_name(&name)?;
    let bytes = std::fs::read(path).ok()?;
    decode_batch(&bytes, lo, hi).ok()
}

/// Block position helpers for journal entries.
#[must_use]
pub fn block_pos_array(pos: BlockPos) -> [i32; 3] {
    [pos.x, pos.y, pos.z]
}

/// Chunk position helpers for journal entries.
#[must_use]
pub fn chunk_pos_array(pos: ChunkPos) -> [i32; 2] {
    [pos.x, pos.z]
}

/// Entity id helpers for journal entries.
#[must_use]
pub fn entity_id_int(id: EntityId) -> i32 {
    id.0
}

/// Read a file fully (recovery helper).
#[must_use]
pub fn read_file_bytes(path: &Path) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}
