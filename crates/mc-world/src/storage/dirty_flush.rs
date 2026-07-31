use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use mc_data::items::ItemRegistry;

use crate::anvil::{
    ChunkPayload, RegionError, chunk_to_payload_with_items_at_tick_for_position, read_region,
    write_region_create_new,
};
use crate::atomic_file;
use crate::block::BlockRegistry;
use crate::chunk::{Chunk, ChunkPos};

use super::read_view::ChunkSnapshot;
use super::{REGION_AXIS_CHUNKS, WorldError, WorldStorage, ensure_chunk_position, region_of};

const DIRTY_FLUSH_STALE_REGION_RETRIES: usize = 3;
static REGION_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub struct DirtyFlushPlan {
    regions: Vec<DirtyFlushRegionPlan>,
    chunks: usize,
    dirty_chunks_at_capture: usize,
    registry: Arc<BlockRegistry>,
    item_registry: Option<Arc<ItemRegistry>>,
    unix_time: u32,
    #[cfg(test)]
    payload_encode_count: Arc<AtomicU64>,
}

#[derive(Debug, Clone)]
struct DirtyFlushRegionPlan {
    region: (i32, i32),
    region_path: PathBuf,
    expected_version: Option<RegionFileVersion>,
    dirty_payloads: Vec<PlannedChunkPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RegionFileVersion {
    len: u64,
    modified: Option<SystemTime>,
}

#[derive(Debug, Clone)]
struct PlannedChunkPayload {
    pos: ChunkPos,
    current_tick: u64,
    dirty_generation: u64,
    snapshot: ChunkSnapshot,
}

#[derive(Debug)]
pub struct DirtyFlushCommit {
    regions: Vec<DirtyFlushRegionCommit>,
}

#[derive(Debug)]
pub struct DirtyFlushInstall {
    regions: Vec<DirtyFlushInstalledRegion>,
    installed_chunks: usize,
}

#[derive(Debug)]
pub struct DirtyFlushSynced {
    regions: Vec<DirtyFlushInstalledRegion>,
    installed_chunks: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirtyFlushFinalize {
    installed_chunks: usize,
    cleaned_chunks: usize,
}

#[derive(Debug)]
struct DirtyFlushRegionCommit {
    region: (i32, i32),
    region_path: PathBuf,
    expected_version: Option<RegionFileVersion>,
    tmp_path: PathBuf,
    chunks: Vec<CommittedChunkPayload>,
}

#[derive(Debug)]
struct DirtyFlushInstalledRegion {
    region_path: PathBuf,
    chunks: Vec<CommittedChunkPayload>,
}

impl Drop for DirtyFlushRegionCommit {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.tmp_path);
    }
}

impl DirtyFlushInstall {
    pub fn sync(self) -> Result<DirtyFlushSynced, WorldError> {
        let mut synced_parents = HashSet::new();
        for region in &self.regions {
            let parent = region
                .region_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf();
            if synced_parents.insert(parent) {
                sync_parent_dir(&region.region_path)?;
            }
        }
        Ok(DirtyFlushSynced {
            regions: self.regions,
            installed_chunks: self.installed_chunks,
        })
    }
}

impl DirtyFlushFinalize {
    #[must_use]
    pub fn installed_chunks(self) -> usize {
        self.installed_chunks
    }

    #[must_use]
    pub fn cleaned_chunks(self) -> usize {
        self.cleaned_chunks
    }
}

#[derive(Debug, Clone)]
struct CommittedChunkPayload {
    pos: ChunkPos,
    dirty_generation: u64,
    snapshot: ChunkSnapshot,
}

#[cfg(test)]
type ChunkSnapshotToken = usize;

#[cfg(test)]
fn chunk_snapshot_token(chunk: &ChunkSnapshot) -> ChunkSnapshotToken {
    Arc::as_ptr(chunk) as ChunkSnapshotToken
}

#[cfg(test)]
fn encode_dirty_flush_chunk_payload(
    chunk: &Chunk,
    expected_pos: ChunkPos,
    registry: &BlockRegistry,
    item_registry: Option<&ItemRegistry>,
    now: u32,
    current_tick: u64,
    payload_encode_count: &AtomicU64,
) -> Result<ChunkPayload, WorldError> {
    payload_encode_count.fetch_add(1, Ordering::Relaxed);
    chunk_to_payload_with_items_at_tick_for_position(
        chunk,
        expected_pos,
        registry,
        item_registry,
        now,
        current_tick,
    )
    .map_err(WorldError::from)
}

#[cfg(not(test))]
fn encode_dirty_flush_chunk_payload(
    chunk: &Chunk,
    expected_pos: ChunkPos,
    registry: &BlockRegistry,
    item_registry: Option<&ItemRegistry>,
    now: u32,
    current_tick: u64,
) -> Result<ChunkPayload, WorldError> {
    chunk_to_payload_with_items_at_tick_for_position(
        chunk,
        expected_pos,
        registry,
        item_registry,
        now,
        current_tick,
    )
    .map_err(WorldError::from)
}

impl DirtyFlushPlan {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chunks == 0
    }

    #[must_use]
    pub fn chunk_count(&self) -> usize {
        self.chunks
    }

    #[must_use]
    pub fn dirty_chunks_at_capture(&self) -> usize {
        self.dirty_chunks_at_capture
    }

    #[must_use]
    pub fn captures_all_dirty_chunks(&self) -> bool {
        self.chunks == self.dirty_chunks_at_capture
    }

    pub fn write(self) -> Result<DirtyFlushCommit, WorldError> {
        let DirtyFlushPlan {
            regions,
            registry,
            item_registry,
            unix_time,
            #[cfg(test)]
            payload_encode_count,
            ..
        } = self;
        let mut commits = Vec::with_capacity(regions.len());
        for region in regions {
            if region_file_version(&region.region_path)?.as_ref()
                != region.expected_version.as_ref()
            {
                return Err(WorldError::StaleRegion(region.region_path));
            }
            let mut by_slot: HashMap<(u8, u8), ChunkPayload> = if region.expected_version.is_some()
            {
                read_region(&region.region_path)?
                    .into_iter()
                    .map(|p| ((p.local_x, p.local_z), p))
                    .collect()
            } else {
                HashMap::new()
            };

            let mut committed_chunks = Vec::with_capacity(region.dirty_payloads.len());
            for planned in region.dirty_payloads {
                ensure_chunk_position(planned.pos, planned.snapshot.pos)?;
                #[cfg(test)]
                let payload = encode_dirty_flush_chunk_payload(
                    &planned.snapshot,
                    planned.pos,
                    &registry,
                    item_registry.as_deref(),
                    unix_time,
                    planned.current_tick,
                    &payload_encode_count,
                )?;
                #[cfg(not(test))]
                let payload = encode_dirty_flush_chunk_payload(
                    &planned.snapshot,
                    planned.pos,
                    &registry,
                    item_registry.as_deref(),
                    unix_time,
                    planned.current_tick,
                )?;
                let local = (
                    planned.pos.x.rem_euclid(REGION_AXIS_CHUNKS) as u8,
                    planned.pos.z.rem_euclid(REGION_AXIS_CHUNKS) as u8,
                );
                debug_assert_eq!(local, (payload.local_x, payload.local_z));
                by_slot.insert(local, payload);
                committed_chunks.push(CommittedChunkPayload {
                    pos: planned.pos,
                    dirty_generation: planned.dirty_generation,
                    snapshot: planned.snapshot,
                });
            }

            let mut payloads: Vec<ChunkPayload> = by_slot.into_values().collect();
            payloads.sort_by_key(|p| (p.local_z, p.local_x));

            let tmp_path = write_unique_region_tmp(&region.region_path, &payloads)?;

            commits.push(DirtyFlushRegionCommit {
                region: region.region,
                region_path: region.region_path,
                expected_version: region.expected_version,
                tmp_path,
                chunks: committed_chunks,
            });
        }

        Ok(DirtyFlushCommit { regions: commits })
    }

    #[cfg(test)]
    fn payload_encode_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.payload_encode_count)
    }
}

fn install_region_file(
    region_path: &Path,
    tmp_path: &Path,
    expected_version: Option<&RegionFileVersion>,
) -> Result<(), WorldError> {
    if region_file_version(region_path)?.as_ref() != expected_version {
        return Err(WorldError::StaleRegion(region_path.to_path_buf()));
    }

    if expected_version.is_some() {
        install_existing_region_file(region_path, tmp_path, expected_version)?;
    } else {
        install_new_region_file(region_path, tmp_path)?;
    }
    Ok(())
}

fn install_existing_region_file(
    region_path: &Path,
    tmp_path: &Path,
    expected_version: Option<&RegionFileVersion>,
) -> Result<(), WorldError> {
    if region_file_version(region_path)?.as_ref() != expected_version {
        let _ = std::fs::remove_file(tmp_path);
        return Err(WorldError::StaleRegion(region_path.to_path_buf()));
    }

    atomic_file::replace_file(tmp_path, region_path).map_err(|source| {
        WorldError::Region(RegionError::Io {
            path: region_path.to_path_buf(),
            source,
        })
    })
}

fn install_new_region_file(region_path: &Path, tmp_path: &Path) -> Result<(), WorldError> {
    let result = std::fs::hard_link(tmp_path, region_path);
    let _ = std::fs::remove_file(tmp_path);
    match result {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            Err(WorldError::StaleRegion(region_path.to_path_buf()))
        }
        Err(e) => Err(WorldError::Region(RegionError::Io {
            path: region_path.to_path_buf(),
            source: e,
        })),
    }
}

fn region_file_version(path: &Path) -> Result<Option<RegionFileVersion>, WorldError> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(Some(RegionFileVersion {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(WorldError::Region(RegionError::Io {
            path: path.to_path_buf(),
            source: e,
        })),
    }
}

fn write_unique_region_tmp(
    region_path: &Path,
    payloads: &[ChunkPayload],
) -> Result<PathBuf, WorldError> {
    for _ in 0..16 {
        let tmp_path = unique_region_tmp_path(region_path);
        match write_region_create_new(&tmp_path, payloads) {
            Ok(()) => return Ok(tmp_path),
            Err(RegionError::Io { source, .. }) if source.kind() == ErrorKind::AlreadyExists => {}
            Err(err) => {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(WorldError::from(err));
            }
        }
    }
    Err(WorldError::Region(RegionError::Io {
        path: region_path.to_path_buf(),
        source: std::io::Error::new(
            ErrorKind::AlreadyExists,
            "could not create unique region temp file",
        ),
    }))
}

fn unique_region_tmp_path(region_path: &Path) -> PathBuf {
    let seq = REGION_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let file_name = region_path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "region.mca".into());
    region_path.with_file_name(format!(".{file_name}.tmp.{pid}.{seq}"))
}

fn sync_parent_dir(path: &Path) -> Result<(), WorldError> {
    atomic_file::sync_parent_dir(path).map_err(|source| {
        WorldError::Region(RegionError::Io {
            path: path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf(),
            source,
        })
    })
}

impl WorldStorage {
    /// Build a dirty chunk flush plan. The plan owns dirty chunk snapshots and
    /// the region versions observed while planning so callers can encode and
    /// write region files after releasing any outer world mutex without
    /// replacing a newer region snapshot.
    pub fn plan_dirty_flush(&self) -> Result<DirtyFlushPlan, WorldError> {
        self.plan_dirty_flush_at_tick(0)
    }

    pub fn plan_dirty_flush_at_tick(
        &self,
        current_tick: u64,
    ) -> Result<DirtyFlushPlan, WorldError> {
        self.plan_dirty_flush_at_tick_bounded(current_tick, usize::MAX)
    }

    /// Build one bounded pressure-flush batch. This fast path caps the retained
    /// plan and encoding/write work; full checkpoints use the unbounded planner.
    pub fn plan_dirty_flush_at_tick_bounded(
        &self,
        current_tick: u64,
        max_chunks: usize,
    ) -> Result<DirtyFlushPlan, WorldError> {
        self.ensure_writable()?;
        let (dirty_chunks_at_capture, mut dirty_snapshots) = self.resident.dirty_flush_snapshot();
        dirty_snapshots.sort_by_key(|(pos, _)| {
            (
                pos.x.div_euclid(REGION_AXIS_CHUNKS),
                pos.z.div_euclid(REGION_AXIS_CHUNKS),
                pos.z,
                pos.x,
            )
        });
        dirty_snapshots.truncate(max_chunks);
        if dirty_snapshots.is_empty() {
            return Ok(DirtyFlushPlan {
                regions: Vec::new(),
                chunks: 0,
                dirty_chunks_at_capture,
                registry: Arc::clone(&self.registry),
                item_registry: self.item_registry.as_ref().map(Arc::clone),
                unix_time: 0,
                #[cfg(test)]
                payload_encode_count: Arc::new(AtomicU64::new(0)),
            });
        }
        let mut by_region: HashMap<(i32, i32), Vec<(ChunkPos, ChunkSnapshot)>> = HashMap::new();
        for (pos, chunk) in dirty_snapshots {
            ensure_chunk_position(pos, chunk.pos)?;
            by_region
                .entry(region_of(pos))
                .or_default()
                .push((pos, chunk));
        }

        let now = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as u32)
            .unwrap_or(0);
        let mut regions = Vec::with_capacity(by_region.len());
        let mut chunks = 0usize;
        for ((rx, rz), mut snapshots) in by_region {
            snapshots.sort_by_key(|(pos, _)| (pos.z, pos.x));
            let region_path = self.region_root.join(format!("r.{rx}.{rz}.mca"));
            let expected_version = region_file_version(&region_path)?;
            let mut dirty_payloads = Vec::with_capacity(snapshots.len());
            for (cpos, chunk) in snapshots {
                dirty_payloads.push(PlannedChunkPayload {
                    pos: cpos,
                    current_tick,
                    dirty_generation: chunk.dirty_generation,
                    snapshot: Arc::clone(&chunk),
                });
                chunks += 1;
            }
            regions.push(DirtyFlushRegionPlan {
                region: (rx, rz),
                region_path,
                expected_version,
                dirty_payloads,
            });
        }
        regions.sort_by_key(|region| region.region);

        Ok(DirtyFlushPlan {
            regions,
            chunks,
            dirty_chunks_at_capture,
            registry: Arc::clone(&self.registry),
            item_registry: self.item_registry.as_ref().map(Arc::clone),
            unix_time: now,
            #[cfg(test)]
            payload_encode_count: Arc::new(AtomicU64::new(0)),
        })
    }

    #[must_use]
    pub fn has_flushable_dirty_chunks(&self) -> bool {
        self.resident.has_flushable_dirty()
    }

    /// Commit a written flush plan. Chunks are marked clean only if their dirty
    /// generation still permits the comparison and the encoded payload still
    /// matches the payload that was written. Chunks changed after planning
    /// remain dirty.
    pub fn commit_dirty_flush(&mut self, commit: DirtyFlushCommit) -> Result<usize, WorldError> {
        let installed = self.install_dirty_flush(commit)?;
        let synced = installed.sync()?;
        Ok(self.finalize_dirty_flush(synced).cleaned_chunks())
    }

    /// Install the exact planned region image for an externally serialized
    /// save barrier. Chunks changed after that barrier stay dirty in memory.
    pub fn commit_dirty_flush_snapshot(
        &mut self,
        commit: DirtyFlushCommit,
    ) -> Result<usize, WorldError> {
        let installed = self.install_dirty_flush_snapshot(commit)?;
        let synced = installed.sync()?;
        Ok(self.finalize_dirty_flush(synced).installed_chunks())
    }

    pub fn install_dirty_flush(
        &mut self,
        commit: DirtyFlushCommit,
    ) -> Result<DirtyFlushInstall, WorldError> {
        self.install_dirty_flush_with_mode(commit, false)
    }

    pub fn install_dirty_flush_snapshot(
        &mut self,
        commit: DirtyFlushCommit,
    ) -> Result<DirtyFlushInstall, WorldError> {
        self.install_dirty_flush_with_mode(commit, true)
    }

    fn install_dirty_flush_with_mode(
        &mut self,
        mut commit: DirtyFlushCommit,
        barrier_snapshot: bool,
    ) -> Result<DirtyFlushInstall, WorldError> {
        let mut installed_regions = Vec::with_capacity(commit.regions.len());
        let mut installed_chunks = 0usize;
        for mut region in commit.regions.drain(..) {
            let chunks = std::mem::take(&mut region.chunks);
            let planned = chunks
                .iter()
                .map(|chunk| {
                    (
                        chunk.pos,
                        chunk.dirty_generation,
                        Arc::clone(&chunk.snapshot),
                    )
                })
                .collect::<Vec<_>>();
            let install_result = if barrier_snapshot {
                self.resident
                    .install_region_snapshot_flush(&planned, || {
                        install_region_file(
                            &region.region_path,
                            &region.tmp_path,
                            region.expected_version.as_ref(),
                        )
                    })
                    .map(|()| true)
            } else {
                self.resident.install_region_flush(&planned, || {
                    install_region_file(
                        &region.region_path,
                        &region.tmp_path,
                        region.expected_version.as_ref(),
                    )
                })
            };
            let current = match install_result {
                Ok(current) => current,
                Err(error) => {
                    self.recover_partial_dirty_flush_install(installed_regions, installed_chunks)?;
                    return Err(error);
                }
            };
            if !current {
                // A live chunk changed while this whole-region image encoded.
                // Drop removes the temp image; the region stays dirty for replan.
                continue;
            }
            installed_chunks += chunks.len();
            installed_regions.push(DirtyFlushInstalledRegion {
                region_path: region.region_path.clone(),
                chunks,
            });
            self.regions.remove(&region.region);
            self.region_lru.retain(|&key| key != region.region);
        }
        Ok(DirtyFlushInstall {
            regions: installed_regions,
            installed_chunks,
        })
    }

    fn recover_partial_dirty_flush_install(
        &mut self,
        regions: Vec<DirtyFlushInstalledRegion>,
        installed_chunks: usize,
    ) -> Result<(), WorldError> {
        if regions.is_empty() {
            return Ok(());
        }
        let synced = DirtyFlushInstall {
            regions,
            installed_chunks,
        }
        .sync()?;
        let _ = self.finalize_dirty_flush(synced);
        Ok(())
    }

    pub fn finalize_dirty_flush(&mut self, synced: DirtyFlushSynced) -> DirtyFlushFinalize {
        let mut cleaned_chunks = 0usize;
        for region in synced.regions {
            let planned = region
                .chunks
                .into_iter()
                .map(|chunk| (chunk.pos, chunk.dirty_generation, chunk.snapshot))
                .collect::<Vec<_>>();
            cleaned_chunks += self.resident.finalize_region_flush(planned);
        }
        DirtyFlushFinalize {
            installed_chunks: synced.installed_chunks,
            cleaned_chunks,
        }
    }

    /// M6.b: write every dirty chunk in the cache back to its
    /// `.mca` region file. Returns the number of chunks flushed.
    /// Groups dirty chunks by region so each `r.X.Z.mca` is rewritten
    /// at most once per call.
    pub fn flush_dirty(&mut self) -> Result<usize, WorldError> {
        self.flush_dirty_at_tick(0)
    }

    pub fn flush_dirty_at_tick(&mut self, current_tick: u64) -> Result<usize, WorldError> {
        self.flush_dirty_at_tick_with_pre_write_hook(current_tick, |_| {})
    }

    fn flush_dirty_at_tick_with_pre_write_hook(
        &mut self,
        current_tick: u64,
        mut pre_write: impl FnMut(&DirtyFlushPlan),
    ) -> Result<usize, WorldError> {
        let mut stale_retries = 0usize;
        let mut flushed_chunks = 0usize;
        loop {
            let plan = match self.plan_dirty_flush_at_tick(current_tick) {
                Ok(plan) => plan,
                Err(error) => {
                    self.mark_save_unhealthy();
                    return Err(error);
                }
            };
            if !plan.captures_all_dirty_chunks() {
                self.mark_save_unhealthy();
                return Err(WorldError::JournalPendingDirtyChunks {
                    dirty_chunks: plan.dirty_chunks_at_capture(),
                    flushable_chunks: plan.chunk_count(),
                });
            }
            if plan.is_empty() {
                self.mark_save_healthy();
                return Ok(flushed_chunks);
            }
            let planned_chunks = plan.chunk_count();
            pre_write(&plan);
            match plan
                .write()
                .and_then(|commit| self.commit_dirty_flush(commit))
            {
                Ok(cleaned) => {
                    flushed_chunks = flushed_chunks.saturating_add(cleaned);
                    if cleaned == planned_chunks {
                        self.mark_save_healthy();
                        return Ok(flushed_chunks);
                    }
                    if stale_retries < DIRTY_FLUSH_STALE_REGION_RETRIES {
                        stale_retries += 1;
                        continue;
                    }
                    self.mark_save_unhealthy();
                    return Err(WorldError::ResidentChangedDuringFlush {
                        attempts: stale_retries.saturating_add(1),
                        remaining_dirty: self.dirty_count(),
                    });
                }
                Err(WorldError::StaleRegion(_))
                    if stale_retries < DIRTY_FLUSH_STALE_REGION_RETRIES =>
                {
                    stale_retries += 1;
                }
                Err(err) => {
                    self.mark_save_unhealthy();
                    return Err(err);
                }
            }
        }
    }

    /// Number of dirty chunks currently in the cache. Used by tests
    /// and the Ctrl-C shutdown log.
    #[must_use]
    pub fn dirty_count(&self) -> usize {
        self.resident.dirty_count()
    }
}

#[cfg(test)]
#[path = "dirty_flush_tests.rs"]
mod tests;
