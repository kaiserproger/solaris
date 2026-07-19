use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use mc_data::items::ItemRegistry;

use crate::anvil::{
    ChunkPayload, RegionError, chunk_to_payload_with_items_at_tick, read_region,
    write_region_create_new,
};
use crate::block::BlockRegistry;
use crate::chunk::{Chunk, ChunkPos};

use super::read_view::ChunkSnapshot;
use super::{REGION_AXIS_CHUNKS, WorldError, WorldStorage, make_cached_chunk_mut, region_of};

const DIRTY_FLUSH_STALE_REGION_RETRIES: usize = 3;
static REGION_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub struct DirtyFlushPlan {
    regions: Vec<DirtyFlushRegionPlan>,
    chunks: usize,
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
    #[cfg(test)]
    snapshot_token: ChunkSnapshotToken,
}

#[derive(Debug, Clone)]
pub struct DirtyFlushCommit {
    regions: Vec<DirtyFlushRegionCommit>,
}

#[derive(Debug, Clone)]
struct DirtyFlushRegionCommit {
    region: (i32, i32),
    chunks: Vec<CommittedChunkPayload>,
}

#[derive(Debug, Clone)]
struct CommittedChunkPayload {
    pos: ChunkPos,
    current_tick: u64,
    dirty_generation: u64,
    snapshot: ChunkSnapshot,
    #[cfg(test)]
    snapshot_token: ChunkSnapshotToken,
    #[cfg(test)]
    payload_digest: u64,
    uncompressed_nbt: Vec<u8>,
}

#[cfg(test)]
type ChunkSnapshotToken = usize;

#[cfg(test)]
fn chunk_snapshot_token(chunk: &ChunkSnapshot) -> ChunkSnapshotToken {
    Arc::as_ptr(chunk) as ChunkSnapshotToken
}

#[cfg(test)]
fn payload_digest(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
fn encode_dirty_flush_chunk_payload(
    chunk: &Chunk,
    registry: &BlockRegistry,
    item_registry: Option<&ItemRegistry>,
    now: u32,
    current_tick: u64,
    payload_encode_count: &AtomicU64,
) -> Result<ChunkPayload, WorldError> {
    payload_encode_count.fetch_add(1, Ordering::Relaxed);
    chunk_to_payload_with_items_at_tick(chunk, registry, item_registry, now, current_tick)
        .map_err(WorldError::from)
}

#[cfg(not(test))]
fn encode_dirty_flush_chunk_payload(
    chunk: &Chunk,
    registry: &BlockRegistry,
    item_registry: Option<&ItemRegistry>,
    now: u32,
    current_tick: u64,
) -> Result<ChunkPayload, WorldError> {
    chunk_to_payload_with_items_at_tick(chunk, registry, item_registry, now, current_tick)
        .map_err(WorldError::from)
}

fn can_fast_clean_chunk(
    chunk: &ChunkSnapshot,
    planned_generation: u64,
    planned_snapshot: &ChunkSnapshot,
) -> bool {
    planned_generation != 0
        && chunk.dirty_generation == planned_generation
        && Arc::ptr_eq(chunk, planned_snapshot)
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
                #[cfg(test)]
                let payload = encode_dirty_flush_chunk_payload(
                    &planned.snapshot,
                    &registry,
                    item_registry.as_deref(),
                    unix_time,
                    planned.current_tick,
                    &payload_encode_count,
                )?;
                #[cfg(not(test))]
                let payload = encode_dirty_flush_chunk_payload(
                    &planned.snapshot,
                    &registry,
                    item_registry.as_deref(),
                    unix_time,
                    planned.current_tick,
                )?;
                by_slot.insert((payload.local_x, payload.local_z), payload.clone());
                committed_chunks.push(CommittedChunkPayload {
                    pos: planned.pos,
                    current_tick: planned.current_tick,
                    dirty_generation: planned.dirty_generation,
                    snapshot: planned.snapshot,
                    #[cfg(test)]
                    snapshot_token: planned.snapshot_token,
                    #[cfg(test)]
                    payload_digest: payload_digest(&payload.uncompressed_nbt),
                    uncompressed_nbt: payload.uncompressed_nbt,
                });
            }

            let mut payloads: Vec<ChunkPayload> = by_slot.into_values().collect();
            payloads.sort_by_key(|p| (p.local_z, p.local_x));

            replace_region_file(
                &region.region_path,
                &payloads,
                region.expected_version.as_ref(),
            )?;

            commits.push(DirtyFlushRegionCommit {
                region: region.region,
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

fn replace_region_file(
    region_path: &Path,
    payloads: &[ChunkPayload],
    expected_version: Option<&RegionFileVersion>,
) -> Result<(), WorldError> {
    if region_file_version(region_path)?.as_ref() != expected_version {
        return Err(WorldError::StaleRegion(region_path.to_path_buf()));
    }

    #[cfg(windows)]
    if expected_version.is_some() {
        return Err(WorldError::Region(RegionError::Io {
            path: region_path.to_path_buf(),
            source: std::io::Error::new(
                ErrorKind::Unsupported,
                "atomic replacement of existing region files is unsupported on Windows",
            ),
        }));
    }

    let tmp_path = write_unique_region_tmp(region_path, payloads)?;
    if expected_version.is_some() {
        install_existing_region_file(region_path, &tmp_path, expected_version)?;
    } else {
        install_new_region_file(region_path, &tmp_path)?;
    }
    sync_parent_dir(region_path)?;
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

    std::fs::rename(tmp_path, region_path).map_err(|e| {
        let _ = std::fs::remove_file(tmp_path);
        WorldError::Region(RegionError::Io {
            path: region_path.to_path_buf(),
            source: e,
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
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let dir = match std::fs::File::open(parent) {
        Ok(dir) => dir,
        Err(e) if is_unsupported_dir_sync_error(e.kind()) => {
            return Ok(());
        }
        Err(e) => {
            return Err(WorldError::Region(RegionError::Io {
                path: parent.to_path_buf(),
                source: e,
            }));
        }
    };
    match dir.sync_all() {
        Ok(()) => Ok(()),
        Err(e) if is_unsupported_dir_sync_error(e.kind()) => Ok(()),
        Err(e) => Err(WorldError::Region(RegionError::Io {
            path: parent.to_path_buf(),
            source: e,
        })),
    }
}

fn is_unsupported_dir_sync_error(kind: ErrorKind) -> bool {
    kind == ErrorKind::Unsupported || cfg!(windows) && kind == ErrorKind::PermissionDenied
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
        let mut dirty_snapshots: Vec<(ChunkPos, ChunkSnapshot)> = self
            .resident
            .flushable_snapshots()
            .into_iter()
            .filter(|(_, chunk)| chunk.dirty)
            .collect();
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
                registry: Arc::clone(&self.registry),
                item_registry: self.item_registry.as_ref().map(Arc::clone),
                unix_time: 0,
                #[cfg(test)]
                payload_encode_count: Arc::new(AtomicU64::new(0)),
            });
        }
        let mut by_region: HashMap<(i32, i32), Vec<(ChunkPos, ChunkSnapshot)>> = HashMap::new();
        for (pos, chunk) in dirty_snapshots {
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
                    #[cfg(test)]
                    snapshot_token: chunk_snapshot_token(&chunk),
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

        Ok(DirtyFlushPlan {
            regions,
            chunks,
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
        let mut cleaned = 0usize;
        let mut written_regions = Vec::new();
        for region in commit.regions {
            written_regions.push(region.region);
            for planned in region.chunks {
                let CommittedChunkPayload {
                    pos,
                    current_tick,
                    dirty_generation,
                    snapshot,
                    uncompressed_nbt,
                    ..
                } = planned;
                let registry = Arc::clone(&self.registry);
                let item_registry = self.item_registry.clone();
                let cleaned_chunk = self
                    .resident
                    .mutate_snapshot(pos, move |chunk| {
                        if !chunk.dirty {
                            return Ok(false);
                        }
                        if dirty_generation != 0 && chunk.dirty_generation != dirty_generation {
                            return Ok(false);
                        }
                        let matches = if can_fast_clean_chunk(chunk, dirty_generation, &snapshot) {
                            true
                        } else {
                            let current = chunk_to_payload_with_items_at_tick(
                                chunk,
                                &registry,
                                item_registry.as_deref(),
                                0,
                                current_tick,
                            )?;
                            current.uncompressed_nbt == uncompressed_nbt
                        };
                        if matches {
                            drop(snapshot);
                            make_cached_chunk_mut(chunk).dirty = false;
                        }
                        Ok::<_, WorldError>(matches)
                    })
                    .transpose()?
                    .unwrap_or(false);
                cleaned += usize::from(cleaned_chunk);
            }
        }
        for region in written_regions {
            self.regions.remove(&region);
            self.region_lru.retain(|&k| k != region);
        }

        Ok(cleaned)
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
        loop {
            let plan = self.plan_dirty_flush_at_tick(current_tick)?;
            if plan.is_empty() {
                return Ok(0);
            }
            pre_write(&plan);
            match plan.write() {
                Ok(commit) => return self.commit_dirty_flush(commit),
                Err(WorldError::StaleRegion(_))
                    if stale_retries < DIRTY_FLUSH_STALE_REGION_RETRIES =>
                {
                    stale_retries += 1;
                }
                Err(err) => return Err(err),
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
