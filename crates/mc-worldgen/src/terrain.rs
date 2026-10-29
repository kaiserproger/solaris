//! Overworld chunk assembly.
//!
//! The density router owns continents, erosion, mountain ridges, rivers,
//! climate, and caves. This module resolves block palettes and assembles
//! chunks from that deterministic field.
//! Vertical base layers:
//!
//! - `y = geometry.min_y()` → bedrock
//! - `geometry.min_y() < y < height - 3` → stone
//! - `height - 3 ≤ y < height` → dirt
//! - `y = height` → grass_block
//! - `y > height` → air
//!
//! Output is deterministic in `(seed, world_x, world_z)` and independent of
//! chunk generation order.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
#[cfg(feature = "worldgen-counters")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mc_data::Identifier;
use mc_world::chunk::{Chunk, ChunkGeometry, ChunkPos, Heightmap, OVERWORLD_GEOMETRY};
use mc_world::{
    BIOME_DIM, BIOME_VOLUME, BiomeSection, BlockRegistry, BlockStateId, ChunkGenerator,
    PackedBitArray,
};

use crate::noise::fbm_2d;
use crate::vanilla_climate::VanillaClimate;
use crate::vanilla_density::{SurfaceHeightSampler, VanillaDensity};
use crate::village::plan_source::{VillagePlanSet, VillagePlanSource};

mod biome_routing;
mod biome_rules;
mod gameplay_rules;
mod geological_ores;
#[cfg(feature = "gpu")]
mod gpu_service;
mod ore_rules;
mod ore_veins;
mod overworld;
mod sediments;
mod solid_cache;
mod solids;
mod trees;

pub use biome_rules::BiomeRules;
pub use gameplay_rules::{ClayRule, TreeRule};
use geological_ores::GeologicalOreRules;
pub use ore_rules::{
    BiomeScope, MAX_ORE_RULES, MAX_ORE_WORK_UNITS_PER_CHUNK, OreRule, OreRules, OreRulesError,
    OreSpacing, YRange,
};
use ore_rules::{
    MAX_ORE_ANCHORS_PER_CELL, MAX_ORE_VEIN_SIZE, ORE_ANCHOR_CELL_EDGE, ORE_ANCHOR_CELL_VOLUME,
};
use overworld::{OverworldRouter, TerrainSample};
pub use solids::{ChunkSolids, SolidGrid};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TerrainGeneratorError {
    #[error("block registry missing required terrain block {name}")]
    MissingRequiredBlock { name: &'static str },
}

pub const SEA_LEVEL: i32 = 63;
/// Number of dirt cells between grass cap and stone.
const DIRT_DEPTH: i32 = 3;
/// Scale and coverage of gravel bars on river beds.
const RIVER_GRAVEL_SCALE: f64 = 34.0;
const RIVER_GRAVEL_THRESHOLD: f64 = 0.30;
const RIVER_GRAVEL_SALT: i64 = 0x5247_5256;
const ORE_VEIN_RADIUS: i32 = 4;
const ORE_COLUMN_HALO: i32 = ORE_VEIN_RADIUS * 2 + 1;
const ORE_DIRECTIONS: [[i8; 3]; 6] = [
    [-1, 0, 0],
    [1, 0, 0],
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
];
const CAVE_SURFACE_CLEARANCE: i32 = 32;
const CAVE_VERTICAL_SAMPLE_STEP: i32 = 2;
const DEEPSLATE_TOP_Y: i32 = 8;
const DEEPSLATE_SOLID_Y: i32 = 0;
const VEGETATION_REGION_SCALE: f64 = 192.0;
/// Columns per plains tree outside groves: vanilla `trees_plains` places one
/// tree with 5% chance per chunk (256 columns / 0.05).
const GRASSLAND_SPARSE_TREE_SPACING: u64 = 5120;
const SPAWN_SEARCH_STEP_BLOCKS: i32 = 64;
const SPAWN_SEARCH_MAX_RADIUS_BLOCKS: i32 = 8_192;
const SPAWN_SITE_SAMPLE_RADIUS_BLOCKS: i32 = 8;
const SPAWN_SITE_MAX_RELIEF: i32 = 3;
const DIAGNOSTIC_CACHE_SHARDS: usize = 64;
const DIAGNOSTIC_CACHE_ENTRIES_PER_SHARD: usize = 2_048;

#[cfg(feature = "worldgen-counters")]
static HEIGHT_REQUESTS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "worldgen-counters")]
static HEIGHT_UNIQUE_KEYS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "worldgen-counters")]
static HEIGHT_CACHE_HITS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "worldgen-counters")]
static HEIGHT_COLUMN_SEARCHES: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "worldgen-counters")]
static HEIGHT_COLUMN_ITERATIONS: AtomicU64 = AtomicU64::new(0);

/// Snapshot of the generation-local height-context cache counters.
///
/// Only populated with the `worldgen-counters` feature; without it every
/// increment below compiles out and collection costs nothing.
///
/// Field semantics: `base_height_requests` counts every `source_height`
/// entry; `unique_height_keys` counts distinct columns ever inserted (one
/// insert each — NOT counting `for_chunk` seeding, which inserts without a
/// request); `height_cache_hits` counts map hits on the request path;
/// `column_iterations_from_height_queries` counts top-down Y-steps of the
/// column search (`max_y - 1 - raw`, off by one vs successful samples by
/// construction — a diagnostic shape metric, NOT noise evaluations: memo
/// and Min/Max change actual evaluations independently; use DensityCounters
/// for those); `actual_density_column_searches` counts the searches.
#[cfg(feature = "worldgen-counters")]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HeightContextStats {
    pub source_hits: u64,
    pub source_misses: u64,
    pub plan_hits: u64,
    pub plan_misses: u64,
    pub base_height_requests: u64,
    pub unique_height_keys: u64,
    pub height_cache_hits: u64,
    pub actual_density_column_searches: u64,
    pub column_iterations_from_height_queries: u64,
}

/// Read the height-context counters accumulated so far.
#[cfg(feature = "worldgen-counters")]
#[must_use]
pub fn height_context_stats() -> HeightContextStats {
    HeightContextStats {
        source_hits: HEIGHT_REQUESTS.load(Ordering::Relaxed),
        source_misses: HEIGHT_UNIQUE_KEYS.load(Ordering::Relaxed),
        plan_hits: HEIGHT_CACHE_HITS.load(Ordering::Relaxed),
        plan_misses: HEIGHT_COLUMN_SEARCHES.load(Ordering::Relaxed),
        base_height_requests: HEIGHT_REQUESTS.load(Ordering::Relaxed),
        unique_height_keys: HEIGHT_UNIQUE_KEYS.load(Ordering::Relaxed),
        height_cache_hits: HEIGHT_CACHE_HITS.load(Ordering::Relaxed),
        actual_density_column_searches: HEIGHT_COLUMN_SEARCHES.load(Ordering::Relaxed),
        column_iterations_from_height_queries: HEIGHT_COLUMN_ITERATIONS.load(Ordering::Relaxed),
    }
}

/// Zero the height-context counters, e.g. between benchmark rounds.
#[cfg(feature = "worldgen-counters")]
pub fn reset_height_context_stats() {
    HEIGHT_REQUESTS.store(0, Ordering::Relaxed);
    HEIGHT_UNIQUE_KEYS.store(0, Ordering::Relaxed);
    HEIGHT_CACHE_HITS.store(0, Ordering::Relaxed);
    HEIGHT_COLUMN_SEARCHES.store(0, Ordering::Relaxed);
    HEIGHT_COLUMN_ITERATIONS.store(0, Ordering::Relaxed);
}

#[inline]
fn count_height_request() {
    #[cfg(feature = "worldgen-counters")]
    HEIGHT_REQUESTS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
fn count_height_unique_key() {
    #[cfg(feature = "worldgen-counters")]
    HEIGHT_UNIQUE_KEYS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
fn count_height_cache_hit() {
    #[cfg(feature = "worldgen-counters")]
    HEIGHT_CACHE_HITS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
fn count_height_column_search(column_iterations: u64) {
    #[cfg(feature = "worldgen-counters")]
    {
        HEIGHT_COLUMN_SEARCHES.fetch_add(1, Ordering::Relaxed);
        HEIGHT_COLUMN_ITERATIONS.fetch_add(column_iterations, Ordering::Relaxed);
    }
    #[cfg(not(feature = "worldgen-counters"))]
    let _ = column_iterations;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpawnLocation {
    pub block_x: i32,
    pub surface_y: i32,
    pub block_z: i32,
}

/// Read-only view of the production column plan for diagnostics and tooling.
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainDiagnosticSample {
    pub surface_y: i32,
    pub biome: Identifier,
    /// The production vegetation density for biomes that support land vegetation.
    pub vegetation_density: Option<f64>,
}

/// Derived column facts, with bounded FIFO eviction rather than whole-shard
/// invalidation. Sampling remains outside the shard lock.
struct DiagnosticCache {
    shards: [Mutex<DiagnosticCacheShard>; DIAGNOSTIC_CACHE_SHARDS],
}

#[derive(Default)]
struct DiagnosticCacheShard {
    entries: HashMap<(i32, i32), TerrainDiagnosticSample>,
    order: VecDeque<(i32, i32)>,
}

impl Default for DiagnosticCache {
    fn default() -> Self {
        Self {
            shards: std::array::from_fn(|_| Mutex::new(DiagnosticCacheShard::default())),
        }
    }
}

impl DiagnosticCache {
    fn shard_index(world_x: i32, world_z: i32) -> usize {
        let mixed = (world_x as u32 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (world_z as u32 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
        (mixed as usize) & (DIAGNOSTIC_CACHE_SHARDS - 1)
    }

    fn get(&self, world_x: i32, world_z: i32) -> Option<TerrainDiagnosticSample> {
        let shard = self.shards[Self::shard_index(world_x, world_z)]
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        shard.entries.get(&(world_x, world_z)).cloned()
    }

    fn insert(
        &self,
        world_x: i32,
        world_z: i32,
        sample: TerrainDiagnosticSample,
    ) -> TerrainDiagnosticSample {
        let mut shard = self.shards[Self::shard_index(world_x, world_z)]
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = shard.entries.get(&(world_x, world_z)) {
            return existing.clone();
        }
        if shard.entries.len() == DIAGNOSTIC_CACHE_ENTRIES_PER_SHARD
            && let Some(evicted) = shard.order.pop_front()
        {
            shard.entries.remove(&evicted);
        }
        shard.entries.insert((world_x, world_z), sample.clone());
        shard.order.push_back((world_x, world_z));
        sample
    }
}

impl SpawnLocation {
    #[must_use]
    pub const fn chunk(self) -> ChunkPos {
        ChunkPos {
            x: self.block_x.div_euclid(16),
            z: self.block_z.div_euclid(16),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum WorldgenMode {
    #[default]
    VanillaLike,
    TellusLike,
}

impl WorldgenMode {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::VanillaLike => "vanilla_like",
            Self::TellusLike => "tellus_like",
        }
    }
}

/// Vanilla-like terrain generator. Holds the resolved state ids of every block
/// state it emits, so `generate` is allocation-free after the `Chunk::empty`
/// allocation it returns.
pub struct TerrainGenerator {
    seed: i64,
    geometry: ChunkGeometry,
    air: BlockStateId,
    bedrock: BlockStateId,
    stone: BlockStateId,
    dirt: BlockStateId,
    grass_block: BlockStateId,
    sand: BlockStateId,
    red_sand: BlockStateId,
    gravel: BlockStateId,
    podzol: BlockStateId,
    snow_block: BlockStateId,
    deepslate: BlockStateId,
    water: BlockStateId,
    cave_lava: BlockStateId,
    terracotta_layers: [BlockStateId; 8],
    biomes: BiomeRules,
    ores: OreRules,
    geological_ores: Option<GeologicalOreRules>,
    ore_generation_profile: &'static str,
    /// Native village plans. `None` leaves terrain routing unadapted and places
    /// no village pieces.
    village_plans: Option<Arc<VillagePlanSource>>,
    overworld_structures: Option<crate::overworld_structures::OverworldStructures>,
    /// Other jigsaw structures retain their own placement grids and biome gates.
    jigsaw_sources: Vec<Arc<VillagePlanSource>>,
    decorations: DecorationBlocks,
    tree_rules: HashMap<Identifier, TreeRule>,
    clay_rule: ClayRule,
    vanilla_worldgen: Option<(VanillaClimate, VanillaDensity)>,
    diagnostic_cache: DiagnosticCache,
    /// Block registry the generator was built with; spawn clearance reads
    /// fluid states from it.
    blocks: Arc<BlockRegistry>,
    /// Batched GPU density (`[server] worldgen_gpu`); `None` evaluates it on
    /// the CPU.
    #[cfg(feature = "gpu")]
    gpu: Option<gpu_service::GpuSolidService>,
    /// Chunk solids shared between the generations that read them.
    solid_cache: solid_cache::SolidCache,
}

#[derive(Clone)]
struct ColumnPlan {
    lx: u8,
    lz: u8,
    wx: i32,
    wz: i32,
    height: i32,
    top_non_air: i32,
    dirt_start: i32,
    biome: Identifier,
    surface: BlockStateId,
    fill: BlockStateId,
    vegetation_density: f64,
    hash: u64,
}

/// Generation-local height context: the chunk's own source heights plus one
/// shared [`SurfaceHeightSampler`] + one village-plan memo the decorations
/// (trees), ore-halo reads and structure writes consult instead of re-running
/// the full height + plan pipeline per neighbour column.
///
/// Cache key discipline: `source_heights` holds each column's SOURCE
/// (pre-village) surface — clamped to `geometry.min_y()..=max_y()-1` — so a
/// village plan can still move the column later without a second SOURCE
/// evaluation. Single village adjustment only: [`HeightContext::adjusted_height`]
/// reads exactly one [`VillagePlanSet`] (the chunk's set for generation-local
/// columns, the column's own chunk's set for halo readers) and applies it once;
/// callers must not pre-adjust or post-adjust.
///
/// Order-independence: every entry is a pure function of `(seed, world_x,
/// world_z)` — SOURCE through the density query, the plan set through the
/// village lookup keyed by chunk, and the final height through the beard
/// analogue — so visitation order only decides hits vs misses, never values.
pub(crate) struct HeightContext<'h> {
    generator: &'h TerrainGenerator,
    geometry_min_y: i32,
    geometry_max_y: i32,
    source_heights: HashMap<(i32, i32), i32>,
    native_sampler: Option<SurfaceHeightSampler<'h>>,
    plans: HashMap<(i32, i32), Option<VillagePlanSet>>,
    /// Precomputed solids, with the density's min y.
    solids: Option<(&'h SolidGrid, i32)>,
}

impl<'h> HeightContext<'h> {
    /// Build a context seeded with the central chunk's ready SOURCE heights.
    /// `source_columns` are the chunk's own entries straight from the
    /// diagnostic cache the fill-density pass inserted (the same entries
    /// `plan_column` planned from); the context re-applies the village set
    /// per read so planned and unplanned readers agree.
    pub(crate) fn for_chunk(
        generator: &'h TerrainGenerator,
        pos: ChunkPos,
        source_columns: &[(i32, i32, i32)],
        native_sampler: Option<SurfaceHeightSampler<'h>>,
    ) -> Self {
        let mut source_heights = HashMap::with_capacity(source_columns.len().max(256));
        for &(wx, wz, base) in source_columns {
            source_heights.insert((wx, wz), base);
        }
        let _ = pos;
        Self {
            generator,
            geometry_min_y: generator.geometry.min_y(),
            geometry_max_y: generator.geometry.max_y(),
            source_heights,
            native_sampler,
            plans: HashMap::new(),
            solids: None,
        }
    }

    /// Answer SOURCE heights and cave checks of the chunks `solids` holds
    /// from them instead of the halo sampler (same answers). `min_y` is the
    /// density's.
    pub(crate) fn with_solids(mut self, solids: &'h SolidGrid, min_y: i32) -> Self {
        self.solids = Some((solids, min_y));
        self
    }

    /// Whether native terrain density leaves one block empty (`<= 0`), from
    /// held solids or the shared halo sampler; `None` without vanilla density.
    fn native_empty(&mut self, world_x: i32, y: i32, world_z: i32) -> Option<bool> {
        if let Some(solid) = self
            .solids
            .and_then(|(solids, min_y)| solids.solid(world_x, y, world_z, min_y))
        {
            return Some(!solid);
        }
        self.native_sampler
            .as_mut()
            .map(|sampler| sampler.density(world_x, y, world_z) <= 0.0)
    }

    /// The SOURCE (pre-village) surface for one column: ready central-chunk
    /// values first, then the shared sampler, then the per-column query.
    fn source_height(&mut self, world_x: i32, world_z: i32) -> i32 {
        count_height_request();
        if let Some(&height) = self.source_heights.get(&(world_x, world_z)) {
            count_height_cache_hit();
            return height;
        }
        // ONE shared sampler: lazily computed halo columns retain lattice
        // corners across columns instead of one sampler per column.
        let held = self
            .solids
            .and_then(|(solids, min_y)| solids.surface_height(world_x, world_z, min_y));
        let raw = if let Some(held) = held.filter(|_| self.native_sampler.is_some()) {
            held
        } else if let Some(sampler) = self.native_sampler.as_mut() {
            sampler.surface_height(world_x, world_z)
        } else {
            self.generator.base_surface_y(world_x, world_z)
        };
        // `base_surface_y` already clamps vanilla heights to the geometry;
        // clamp the sampler path identically so every stored value is final.
        let clamped = raw.clamp(self.geometry_min_y, self.geometry_max_y - 1);
        // The column search walks top-down one `sample_terrain` per Y until
        // the first solid: `column_iterations = max_y - 1 - raw` by
        // construction (off by one vs successful samples — the successful
        // sample yields 0 further steps). Diagnostic shape metric, NOT a
        // noise-evaluation count (see DensityCounters for actual evaluations).
        // The own-router path is a single closed-form column query, counted
        // as one search of one iteration.
        let column_iterations = if self.native_sampler.is_some() {
            (self.geometry_max_y - 1 - raw.min(self.geometry_max_y - 1)).max(0) as u64
        } else {
            1
        };
        count_height_column_search(column_iterations);
        count_height_unique_key();
        self.source_heights.insert((world_x, world_z), clamped);
        clamped
    }

    /// The chunk's plan set, memoized: one lookup per chunk reached.
    /// (No height counters: the plan memo is village work, not a height
    /// query; the required five counters live on the SOURCE path above.)
    fn plans_for_chunk(&mut self, chunk: ChunkPos) -> Option<VillagePlanSet> {
        if let Some(hit) = self.plans.get(&(chunk.x, chunk.z)) {
            return hit.clone();
        }
        let set = self.generator.village_plans_for_chunk(chunk);
        self.plans.insert((chunk.x, chunk.z), set.clone());
        set
    }

    /// Final (village-adjusted) height: cached SOURCE + single adjustment.
    pub(crate) fn adjusted_height(&mut self, world_x: i32, world_z: i32) -> i32 {
        let base = self.source_height(world_x, world_z);
        let chunk = ChunkPos {
            x: world_x.div_euclid(16),
            z: world_z.div_euclid(16),
        };
        let plans = self.plans_for_chunk(chunk);
        self.generator
            .village_surface_y(world_x, world_z, base, plans.as_ref())
    }

    /// Column plan for halo readers that need more than a height: SOURCE
    /// diagnostic (cached by the generator; a miss takes its height from
    /// this context's shared sampler) + this context's single plan set.
    fn halo_column(&mut self, world_x: i32, world_z: i32) -> ColumnPlan {
        let chunk = ChunkPos {
            x: world_x.div_euclid(16),
            z: world_z.div_euclid(16),
        };
        let plans = self.plans_for_chunk(chunk);
        // An independent column query walks the whole height with a fresh
        // sampler (~0.1 ms); the shared lattice answers the same height
        // (`vanilla_halo_surface_heights_match_independent_queries`) for a
        // fraction. Halo planning was 89 of 104 ms of a fresh chunk.
        let diagnostic = match self.generator.diagnostic_cache.get(world_x, world_z) {
            Some(sample) => sample,
            None => {
                let height = self
                    .native_sampler
                    .is_some()
                    .then(|| self.source_height(world_x, world_z));
                let sample = self
                    .generator
                    .diagnostic_sample_uncached(world_x, world_z, height);
                self.generator
                    .diagnostic_cache
                    .insert(world_x, world_z, sample)
            }
        };
        let lx = world_x.rem_euclid(16) as u8;
        let lz = world_z.rem_euclid(16) as u8;
        self.generator
            .plan_column_from_diagnostic(chunk, lx, lz, plans.as_ref(), diagnostic)
    }

    /// WORLD_SURFACE_WG after terrain adaptation, including water. Gravity
    /// must follow the fitted terrain, not restore dirt across an entrance cut.
    pub(crate) fn structure_surface(&mut self, world_x: i32, world_z: i32) -> i32 {
        let base = self.source_height(world_x, world_z);
        let chunk = ChunkPos {
            x: world_x.div_euclid(16),
            z: world_z.div_euclid(16),
        };
        let plans = self
            .plans
            .entry((chunk.x, chunk.z))
            .or_insert_with(|| self.generator.village_plans_for_chunk(chunk));
        let solid_y = self
            .generator
            .village_surface_y(world_x, world_z, base, plans.as_ref());
        self.generator.world_surface_y(solid_y)
    }
    /// Map entries held (seeded + lazily resolved). Test hook only.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.source_heights.len()
    }
}

struct OreColumnCache<'a, 'h> {
    min_x: i64,
    min_z: i64,
    side: usize,
    columns: Vec<Option<ColumnPlan>>,
    /// The generation-local height context: ONE shared native sampler plus
    /// the village-plan memo, owned by the caller. The previous
    /// `native_heights: Option<SurfaceHeightSampler>` + `plans` fields lived
    /// here; they moved into the context so tree, ore and structure readers
    /// share the same sampler and the same plan lookup instead of each
    /// building their own.
    heights: &'a mut HeightContext<'h>,
    cave_min_y: i32,
    cave_layers: usize,
    cave_samples: Vec<u8>,
    raw_cave_samples: Vec<u8>,
    /// Full-density cave answers outside the sampled layers, per cell: ore
    /// veins and their air-exposure scans ask the same cells many times.
    dense_caves: std::collections::HashMap<(i32, i32, i32), bool>,
}

impl OreColumnCache<'_, '_> {
    fn column_index(&self, world_x: i32, world_z: i32) -> Option<usize> {
        let local_x = i64::from(world_x).checked_sub(self.min_x)?;
        let local_z = i64::from(world_z).checked_sub(self.min_z)?;
        let local_x = usize::try_from(local_x).ok()?;
        let local_z = usize::try_from(local_z).ok()?;
        if local_x >= self.side || local_z >= self.side {
            return None;
        }
        Some(local_z * self.side + local_x)
    }

    fn get_or_plan(&mut self, world_x: i32, world_z: i32) -> Option<&ColumnPlan> {
        let index = self.column_index(world_x, world_z)?;
        if self.columns[index].is_none() {
            // The halo reaches into neighbouring chunks, so the plan is the
            // column's own chunk's set — never this chunk's, and never a
            // single column's filtered slice of it (`plan_column` applies the
            // set per column anyway). The context memoizes the lookup per
            // chunk and serves SOURCE heights from one shared sampler, so
            // halo columns cost one plan lookup per chunk and one sampler
            // query per column however many ore cells read them.
            let column = self.heights.halo_column(world_x, world_z);
            self.columns[index] = Some(column);
        }
        self.columns[index].as_ref()
    }

    fn cave_sample_index(&self, world_x: i32, sample_y: i32, world_z: i32) -> Option<usize> {
        let column_index = self.column_index(world_x, world_z)?;
        let delta_y = sample_y.saturating_sub(self.cave_min_y);
        if delta_y < 0 || delta_y % CAVE_VERTICAL_SAMPLE_STEP != 0 {
            return None;
        }
        let layer = usize::try_from(delta_y / CAVE_VERTICAL_SAMPLE_STEP).ok()?;
        if layer >= self.cave_layers {
            return None;
        }
        Some(layer * self.side * self.side + column_index)
    }

    fn record_raw_cave(&mut self, world_x: i32, sample_y: i32, world_z: i32, cave: bool) {
        if let Some(index) = self.cave_sample_index(world_x, sample_y, world_z) {
            self.raw_cave_samples[index] = if cave { 2 } else { 1 };
        }
    }

    fn get_or_raw_cave(
        &mut self,
        generator: &TerrainGenerator,
        world_x: i32,
        sample_y: i32,
        world_z: i32,
    ) -> bool {
        let Some(index) = self.cave_sample_index(world_x, sample_y, world_z) else {
            return generator
                .density_router()
                .raw_cave(world_x, sample_y, world_z);
        };
        match self.raw_cave_samples[index] {
            1 => false,
            2 => true,
            _ => {
                let cave = generator
                    .density_router()
                    .raw_cave(world_x, sample_y, world_z);
                self.raw_cave_samples[index] = if cave { 2 } else { 1 };
                cave
            }
        }
    }

    fn get_or_cave(
        &mut self,
        generator: &TerrainGenerator,
        world_x: i32,
        sample_y: i32,
        world_z: i32,
        surface_y: i32,
    ) -> bool {
        let Some(index) = self.cave_sample_index(world_x, sample_y, world_z) else {
            // Above the sampled cave layers: the full-density test, through
            // the halo sampler's shared lattice corners (same values).
            let heights = &mut *self.heights;
            return *self
                .dense_caves
                .entry((world_x, sample_y, world_z))
                .or_insert_with(|| {
                    sample_y < surface_y
                        && heights
                            .native_empty(world_x, sample_y, world_z)
                            .unwrap_or_else(|| {
                                generator.is_cave_cell(world_x, sample_y, world_z, surface_y)
                            })
                });
        };
        match self.cave_samples[index] {
            1 => false,
            2 => true,
            _ => {
                let cave = sample_y > generator.geometry.min_y().saturating_add(10)
                    && sample_y < surface_y.saturating_sub(CAVE_SURFACE_CLEARANCE).min(32)
                    && self.get_or_raw_cave(generator, world_x, sample_y, world_z)
                    && [(-1, 0), (1, 0), (0, -1), (0, 1)]
                        .into_iter()
                        .any(|(dx, dz)| {
                            world_x
                                .checked_add(dx)
                                .zip(world_z.checked_add(dz))
                                .is_some_and(|(x, z)| {
                                    self.get_or_raw_cave(generator, x, sample_y, z)
                                })
                        });
                self.cave_samples[index] = if cave { 2 } else { 1 };
                cave
            }
        }
    }
}

#[derive(Clone)]
struct DecorationBlocks {
    oak_log: Option<BlockStateId>,
    oak_leaves: Option<BlockStateId>,
    forest_log: Option<BlockStateId>,
    forest_leaves: Option<BlockStateId>,
    cold_log: Option<BlockStateId>,
    cold_leaves: Option<BlockStateId>,
    jungle_log: Option<BlockStateId>,
    jungle_leaves: Option<BlockStateId>,
    acacia_log: Option<BlockStateId>,
    acacia_leaves: Option<BlockStateId>,
    mangrove_log: Option<BlockStateId>,
    mangrove_leaves: Option<BlockStateId>,
    mangrove_roots: Option<BlockStateId>,
    wet_mangrove_roots: Option<BlockStateId>,
    muddy_mangrove_roots: Option<BlockStateId>,
    mud: Option<BlockStateId>,
    blue_orchid: Option<BlockStateId>,
    short_grass: Option<BlockStateId>,
    dandelion: Option<BlockStateId>,
    poppy: Option<BlockStateId>,
    pumpkin: Option<BlockStateId>,
    sugar_cane: Option<BlockStateId>,
    cactus: Option<BlockStateId>,
    seagrass: Option<BlockStateId>,
    clay: Option<BlockStateId>,
    kelp_plant: Option<BlockStateId>,
    kelp: Option<BlockStateId>,
    leaf_distances: [Option<[BlockStateId; 7]>; 6],
    leaf_supports: Vec<bool>,
}

impl DecorationBlocks {
    fn new(registry: &BlockRegistry) -> Self {
        let mut decorations = Self {
            oak_log: optional_block(registry, "minecraft:oak_log"),
            oak_leaves: optional_generated_leaves(registry, "minecraft:oak_leaves"),
            forest_log: optional_block(registry, "minecraft:birch_log")
                .or_else(|| optional_block(registry, "minecraft:oak_log")),
            forest_leaves: optional_generated_leaves(registry, "minecraft:birch_leaves")
                .or_else(|| optional_generated_leaves(registry, "minecraft:oak_leaves")),
            cold_log: optional_block(registry, "minecraft:spruce_log")
                .or_else(|| optional_block(registry, "minecraft:oak_log")),
            cold_leaves: optional_generated_leaves(registry, "minecraft:spruce_leaves")
                .or_else(|| optional_generated_leaves(registry, "minecraft:oak_leaves")),
            jungle_log: optional_block(registry, "minecraft:jungle_log")
                .or_else(|| optional_block(registry, "minecraft:oak_log")),
            jungle_leaves: optional_generated_leaves(registry, "minecraft:jungle_leaves")
                .or_else(|| optional_generated_leaves(registry, "minecraft:oak_leaves")),
            acacia_log: optional_block(registry, "minecraft:acacia_log")
                .or_else(|| optional_block(registry, "minecraft:oak_log")),
            acacia_leaves: optional_generated_leaves(registry, "minecraft:acacia_leaves")
                .or_else(|| optional_generated_leaves(registry, "minecraft:oak_leaves")),
            mangrove_log: optional_block(registry, "minecraft:mangrove_log"),
            mangrove_leaves: optional_generated_leaves(registry, "minecraft:mangrove_leaves"),
            mangrove_roots: optional_block(registry, "minecraft:mangrove_roots"),
            wet_mangrove_roots: None,
            muddy_mangrove_roots: optional_block(registry, "minecraft:muddy_mangrove_roots"),
            mud: optional_block(registry, "minecraft:mud"),
            blue_orchid: optional_block(registry, "minecraft:blue_orchid"),
            short_grass: optional_block(registry, "minecraft:short_grass"),
            dandelion: optional_block(registry, "minecraft:dandelion"),
            poppy: optional_block(registry, "minecraft:poppy"),
            pumpkin: optional_block(registry, "minecraft:pumpkin"),
            sugar_cane: optional_block(registry, "minecraft:sugar_cane"),
            cactus: optional_block(registry, "minecraft:cactus"),
            seagrass: optional_block(registry, "minecraft:seagrass"),
            clay: optional_block(registry, "minecraft:clay"),
            kelp_plant: optional_block(registry, "minecraft:kelp_plant"),
            kelp: optional_block(registry, "minecraft:kelp"),
            leaf_distances: [None; 6],
            leaf_supports: registry
                .states()
                .map(|state| {
                    mc_world::plant_rules_26_1_2::leaf_distance_from_state(registry, state.id) == 0
                })
                .collect(),
        };
        decorations.wet_mangrove_roots = decorations.mangrove_roots.and_then(|root| {
            let state = registry.by_id(root)?;
            let mut properties = state.properties.clone();
            properties
                .iter_mut()
                .find(|(name, _)| name == "waterlogged")?
                .1 = "true".to_owned();
            registry.by_name_and_props(&state.block.id, &properties)
        });
        decorations.leaf_distances = [
            decorations.oak_leaves,
            decorations.forest_leaves,
            decorations.cold_leaves,
            decorations.jungle_leaves,
            decorations.acacia_leaves,
            decorations.mangrove_leaves,
        ]
        .map(|leaf| {
            let leaf = leaf?;
            let state = registry.by_id(leaf)?;
            let mut properties = state.properties.clone();
            let property = properties.iter().position(|(key, _)| key == "distance")?;
            let mut distances = [leaf; 7];
            for (index, distance) in distances.iter_mut().enumerate() {
                properties[property].1 = (index + 1).to_string();
                *distance = registry.by_name_and_props(&state.block.id, &properties)?;
            }
            Some(distances)
        });
        decorations
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TreeKind {
    Oak,
    Birch,
    Spruce,
    Jungle,
    JungleBush,
    Acacia,
    Mangrove,
}

#[derive(Debug, Clone, Copy)]
struct TreeBlocks {
    kind: TreeKind,
    log: BlockStateId,
    leaves: BlockStateId,
}

#[derive(Debug, Clone, Copy)]
struct TreeLeafOffset {
    relative_y: i32,
    dx: i8,
    dz: i8,
    radius: i8,
}

fn tree_canopy_radius(kind: TreeKind, relative_y: i32) -> Option<i8> {
    match (kind, relative_y) {
        (TreeKind::Oak, -2 | -1) => Some(2),
        (TreeKind::Oak, 0 | 1) => Some(1),
        (TreeKind::Birch, -2 | -1) => Some(2),
        (TreeKind::Birch, 0) => Some(1),
        (TreeKind::Birch, 1) => Some(0),
        (TreeKind::Spruce, -4) => Some(1),
        (TreeKind::Spruce, -3 | -2) => Some(2),
        (TreeKind::Spruce, -1 | 0) => Some(1),
        (TreeKind::Spruce, 1) => Some(0),
        (TreeKind::Jungle, -2..=0) => Some(2),
        (TreeKind::Jungle, 1) => Some(1),
        (TreeKind::JungleBush, 0) => Some(2),
        (TreeKind::JungleBush, 1) => Some(1),
        (TreeKind::JungleBush, 2) => Some(0),
        (TreeKind::Acacia, -1 | 0) => Some(2),
        (TreeKind::Acacia, 1) => Some(1),
        (TreeKind::Mangrove, -3..=-1) => Some(2),
        (TreeKind::Mangrove, 0) => Some(1),
        (TreeKind::Mangrove, 1) => Some(0),
        _ => None,
    }
}

fn try_resolve_block(
    registry: &BlockRegistry,
    name: &'static str,
) -> Result<BlockStateId, TerrainGeneratorError> {
    let id = Identifier::parse(name).expect("static identifier");
    registry
        .block(&id)
        .map(|b| b.default)
        .ok_or(TerrainGeneratorError::MissingRequiredBlock { name })
}

fn resolve_block_or(registry: &BlockRegistry, name: &str, fallback: BlockStateId) -> BlockStateId {
    let id = Identifier::parse(name).expect("static identifier");
    registry.block(&id).map(|b| b.default).unwrap_or(fallback)
}

fn optional_block(registry: &BlockRegistry, name: &str) -> Option<BlockStateId> {
    let id = Identifier::parse(name).expect("static identifier");
    registry.block(&id).map(|b| b.default)
}

fn optional_generated_leaves(registry: &BlockRegistry, name: &str) -> Option<BlockStateId> {
    let id = Identifier::parse(name).expect("static identifier");
    registry
        .by_name_and_props(
            &id,
            &[
                ("distance".to_string(), "7".to_string()),
                ("persistent".to_string(), "false".to_string()),
                ("waterlogged".to_string(), "false".to_string()),
            ],
        )
        .or_else(|| registry.block(&id).map(|block| block.default))
}

fn checked_y_offset(y: i32, offset: i32) -> Option<i32> {
    i32::try_from(i64::from(y) + i64::from(offset)).ok()
}

fn heightmap_value_for_top(geometry: ChunkGeometry, top: i32) -> Option<u32> {
    let value = i64::from(top) + 1 - i64::from(geometry.min_y());
    let value = u32::try_from(value).ok()?;
    (value <= geometry.height() as u32).then_some(value)
}

fn world_block_coordinate(chunk: i32, local: u8) -> i32 {
    let coordinate = i64::from(chunk) * 16 + i64::from(local);
    i32::try_from(coordinate).expect("chunk lies outside the supported i32 block-coordinate range")
}

impl TerrainGenerator {
    /// Build a generator from a seed plus a block registry.
    ///
    /// # Panics
    ///
    /// Panics if the registry is missing required vanilla terrain blocks. Use
    /// [`TerrainGenerator::try_with_biome_rules`] for fallible startup validation.
    #[must_use]
    pub fn new(seed: i64, registry: Arc<BlockRegistry>) -> Self {
        Self::try_with_biome_rules(seed, registry, BiomeRules::vanilla_overworld())
            .unwrap_or_else(|err| panic!("{err}"))
    }

    pub fn try_with_biome_rules(
        seed: i64,
        registry: Arc<BlockRegistry>,
        biomes: BiomeRules,
    ) -> Result<Self, TerrainGeneratorError> {
        let stone = try_resolve_block(registry.as_ref(), "minecraft:stone")?;
        let ores = OreRules::solaris_default(registry.as_ref(), &biomes, stone);
        Self::try_with_rules(seed, registry, biomes, ores)
    }

    /// Compile the local vanilla router and expanded overworld biome preset.
    /// Missing or invalid vanilla data is a startup error.
    pub fn load_vanilla_worldgen(&mut self, data_root: &std::path::Path) -> Result<(), String> {
        let climate = VanillaClimate::new(self.seed as u64, data_root)?;
        let density = VanillaDensity::from_runtime(climate.runtime())?;
        density.validate_aquifer_fields()?;
        let structures = crate::overworld_structures::OverworldStructures::load(
            data_root,
            self.seed,
            &self.blocks,
        )?;
        self.vanilla_worldgen = Some((climate, density));
        self.overworld_structures = Some(structures);
        self.diagnostic_cache = DiagnosticCache::default();
        Ok(())
    }

    fn vanilla(&self) -> Option<&(VanillaClimate, VanillaDensity)> {
        self.vanilla_worldgen.as_ref()
    }

    /// The unzoomed noise biome used by chunk palettes and structure gates.
    #[must_use]
    pub fn noise_biome(&self, world_x: i32, y: i32, world_z: i32) -> Identifier {
        if let Some((climate, _)) = self.vanilla() {
            return Identifier::parse(climate.sample(world_x, y, world_z).biome)
                .expect("validated vanilla biome identifier");
        }
        self.base_biome(world_x, world_z)
    }
    /// Build a generator with explicit biome and ore rules.
    ///
    /// # Panics
    ///
    /// Panics if the registry is missing required vanilla terrain blocks. Use
    /// [`TerrainGenerator::try_with_rules`] for fallible startup validation.
    #[must_use]
    pub fn with_rules(
        seed: i64,
        registry: Arc<BlockRegistry>,
        biomes: BiomeRules,
        ores: OreRules,
    ) -> Self {
        Self::try_with_rules(seed, registry, biomes, ores).unwrap_or_else(|err| panic!("{err}"))
    }

    /// Build a generator with explicit biome and ore rules, returning startup
    /// validation errors instead of panicking.
    ///
    /// Required terrain blocks fail construction when absent. Optional surface,
    /// fluid, and decoration blocks keep Solaris' existing fallback behavior so
    /// reduced test registries can still exercise generation.
    ///
    /// # Errors
    ///
    /// Returns [`TerrainGeneratorError::MissingRequiredBlock`] when the block
    /// registry is missing `minecraft:air`, `minecraft:bedrock`,
    /// `minecraft:stone`, `minecraft:dirt`, `minecraft:grass_block`, or
    /// `minecraft:iron_ore`.
    pub fn try_with_rules(
        seed: i64,
        registry: Arc<BlockRegistry>,
        biomes: BiomeRules,
        ores: OreRules,
    ) -> Result<Self, TerrainGeneratorError> {
        let air = try_resolve_block(registry.as_ref(), "minecraft:air")?;
        let stone = try_resolve_block(registry.as_ref(), "minecraft:stone")?;
        Ok(Self {
            seed,
            geometry: OVERWORLD_GEOMETRY,
            air,
            bedrock: try_resolve_block(registry.as_ref(), "minecraft:bedrock")?,
            stone,
            dirt: try_resolve_block(registry.as_ref(), "minecraft:dirt")?,
            grass_block: try_resolve_block(registry.as_ref(), "minecraft:grass_block")?,
            sand: resolve_block_or(registry.as_ref(), "minecraft:sand", stone),
            red_sand: resolve_block_or(registry.as_ref(), "minecraft:red_sand", stone),
            gravel: resolve_block_or(registry.as_ref(), "minecraft:gravel", stone),
            podzol: resolve_block_or(registry.as_ref(), "minecraft:podzol", stone),
            snow_block: resolve_block_or(registry.as_ref(), "minecraft:snow_block", stone),
            deepslate: resolve_block_or(registry.as_ref(), "minecraft:deepslate", stone),
            water: resolve_block_or(registry.as_ref(), "minecraft:water", air),
            cave_lava: resolve_block_or(registry.as_ref(), "minecraft:lava", air),
            terracotta_layers: [
                "minecraft:terracotta",
                "minecraft:orange_terracotta",
                "minecraft:terracotta",
                "minecraft:yellow_terracotta",
                "minecraft:brown_terracotta",
                "minecraft:terracotta",
                "minecraft:red_terracotta",
                "minecraft:white_terracotta",
            ]
            .map(|name| resolve_block_or(registry.as_ref(), name, stone)),
            biomes,
            ores,
            geological_ores: None,
            ore_generation_profile: "vanilla",
            village_plans: None,
            overworld_structures: None,
            jigsaw_sources: Vec::new(),
            decorations: DecorationBlocks::new(registry.as_ref()),
            tree_rules: HashMap::new(),
            clay_rule: ClayRule::default(),
            vanilla_worldgen: None,
            diagnostic_cache: DiagnosticCache::default(),
            blocks: registry,
            #[cfg(feature = "gpu")]
            gpu: None,
            solid_cache: solid_cache::SolidCache::default(),
        })
    }

    /// Wire the village plan source: vanilla villages assembled by the jigsaw
    /// solver, placed through [`crate::village::piece`], with their
    /// `terrain_adaptation = beard_thin` applied as [`crate::village::beard`]'s
    /// column-height analogue.
    ///
    /// Until this is called the generator places no villages, and every path
    /// stays byte-identical to a build without village generation.
    #[must_use]
    pub fn with_village_plans(mut self, source: Arc<VillagePlanSource>) -> Self {
        self.village_plans = Some(source);
        self
    }

    #[must_use]
    pub fn with_jigsaw_source(mut self, source: Arc<VillagePlanSource>) -> Self {
        self.jigsaw_sources.push(source);
        self
    }

    /// The village plan source, when the generator was given one.
    #[must_use]
    pub fn village_plan_source(&self) -> Option<&Arc<VillagePlanSource>> {
        self.village_plans.as_ref()
    }

    /// Explicit standalone vanilla plugin deposit profile. Native Earth never
    /// calls this path: its ores belong to the immutable geological base.
    pub fn try_with_realistic_deposits(
        mut self,
        registry: &BlockRegistry,
    ) -> Result<Self, TerrainGeneratorError> {
        self.ores = OreRules::new(Vec::new()).expect("empty ore rules are valid");
        self.geological_ores = Some(GeologicalOreRules::try_new(registry)?);
        self.ore_generation_profile = "realistic_deposits";
        Ok(self)
    }

    #[must_use]
    pub fn with_realistic_deposits(self, registry: &BlockRegistry) -> Self {
        self.try_with_realistic_deposits(registry)
            .unwrap_or_else(|error| panic!("{error}"))
    }

    #[must_use]
    pub const fn ore_generation_profile(&self) -> &'static str {
        self.ore_generation_profile
    }

    #[must_use]
    pub fn with_geometry(mut self, geometry: ChunkGeometry) -> Self {
        self.geometry = geometry;
        self.diagnostic_cache = DiagnosticCache::default();
        self
    }

    /// Sample the terrain height for an absolute world `(x, z)`.
    /// Public so tests + spawn-position picking can use the same
    /// function the generator does.
    ///
    /// A configured village plan source applies the column-height analogue
    /// here, on the same on-demand plan lookup
    /// ([`VillagePlanSource::plans_for_column`]) the chunk fill reads its own
    /// chunk's plans from, so this query and the generated terrain agree.
    #[must_use]
    pub fn surface_height(&self, world_x: i32, world_z: i32) -> i32 {
        let base = self.base_surface_y(world_x, world_z);
        match self.village_plans_for_column(world_x, world_z) {
            Some(plans) => self.village_surface_y(world_x, world_z, base, Some(&plans)),
            None => base,
        }
    }

    /// The router's surface for one column, before the village analogue.
    fn base_surface_y(&self, world_x: i32, world_z: i32) -> i32 {
        if let Some((_, density)) = self.vanilla() {
            return density
                .surface_height(world_x, world_z)
                .clamp(self.geometry.min_y(), self.geometry.max_y() - 1);
        }
        self.density_router().sample(world_x, world_z).surface_y
    }

    /// The plan-free WORLD_SURFACE_WG top block includes water; the density
    /// surface alone is the ocean/river floor. Match the fill lane's water top.
    fn world_surface_y(&self, solid_y: i32) -> i32 {
        let water_top = if self.vanilla().is_some() {
            SEA_LEVEL - 1
        } else {
            SEA_LEVEL
        };
        solid_y.max(water_top).min(self.geometry.max_y() - 1)
    }

    /// WORLD_SURFACE_WG's first free height, used by jigsaw assembly.
    fn first_free_height(&self, world_x: i32, world_z: i32) -> i32 {
        self.world_surface_y(self.base_surface_y(world_x, world_z))
            .saturating_add(1)
    }

    /// Every vanilla village whose start chunk lies in the inclusive chunk
    /// rectangle `min_chunk..=max_chunk`, as the plans this generator generates
    /// them from.
    ///
    /// The two column queries the plan enumeration reads are this generator's
    /// own plan-free ones ([`TerrainGenerator::first_free_height`] and
    /// [`TerrainGenerator::base_biome`]), so a listed village is the village a
    /// later chunk fill will actually place, and listing one generates nothing.
    /// A generator with no village plans lists nothing.
    #[must_use]
    pub fn village_sites_in_region(
        &self,
        min_chunk: (i32, i32),
        max_chunk: (i32, i32),
    ) -> Vec<mc_world::GeneratedVillageSite> {
        let Some(source) = self.village_plans.as_ref() else {
            return Vec::new();
        };
        let free_height = |x, z| self.first_free_height(x, z);
        let biome_at = |x, y, z| self.noise_biome(x, y, z);
        source.sites_in_region(min_chunk, max_chunk, &free_height, &biome_at)
    }

    /// The plans the chunk being filled reads its pieces and beard columns
    /// from: a single on-demand lookup per generated chunk.
    fn village_plans_for_chunk(&self, pos: ChunkPos) -> Option<VillagePlanSet> {
        let source = self.village_plans.as_ref()?;
        let free_height = |x, z| self.first_free_height(x, z);
        let biome_at = |x, y, z| self.noise_biome(x, y, z);
        source.plans_for_chunk(pos.x, pos.z, &free_height, &biome_at)
    }

    /// The plans covering one column, for callers that hold a column rather than
    /// the chunk being filled. The chunk's own set filtered to the column, so a
    /// column and its chunk cannot disagree.
    fn village_plans_for_column(&self, world_x: i32, world_z: i32) -> Option<VillagePlanSet> {
        let source = self.village_plans.as_ref()?;
        let free_height = |x, z| self.first_free_height(x, z);
        let biome_at = |x, y, z| self.noise_biome(x, y, z);
        source.plans_for_column(world_x, world_z, &free_height, &biome_at)
    }

    /// The terrain's own biome at a column, before any village is applied.
    ///
    /// This is the biome the structure gate reads, and it deliberately does not
    /// go through [`TerrainGenerator::surface_height`] or
    /// [`TerrainGenerator::diagnostic_sample`]: both consult the plan lookup,
    /// which asks this question, so a plan-aware path here would recurse.
    #[must_use]
    pub fn base_biome(&self, world_x: i32, world_z: i32) -> Identifier {
        self.cached_diagnostic_sample(world_x, world_z).biome
    }

    /// The surface a column is planned at: the terrain's own surface, moved by
    /// the column-height analogue of every plan that reaches the column.
    fn village_surface_y(
        &self,
        world_x: i32,
        world_z: i32,
        base: i32,
        plans: Option<&VillagePlanSet>,
    ) -> i32 {
        match plans {
            Some(plans) => plans.adjusted_surface_y(
                world_x,
                world_z,
                base,
                self.geometry.min_y(),
                self.geometry.max_y(),
            ),
            None => base,
        }
    }

    /// Sample the same terrain, biome, and vegetation decisions used to plan a
    /// generated column, without constructing or mutating a chunk.
    ///
    /// A configured village plan source moves `surface_y` exactly where it moves
    /// a generated column's surface: [`TerrainGenerator::surface_height`],
    /// [`TerrainGenerator::plan_column`] and this sample all read one plan per
    /// chunk ([`VillagePlanSource::plans_for_column`] here, the chunk's own plans
    /// inside generation), so they cannot disagree.
    #[must_use]
    pub fn diagnostic_sample(&self, world_x: i32, world_z: i32) -> TerrainDiagnosticSample {
        let sample = self.cached_diagnostic_sample(world_x, world_z);
        let plans = self.village_plans_for_column(world_x, world_z);
        TerrainDiagnosticSample {
            surface_y: self.village_surface_y(world_x, world_z, sample.surface_y, plans.as_ref()),
            ..sample
        }
    }

    /// The terrain's own decisions for a column, memoized, before the village
    /// analogue. The fill path reads this and applies its chunk's plan to the
    /// result, so the analogue is applied once per consumer, not twice.
    fn cached_diagnostic_sample(&self, world_x: i32, world_z: i32) -> TerrainDiagnosticSample {
        if let Some(sample) = self.diagnostic_cache.get(world_x, world_z) {
            return sample;
        }
        let sample = self.diagnostic_sample_uncached(world_x, world_z, None);
        self.diagnostic_cache.insert(world_x, world_z, sample)
    }

    fn diagnostic_sample_uncached(
        &self,
        world_x: i32,
        world_z: i32,
        native_height: Option<i32>,
    ) -> TerrainDiagnosticSample {
        // The vanilla branch never reads the own-router sample: `surface_y` is
        // overwritten from the native density, `moisture` from the native
        // climate, and `biome` from `noise_biome`. The router call below is
        // therefore dead work in exactly this branch — and skipping it is
        // side-effect-free by construction: `OverworldRouter::sample` and
        // `fbm_2d` are pure functions of `(seed, x, z)` (no statics, cells,
        // locks or atomics anywhere in `overworld/*` or `noise.rs`), so no
        // memo, counter or global observes the call.
        if let Some((climate, density)) = self.vanilla() {
            let surface_y = native_height
                .unwrap_or_else(|| density.surface_height(world_x, world_z))
                .clamp(self.geometry.min_y(), self.geometry.max_y() - 1);
            let native = climate.sample(world_x, surface_y, world_z);
            let biome =
                Identifier::parse(native.biome).expect("validated vanilla biome identifier");
            let vegetation_density = self
                .biome_supports_land_vegetation(&biome)
                .then(|| self.vegetation_density(world_x, world_z, f64::from(native.climate[1])));
            return TerrainDiagnosticSample {
                surface_y,
                biome,
                vegetation_density,
            };
        }
        let sample = self.density_router().sample(world_x, world_z);
        let biome = self.vanilla_biome_for(world_x, world_z, sample.surface_y, sample);
        let surface_y = sample.surface_y;
        let vegetation_density = self
            .biome_supports_land_vegetation(&biome)
            .then(|| self.vegetation_density(world_x, world_z, sample.moisture));
        TerrainDiagnosticSample {
            surface_y,
            biome,
            vegetation_density,
        }
    }

    /// Find a deterministic natural spawn centre without modifying terrain.
    ///
    /// The search walks expanding 64-block rings around the origin and accepts
    /// only dry, low-relief inland terrain away from river centres and strong
    /// mountain ridges. Each routing-level candidate is then checked against the
    /// chunk this generator actually produces for it (features, decoration and
    /// village pieces included): a dry, non-fluid support block at `surface_y`
    /// and air in the two body blocks above. A candidate that fails is skipped
    /// for the next one within the same bounded search.
    #[must_use]
    pub fn locate_safe_spawn(&self) -> Option<SpawnLocation> {
        self.spawn_candidates()
            .find(|spawn| self.spawn_is_clear(*spawn))
    }

    fn spawn_is_clear(&self, spawn: SpawnLocation) -> bool {
        let chunk = ChunkGenerator::generate(self, spawn.chunk());
        let lx = spawn.block_x.rem_euclid(16) as u8;
        let lz = spawn.block_z.rem_euclid(16) as u8;
        chunk.get_block(lx, spawn.surface_y, lz).is_some_and(|id| {
            id != self.air
                && self
                    .blocks
                    .by_id(id)
                    .is_some_and(|state| !state.has_fluid())
        }) && chunk.get_block(lx, spawn.surface_y + 1, lz) == Some(self.air)
            && chunk.get_block(lx, spawn.surface_y + 2, lz) == Some(self.air)
    }

    /// Routing-level spawn candidates in search order: the origin, then each
    /// expanding 64-block ring.
    fn spawn_candidates(&self) -> impl Iterator<Item = SpawnLocation> + '_ {
        let max_ring = SPAWN_SEARCH_MAX_RADIUS_BLOCKS / SPAWN_SEARCH_STEP_BLOCKS;
        let rings = (1..=max_ring).flat_map(|ring| {
            let radius = ring * SPAWN_SEARCH_STEP_BLOCKS;
            let rows = (-ring..=ring).flat_map(move |offset| {
                let coordinate = offset * SPAWN_SEARCH_STEP_BLOCKS;
                [(coordinate, -radius), (coordinate, radius)]
            });
            let columns = ((-ring + 1)..ring).flat_map(move |offset| {
                let coordinate = offset * SPAWN_SEARCH_STEP_BLOCKS;
                [(-radius, coordinate), (radius, coordinate)]
            });
            rows.chain(columns)
        });
        std::iter::once((0, 0))
            .chain(rings)
            .filter_map(|(x, z)| self.spawn_candidate(x, z))
    }

    fn spawn_candidate(&self, block_x: i32, block_z: i32) -> Option<SpawnLocation> {
        let centre = self.routing_sample(block_x, block_z);
        let sea_level = SEA_LEVEL;
        if centre.surface_y < sea_level + 4
            || centre.continentalness < 0.02
            || centre.river < 0.08
            || centre.ridges > 0.24
        {
            return None;
        }

        let mut minimum = centre.surface_y;
        let mut maximum = centre.surface_y;
        for dz in [
            -SPAWN_SITE_SAMPLE_RADIUS_BLOCKS,
            0,
            SPAWN_SITE_SAMPLE_RADIUS_BLOCKS,
        ] {
            for dx in [
                -SPAWN_SITE_SAMPLE_RADIUS_BLOCKS,
                0,
                SPAWN_SITE_SAMPLE_RADIUS_BLOCKS,
            ] {
                let sample =
                    self.routing_sample(block_x.checked_add(dx)?, block_z.checked_add(dz)?);
                if sample.surface_y < sea_level + 3 || sample.river < 0.04 {
                    return None;
                }
                minimum = minimum.min(sample.surface_y);
                maximum = maximum.max(sample.surface_y);
            }
        }
        if maximum - minimum > SPAWN_SITE_MAX_RELIEF {
            return None;
        }
        Some(SpawnLocation {
            block_x,
            surface_y: centre.surface_y,
            block_z,
        })
    }

    fn density_router(&self) -> OverworldRouter {
        OverworldRouter::new(self.seed, self.geometry)
    }

    fn routing_sample(&self, x: i32, z: i32) -> TerrainSample {
        if let Some((climate, density)) = self.vanilla() {
            let surface_y = density
                .surface_height(x, z)
                .clamp(self.geometry.min_y(), self.geometry.max_y() - 1);
            let sample = climate.sample(x, surface_y, z).climate;
            return TerrainSample {
                surface_y,
                temperature: f64::from(sample[0]),
                moisture: f64::from(sample[1]),
                continentalness: f64::from(sample[2]),
                erosion: f64::from(sample[3]),
                ridges: f64::from(sample[5].abs()),
                river: f64::from(sample[5].abs()),
                wetland: 0.0,
                climate_domain: 0.0,
            };
        }
        self.density_router().sample(x, z)
    }

    #[cfg(test)]
    fn biome_for_cell(
        &self,
        world_x: i32,
        y: i32,
        world_z: i32,
        surface_height: i32,
    ) -> Identifier {
        if self.vanilla().is_some() {
            return self.noise_biome(world_x, y, world_z);
        }
        if i64::from(y) < i64::from(surface_height) - 24 && y < 32 {
            return self
                .biomes
                .pick(&self.biomes.cave, world_x, world_z, self.seed, 0x554E_4447);
        }
        self.biome_for(world_x, world_z, surface_height)
    }

    fn vegetation_density(&self, world_x: i32, world_z: i32, moisture: f64) -> f64 {
        let regional = fbm_2d(
            f64::from(world_x) / VEGETATION_REGION_SCALE,
            f64::from(world_z) / VEGETATION_REGION_SCALE,
            self.seed ^ 0x5645_4745,
            2,
            0.5,
        );
        (regional * 0.72 + moisture * 0.28).clamp(-1.0, 1.0)
    }

    /// Gravel bars along a river bed: a coarse two-octave field so bars read as
    /// patches rather than per-block speckle.
    fn river_gravel_bar(&self, world_x: i32, world_z: i32) -> bool {
        fbm_2d(
            f64::from(world_x) / RIVER_GRAVEL_SCALE,
            f64::from(world_z) / RIVER_GRAVEL_SCALE,
            self.seed ^ RIVER_GRAVEL_SALT,
            2,
            0.5,
        ) > RIVER_GRAVEL_THRESHOLD
    }

    fn biome_supports_land_vegetation(&self, biome: &Identifier) -> bool {
        self.biomes.grassland.contains(biome)
            || self.biomes.temperate_forest.contains(biome)
            || self.biomes.jungle.contains(biome)
            || self.biomes.swamp.contains(biome)
            || Self::is_cold_forest(biome)
            || Self::is_savanna(biome)
    }

    fn plan_column(
        &self,
        pos: ChunkPos,
        lx: u8,
        lz: u8,
        plans: Option<&VillagePlanSet>,
    ) -> ColumnPlan {
        let wx = world_block_coordinate(pos.x, lx);
        let wz = world_block_coordinate(pos.z, lz);
        let diagnostic = self.cached_diagnostic_sample(wx, wz);
        self.plan_column_from_diagnostic(pos, lx, lz, plans, diagnostic)
    }

    fn plan_column_from_diagnostic(
        &self,
        pos: ChunkPos,
        lx: u8,
        lz: u8,
        plans: Option<&VillagePlanSet>,
        diagnostic: TerrainDiagnosticSample,
    ) -> ColumnPlan {
        let wx = world_block_coordinate(pos.x, lx);
        let wz = world_block_coordinate(pos.z, lz);
        let height = self.village_surface_y(wx, wz, diagnostic.surface_y, plans);
        let biome = diagnostic.biome;
        let vegetation_density = diagnostic.vegetation_density.unwrap_or(-1.0);
        let (sea_level, water_enabled) = (SEA_LEVEL, true);
        let (mut surface, mut fill) = self.surface_materials(&biome);
        if self.biomes.mountain.contains(&biome) && height >= sea_level + 112 {
            surface = self.snow_block;
            fill = self.stone;
        }
        // Vanilla river beds are sand with gravel bars; without them the bed
        // reads as one flat sand sheet.
        if self.biomes.is_river(&biome) && self.river_gravel_bar(wx, wz) {
            surface = self.gravel;
            fill = self.gravel;
        }
        let top_non_air = if self.vanilla().is_some() {
            height.max(sea_level - 1).min(self.geometry.max_y() - 1)
        } else if water_enabled && (height < sea_level || self.biomes.is_river(&biome)) {
            let inclusive_top = checked_y_offset(self.geometry.max_y(), -1).unwrap_or(height);
            sea_level.clamp(height, inclusive_top)
        } else {
            height
        };
        let minimum_fill_y =
            checked_y_offset(self.geometry.min_y(), 1).unwrap_or(self.geometry.min_y());
        ColumnPlan {
            lx,
            lz,
            wx,
            wz,
            height,
            top_non_air,
            dirt_start: checked_y_offset(height, -DIRT_DEPTH)
                .unwrap_or(minimum_fill_y)
                .max(minimum_fill_y),
            biome,
            surface,
            fill,
            vegetation_density,
            hash: feature_hash(self.seed, wx, height, wz, 0xDEC0_0001),
        }
    }

    fn fill_column(&self, chunk: &mut Chunk, plan: &ColumnPlan) {
        let min_y = self.geometry.min_y();
        let _ = chunk.set_block(plan.lx, min_y, plan.lz, self.bedrock);
        let Some(first_fill_y) = checked_y_offset(min_y, 1) else {
            return;
        };
        for y in first_fill_y..plan.dirt_start {
            let _ = chunk.set_block(
                plan.lx,
                y,
                plan.lz,
                self.base_stone_for_y(plan.lx, y, plan.lz, chunk.pos),
            );
        }
        for y in plan.dirt_start..plan.height {
            let _ = chunk.set_block(plan.lx, y, plan.lz, plan.fill);
        }
        let _ = chunk.set_block(plan.lx, plan.height, plan.lz, plan.surface);
        if plan.top_non_air > plan.height {
            let Some(first_water_y) = checked_y_offset(plan.height, 1) else {
                return;
            };
            for y in first_water_y..=plan.top_non_air {
                let _ = chunk.set_block(plan.lx, y, plan.lz, self.water);
            }
        }
    }

    fn surface_materials(&self, biome: &Identifier) -> (BlockStateId, BlockStateId) {
        let path = biome.path();
        if path == "stony_shore" {
            return (self.gravel, self.stone);
        }
        if self.biomes.is_surface_water(biome) || self.biomes.is_beach_or_shore(biome) {
            return (self.sand, self.sand);
        }
        if path.contains("badlands") {
            return (self.red_sand, self.terracotta_layers[0]);
        }
        if path == "desert" {
            return (self.sand, self.sand);
        }
        if path == "mangrove_swamp"
            && let Some(mud) = self.decorations.mud
        {
            return (mud, self.dirt);
        }
        if self.biomes.mountain.contains(biome) || path.contains("stony") {
            return (self.gravel, self.stone);
        }
        if self.biomes.cold.contains(biome) || path.contains("snow") || path.contains("frozen") {
            return (self.snow_block, self.dirt);
        }
        if self.biomes.jungle.contains(biome) {
            return (self.grass_block, self.dirt);
        }
        if path.contains("taiga") {
            return (self.podzol, self.dirt);
        }
        (self.grass_block, self.dirt)
    }

    fn assign_biomes(&self, chunk: &mut Chunk, columns: &[ColumnPlan; 256]) {
        let quart_ys = chunk.biomes.len() * BIOME_DIM;
        let quart_y =
            |q: usize| i32::try_from(i64::from(self.geometry.min_y()) + q as i64 * 4 + 2).ok();
        // Each quart column's whole height in one pass, so the column
        // sampler computes its Y-free climate fields once per chunk instead
        // of once per section. Indexed `(cz * 4 + cx) * quart_ys + q`.
        let sampled = self.vanilla().map(|(climate, _)| {
            let mut sampler = climate.column_sampler();
            let mut sampled = Vec::with_capacity(BIOME_DIM * BIOME_DIM * quart_ys);
            for column in 0..BIOME_DIM * BIOME_DIM {
                let (cz, cx) = (column / BIOME_DIM, column % BIOME_DIM);
                let plan = &columns[(cz * 4 + 2) * 16 + cx * 4 + 2];
                for q in 0..quart_ys {
                    sampled.push(quart_y(q).map(|y| sampler.sample(plan.wx, y, plan.wz).biome));
                }
            }
            sampled
        });
        for (section_idx, section) in chunk.biomes.iter_mut().enumerate() {
            let mut palette: Vec<Identifier> = Vec::with_capacity(4);
            // Raw biome names already resolved to a palette slot.
            let mut named: Vec<(&str, usize)> = Vec::with_capacity(4);
            let mut indices = PackedBitArray::zeroed(6, BIOME_VOLUME);
            for cy in 0..BIOME_DIM {
                let q = section_idx * BIOME_DIM + cy;
                let Some(y) = quart_y(q) else {
                    continue;
                };
                for cz in 0..BIOME_DIM {
                    for cx in 0..BIOME_DIM {
                        let lx = cx * 4 + 2;
                        let lz = cz * 4 + 2;
                        let column = &columns[lz * 16 + lx];
                        let palette_idx = if let Some(sampled) = &sampled {
                            let name = sampled[(cz * BIOME_DIM + cx) * quart_ys + q]
                                .expect("same checked biome Y");
                            match named.iter().find(|(seen, _)| *seen == name) {
                                Some(&(_, slot)) => slot,
                                None => {
                                    let biome = Identifier::parse(name)
                                        .expect("validated vanilla biome identifier");
                                    let slot = palette_slot(&mut palette, biome);
                                    named.push((name, slot));
                                    slot
                                }
                            }
                        } else if i64::from(y) < i64::from(column.height) - 24 && y < 32 {
                            palette_slot(
                                &mut palette,
                                self.biomes.pick(
                                    &self.biomes.cave,
                                    column.wx,
                                    column.wz,
                                    self.seed,
                                    0x554E_4447,
                                ),
                            )
                        } else {
                            palette_slot(&mut palette, column.biome.clone())
                        };
                        let idx = (cy * BIOME_DIM + cz) * BIOME_DIM + cx;
                        indices.set(idx, palette_idx as u32);
                    }
                }
            }
            if palette.len() == 1 {
                *section = BiomeSection::filled(palette.pop().unwrap());
            } else {
                *section = BiomeSection::from_indirect(palette, indices);
            }
        }
    }

    fn apply_caves(&self, chunk: &mut Chunk, columns: &[ColumnPlan; 256]) {
        self.apply_caves_with_cache(chunk, columns, None);
    }

    fn apply_caves_with_cache(
        &self,
        chunk: &mut Chunk,
        columns: &[ColumnPlan; 256],
        mut ore_cache: Option<&mut OreColumnCache>,
    ) {
        if self.vanilla().is_some() {
            return;
        }
        const RAW_SIDE: usize = 18;
        let cave_min_y = self.geometry.min_y().saturating_add(8);
        let cave_max_y = columns
            .iter()
            .filter_map(|plan| self.cave_y_bounds(plan).map(|(_, max_y)| max_y))
            .max()
            .unwrap_or(cave_min_y.saturating_sub(1))
            .min(31);
        if cave_max_y < cave_min_y {
            return;
        }

        let chunk_min_x = i64::from(chunk.pos.x) * 16;
        let chunk_min_z = i64::from(chunk.pos.z) * 16;
        let router = self.density_router();
        let raw_min_x = chunk_min_x - 1;
        let raw_min_z = chunk_min_z - 1;
        let raw_max_x = chunk_min_x + 16;
        let raw_max_z = chunk_min_z + 16;
        let raw_grid_bounds = i32::try_from(raw_min_x)
            .ok()
            .zip(i32::try_from(raw_min_z).ok())
            .zip(i32::try_from(raw_max_x).ok())
            .zip(i32::try_from(raw_max_z).ok());
        let mut raw = [0_u8; RAW_SIDE * RAW_SIDE];
        for sample_y in (cave_min_y..=cave_max_y).step_by(CAVE_VERTICAL_SAMPLE_STEP as usize) {
            if let Some((((raw_min_x, raw_min_z), _), _)) = raw_grid_bounds {
                router.fill_raw_cave_layer(raw_min_x, raw_min_z, RAW_SIDE, sample_y, &mut raw);
            } else {
                raw.fill(1);
                for raw_z in 0..RAW_SIDE {
                    for raw_x in 0..RAW_SIDE {
                        let world_x = chunk_min_x + raw_x as i64 - 1;
                        let world_z = chunk_min_z + raw_z as i64 - 1;
                        let cave = i32::try_from(world_x)
                            .ok()
                            .zip(i32::try_from(world_z).ok())
                            .is_some_and(|(world_x, world_z)| {
                                router.raw_cave(world_x, sample_y, world_z)
                            });
                        raw[raw_z * RAW_SIDE + raw_x] = if cave { 2 } else { 1 };
                    }
                }
            }

            for plan in columns {
                let Some((plan_min_y, plan_max_y)) = self.cave_y_bounds(plan) else {
                    continue;
                };
                let cave_limit = plan.height.saturating_sub(CAVE_SURFACE_CLEARANCE).min(32);
                if sample_y < plan_min_y || sample_y > plan_max_y || sample_y >= cave_limit {
                    continue;
                }
                let raw_x = usize::from(plan.lx) + 1;
                let raw_z = usize::from(plan.lz) + 1;
                let center = raw_z * RAW_SIDE + raw_x;
                if raw[center] != 2 {
                    continue;
                }
                let connected = raw[raw_z * RAW_SIDE + raw_x - 1] == 2
                    || raw[raw_z * RAW_SIDE + raw_x + 1] == 2
                    || raw[(raw_z - 1) * RAW_SIDE + raw_x] == 2
                    || raw[(raw_z + 1) * RAW_SIDE + raw_x] == 2;
                if !connected {
                    continue;
                }
                for y in sample_y..sample_y.saturating_add(CAVE_VERTICAL_SAMPLE_STEP) {
                    if y > plan_max_y || y >= self.geometry.max_y() {
                        break;
                    }
                    let _ = chunk.set_block(plan.lx, y, plan.lz, self.air);
                }
            }
            if let Some(cache) = ore_cache.as_deref_mut() {
                for raw_z in 0..RAW_SIDE {
                    for raw_x in 0..RAW_SIDE {
                        let state = raw[raw_z * RAW_SIDE + raw_x];
                        if state == 0 {
                            continue;
                        }
                        let world_x = chunk_min_x + raw_x as i64 - 1;
                        let world_z = chunk_min_z + raw_z as i64 - 1;
                        if let (Ok(world_x), Ok(world_z)) =
                            (i32::try_from(world_x), i32::try_from(world_z))
                        {
                            cache.record_raw_cave(world_x, sample_y, world_z, state == 2);
                        }
                    }
                }
            }
        }
    }

    fn cave_y_bounds(&self, plan: &ColumnPlan) -> Option<(i32, i32)> {
        let cave_min_y = i64::from(self.geometry.min_y()) + 8;
        let cave_max_y = (i64::from(plan.height) - i64::from(CAVE_SURFACE_CLEARANCE))
            .min(i64::from(plan.dirt_start) - 1);
        if cave_max_y < cave_min_y {
            return None;
        }
        Some((
            i32::try_from(cave_min_y).ok()?,
            i32::try_from(cave_max_y).ok()?,
        ))
    }

    fn ore_column_cache<'a, 'h>(
        &self,
        chunk: &Chunk,
        chunk_columns: &[ColumnPlan; 256],
        heights: &'a mut HeightContext<'h>,
    ) -> OreColumnCache<'a, 'h> {
        let min_x_i64 = i64::from(chunk.pos.x) * 16 - i64::from(ORE_COLUMN_HALO);
        let min_z_i64 = i64::from(chunk.pos.z) * 16 - i64::from(ORE_COLUMN_HALO);
        let side = usize::try_from(16 + ORE_COLUMN_HALO * 2)
            .expect("bounded ore column cache side fits usize");
        let min_x = min_x_i64;
        let min_z = min_z_i64;
        let cave_min_y = self.geometry.min_y().saturating_add(8);
        let cave_max_y = self.geometry.max_y().saturating_sub(1).min(31);
        let cave_layers = if cave_max_y < cave_min_y {
            0
        } else {
            usize::try_from((cave_max_y - cave_min_y) / CAVE_VERTICAL_SAMPLE_STEP + 1)
                .expect("bounded cave layer count fits usize")
        };
        let mut cache = OreColumnCache {
            min_x,
            min_z,
            side,
            columns: vec![None; side * side],
            heights,
            cave_min_y,
            cave_layers,
            cave_samples: vec![0; side * side * cave_layers],
            raw_cave_samples: vec![0; side * side * cave_layers],
            dense_caves: std::collections::HashMap::new(),
        };
        let center_offset = usize::try_from(ORE_COLUMN_HALO).expect("ore halo fits usize");
        for plan in chunk_columns {
            let local_x = center_offset + usize::from(plan.lx);
            let local_z = center_offset + usize::from(plan.lz);
            cache.columns[local_z * side + local_x] = Some(plan.clone());
        }
        cache
    }

    fn apply_ores_with_cache(&self, chunk: &mut Chunk, column_cache: &mut OreColumnCache<'_, '_>) {
        debug_assert!(self.geological_ores.is_none());
        let chunk_min_x = i64::from(chunk.pos.x) * 16;
        let chunk_min_z = i64::from(chunk.pos.z) * 16;
        // Re-evaluate nearby anchors so a vein crossing a chunk edge is derived
        // identically no matter which side is generated first.
        let radius = i64::from(ORE_VEIN_RADIUS);
        let cell_edge = i64::from(ORE_ANCHOR_CELL_EDGE);
        let min_cell_x = (chunk_min_x - radius).div_euclid(cell_edge);
        let max_cell_x = (chunk_min_x + 15 + radius).div_euclid(cell_edge);
        let min_cell_z = (chunk_min_z - radius).div_euclid(cell_edge);
        let max_cell_z = (chunk_min_z + 15 + radius).div_euclid(cell_edge);

        for (rule_index, rule) in self.ores.rules().iter().enumerate() {
            let Some(first_stone_y) = checked_y_offset(self.geometry.min_y(), 1) else {
                continue;
            };
            let Some(inclusive_top) = checked_y_offset(self.geometry.max_y(), -1) else {
                continue;
            };
            let min_y = rule.y.min.max(first_stone_y);
            let max_y = rule.y.max.min(inclusive_top);
            if min_y > max_y {
                continue;
            }
            let vein_size = usize::try_from(rule.size)
                .unwrap_or(MAX_ORE_VEIN_SIZE)
                .clamp(1, MAX_ORE_VEIN_SIZE);
            let min_cell_y = min_y.div_euclid(ORE_ANCHOR_CELL_EDGE);
            let max_cell_y = max_y.div_euclid(ORE_ANCHOR_CELL_EDGE);
            let rule_salt = 0x0AE0_0000_u64 ^ rule_index as u64;

            for cell_y in min_cell_y..=max_cell_y {
                let sample_y = i64::from(cell_y) * i64::from(ORE_ANCHOR_CELL_EDGE)
                    + i64::from(ORE_ANCHOR_CELL_EDGE / 2);
                let Ok(sample_y) = i32::try_from(sample_y) else {
                    continue;
                };
                let spacing = rule.spacing.at_y(sample_y, rule.y).max(1);
                let denominator = spacing.saturating_mul(vein_size as u64).max(1);
                for cell_z in min_cell_z..=max_cell_z {
                    for cell_x in min_cell_x..=max_cell_x {
                        let cell_x_i32 =
                            i32::try_from(cell_x).expect("valid chunk halo cell x fits i32");
                        let cell_z_i32 =
                            i32::try_from(cell_z).expect("valid chunk halo cell z fits i32");
                        let cell_hash =
                            feature_hash(self.seed, cell_x_i32, cell_y, cell_z_i32, rule_salt);
                        let guaranteed = ORE_ANCHOR_CELL_VOLUME / denominator;
                        let remainder = ORE_ANCHOR_CELL_VOLUME % denominator;
                        let extra = usize::from(
                            remainder != 0 && cell_hash.rotate_left(29) % denominator < remainder,
                        );
                        let anchor_count = usize::try_from(guaranteed)
                            .unwrap_or(MAX_ORE_ANCHORS_PER_CELL)
                            .saturating_add(extra)
                            .min(MAX_ORE_ANCHORS_PER_CELL);
                        if anchor_count == 0 {
                            continue;
                        }

                        let start = (cell_hash & 63) as usize;
                        let step = (((cell_hash >> 6) & 31) as usize) * 2 + 1;
                        for slot in 0..anchor_count {
                            let cell_index = (start + slot * step) & 63;
                            let anchor_x = cell_x * cell_edge + (cell_index & 3) as i64;
                            let anchor_z = cell_z * cell_edge + ((cell_index >> 2) & 3) as i64;
                            let anchor_y =
                                i64::from(cell_y) * cell_edge + ((cell_index >> 4) & 3) as i64;
                            let (Ok(anchor_x), Ok(anchor_y), Ok(anchor_z)) = (
                                i32::try_from(anchor_x),
                                i32::try_from(anchor_y),
                                i32::try_from(anchor_z),
                            ) else {
                                continue;
                            };
                            if !(min_y..=max_y).contains(&anchor_y) {
                                continue;
                            }
                            if !rule.biomes.is_any() {
                                let matches = if let Some((climate, _)) = self.vanilla() {
                                    rule.biomes.matches_name(
                                        climate.sample(anchor_x, anchor_y, anchor_z).biome,
                                    )
                                } else {
                                    column_cache
                                        .get_or_plan(anchor_x, anchor_z)
                                        .is_some_and(|column| rule.biomes.matches(&column.biome))
                                };
                                if !matches {
                                    continue;
                                }
                            }
                            let vein_hash = feature_hash(
                                self.seed,
                                anchor_x,
                                anchor_y,
                                anchor_z,
                                rule_salt ^ slot as u64,
                            );
                            self.place_ore_vein(
                                chunk,
                                column_cache,
                                rule,
                                [anchor_x, anchor_y, anchor_z],
                                vein_hash,
                                vein_size,
                            );
                        }
                    }
                }
            }
        }
    }

    fn place_ore_vein(
        &self,
        chunk: &mut Chunk,
        column_cache: &mut OreColumnCache<'_, '_>,
        rule: &OreRule,
        anchor: [i32; 3],
        vein_hash: u64,
        vein_size: usize,
    ) {
        let mut offsets = [[0_i8; 3]; MAX_ORE_VEIN_SIZE];
        let offset_count = ore_veins::ore_offsets(&mut offsets, vein_hash, vein_size);
        let min_x = i64::from(chunk.pos.x) * 16;
        let min_z = i64::from(chunk.pos.z) * 16;
        for (index, &offset) in offsets[..offset_count].iter().enumerate() {
            // Off-chunk cells can never be written here. Reject them before
            // reconstructing their density and exposure halo; the remaining
            // in-chunk cells still query cross-boundary neighbours normally.
            let x = i64::from(anchor[0]) + i64::from(offset[0]);
            let z = i64::from(anchor[2]) + i64::from(offset[2]);
            if !(min_x..min_x + 16).contains(&x) || !(min_z..min_z + 16).contains(&z) {
                continue;
            }
            if self.ore_cell_can_generate(
                column_cache,
                rule,
                anchor,
                offset,
                vein_hash ^ index as u64,
            ) {
                self.place_ore_cell(chunk, rule, anchor, offset);
            }
        }
    }

    fn ore_cell_can_generate(
        &self,
        column_cache: &mut OreColumnCache<'_, '_>,
        rule: &OreRule,
        anchor: [i32; 3],
        offset: [i8; 3],
        cell_hash: u64,
    ) -> bool {
        let Some(world_x) = anchor[0].checked_add(i32::from(offset[0])) else {
            return false;
        };
        let Some(world_y) = anchor[1].checked_add(i32::from(offset[1])) else {
            return false;
        };
        let Some(world_z) = anchor[2].checked_add(i32::from(offset[2])) else {
            return false;
        };
        if !(rule.y.min..=rule.y.max).contains(&world_y) {
            return false;
        }
        let Some(base) = self.generated_cell_before_ores(column_cache, world_x, world_y, world_z)
        else {
            return false;
        };
        if base != self.stone && base != self.deepslate {
            return false;
        }
        !self.should_discard_exposed_ore(column_cache, world_x, world_y, world_z, rule, cell_hash)
    }

    fn place_ore_cell(&self, chunk: &mut Chunk, rule: &OreRule, anchor: [i32; 3], offset: [i8; 3]) {
        let world_x = i64::from(anchor[0]) + i64::from(offset[0]);
        let Some(world_y) = anchor[1].checked_add(i32::from(offset[1])) else {
            return;
        };
        let world_z = i64::from(anchor[2]) + i64::from(offset[2]);
        let chunk_min_x = i64::from(chunk.pos.x) * 16;
        let chunk_min_z = i64::from(chunk.pos.z) * 16;
        if !(chunk_min_x..chunk_min_x + 16).contains(&world_x)
            || !(chunk_min_z..chunk_min_z + 16).contains(&world_z)
        {
            return;
        }
        let lx = (world_x - chunk_min_x) as u8;
        let lz = (world_z - chunk_min_z) as u8;
        let Some(base) = chunk.get_block(lx, world_y, lz) else {
            return;
        };
        if base != self.stone && base != self.deepslate {
            return;
        }
        let ore = self.ore_variant(base, rule.normal, rule.deepslate);
        let _ = chunk.set_block(lx, world_y, lz, ore);
    }

    fn should_discard_exposed_ore(
        &self,
        column_cache: &mut OreColumnCache<'_, '_>,
        world_x: i32,
        y: i32,
        world_z: i32,
        rule: &OreRule,
        hash: u64,
    ) -> bool {
        // Cheap gates first: NaN/zero chance never discards, so the six-cell
        // air scan below must not run at all for those rules — its result is
        // unread and each cell would plan a halo column for nothing.
        let chance = rule.discard_chance_on_air_exposure;
        if !chance.is_finite() || chance <= 0.0 {
            return false;
        }
        if chance >= 1.0 {
            return self.generated_cell_touches_air(column_cache, world_x, y, world_z);
        }
        if !self.generated_cell_touches_air(column_cache, world_x, y, world_z) {
            return false;
        }
        let sample = (hash >> 11) as f64 * (1.0 / ((1_u64 << 53) as f64));
        sample < chance
    }

    fn generated_cell_touches_air(
        &self,
        column_cache: &mut OreColumnCache<'_, '_>,
        world_x: i32,
        y: i32,
        world_z: i32,
    ) -> bool {
        ORE_DIRECTIONS.iter().any(|direction| {
            let neighbour = world_x
                .checked_add(i32::from(direction[0]))
                .zip(y.checked_add(i32::from(direction[1])))
                .zip(world_z.checked_add(i32::from(direction[2])));
            let Some(((nx, ny), nz)) = neighbour else {
                return true;
            };
            self.generated_cell_before_ores(column_cache, nx, ny, nz) == Some(self.air)
        })
    }

    fn generated_cell_before_ores(
        &self,
        column_cache: &mut OreColumnCache<'_, '_>,
        world_x: i32,
        y: i32,
        world_z: i32,
    ) -> Option<BlockStateId> {
        if y < self.geometry.min_y() || y >= self.geometry.max_y() {
            return None;
        }
        let (top_non_air, height, dirt_start, fill, surface, cave_bounds) = {
            let plan = column_cache.get_or_plan(world_x, world_z)?;
            (
                plan.top_non_air,
                plan.height,
                plan.dirt_start,
                plan.fill,
                plan.surface,
                self.cave_y_bounds(plan),
            )
        };
        let pos = ChunkPos {
            x: world_x.div_euclid(16),
            z: world_z.div_euclid(16),
        };
        if y > top_non_air {
            return Some(self.air);
        }
        if y > height {
            return Some(self.water);
        }
        if y == self.geometry.min_y() {
            return Some(self.bedrock);
        }

        if let Some((cave_min_y, cave_max_y)) = cave_bounds
            && (cave_min_y..=cave_max_y).contains(&y)
        {
            let step = i64::from(CAVE_VERTICAL_SAMPLE_STEP);
            let sample_y = i64::from(cave_min_y)
                + (i64::from(y) - i64::from(cave_min_y)).div_euclid(step) * step;
            let Ok(sample_y) = i32::try_from(sample_y) else {
                return None;
            };
            if column_cache.get_or_cave(self, world_x, sample_y, world_z, height) {
                return Some(self.air);
            }
        }
        if y < dirt_start {
            return Some(self.base_stone_for_y(
                world_x.rem_euclid(16) as u8,
                y,
                world_z.rem_euclid(16) as u8,
                pos,
            ));
        }
        if y < height {
            return Some(fill);
        }
        Some(surface)
    }

    /// Write every plan's piece blocks into this chunk and refresh the
    /// heightmaps of the columns they touched.
    ///
    /// Pieces are clipped to the chunk's box, so a village spanning several
    /// chunks writes each chunk's part of it when that chunk is filled, and a
    /// chunk two villages reach writes both.
    ///
    /// A piece that cannot be placed is a registry mismatch, and
    /// [`VillagePlanSource::new`] refuses such a registry at startup rather than
    /// dropping blocks here. Chunk generation has no way to report a failure, so
    /// the invariant is enforced where it can be, once, before a world uses the
    /// village — and reaching the failure here means the village would silently
    /// lose blocks, which is worse than stopping.
    fn apply_village_pieces(
        &self,
        chunk: &mut Chunk,
        plans: &VillagePlanSet,
        heights: &mut HeightContext<'_>,
    ) {
        let Some(source) = self.village_plans.as_ref() else {
            return;
        };
        // The piece writer's surface is the terrain's own (SOURCE) surface —
        // the `WORLD_SURFACE_WG` heightmap the `terrain_matching` projection's
        // gravity processor reads — served from the shared context so repeated
        // piece-column queries do not re-run the density query per column.
        let heights_cell = RefCell::new(heights);
        let surface = |x: i32, z: i32| heights_cell.borrow_mut().structure_surface(x, z);
        let mut touched = [false; 256];
        source
            .write_plans(chunk, plans, &mut touched, &surface, self.air)
            .expect("the plan source was validated against this registry at construction");
        for lz in 0..16u8 {
            for lx in 0..16u8 {
                if touched[lz as usize * 16 + lx as usize] {
                    self.refresh_structure_column(chunk, lx, lz);
                }
            }
        }
    }

    fn apply_jigsaw_structures(&self, chunk: &mut Chunk) {
        let surface = |x, z| self.first_free_height(x, z);
        let biome = |x, y, z| self.noise_biome(x, y, z);
        let mut touched = [false; 256];
        for source in &self.jigsaw_sources {
            let Some(plans) = source.plans_for_chunk(chunk.pos.x, chunk.pos.z, &surface, &biome)
            else {
                continue;
            };
            source
                .write_plans(chunk, &plans, &mut touched, &surface, self.air)
                .expect("jigsaw source validated at construction");
        }
        for z in 0..16u8 {
            for x in 0..16u8 {
                if touched[z as usize * 16 + x as usize] {
                    self.refresh_structure_column(chunk, x, z);
                }
            }
        }
    }

    fn apply_decorations(
        &self,
        chunk: &mut Chunk,
        columns: &[ColumnPlan; 256],
        heights: &mut HeightContext<'_>,
    ) {
        let mut touched = [None; 256];
        let mut leaves = Vec::new();
        for lz in 0..16u8 {
            for lx in 0..16u8 {
                let idx = lz as usize * 16 + lx as usize;
                let plan = &columns[idx];
                let height = plan.height;
                let Some(decoration_limit) = checked_y_offset(height, 8) else {
                    continue;
                };
                let Some(base_y) = checked_y_offset(height, 1) else {
                    continue;
                };
                if height <= self.geometry.min_y() || decoration_limit >= self.geometry.max_y() {
                    continue;
                }
                let biome = &plan.biome;
                let surface = plan.surface;
                let h = plan.hash;

                if chunk.get_block(lx, height, lz) != Some(surface) {
                    continue;
                }

                let tree_spacing = self.tree_spacing_for_biome(biome);
                let base_tree = self
                    .base_tree_spacing(biome)
                    .is_some_and(|spacing| h.is_multiple_of(spacing));
                if (base_tree
                    || (tree_spacing.is_some_and(|spacing| h.is_multiple_of(spacing))
                        && self.tree_density_allows(plan)))
                    && self.tree_site_is_stable(plan, heights)
                    && self.place_tree(
                        chunk,
                        plan,
                        self.tree_blocks_for_biome(biome),
                        &mut touched,
                        &mut leaves,
                    )
                {
                    continue;
                }
                if self.biomes.hot_dry.contains(biome)
                    && surface == self.sand
                    && h.is_multiple_of(47)
                    && self.place_cactus(chunk, lx, base_y, lz, &mut touched)
                {
                    continue;
                }
                if (self.biomes.beach.contains(biome)
                    || self.biomes.river.contains(biome)
                    || self.biomes.swamp.contains(biome))
                    && h.is_multiple_of(29)
                    && self.place_sugar_cane(chunk, lx, base_y, lz, &mut touched)
                {
                    continue;
                }
                if (self.biomes.grassland.contains(biome)
                    || self.biomes.temperate_forest.contains(biome)
                    || self.biomes.jungle.contains(biome)
                    || self.biomes.swamp.contains(biome)
                    || Self::is_savanna(biome))
                    && (surface == self.grass_block || surface == self.podzol)
                {
                    let (grass_spacing, dandelion_spacing, poppy_spacing) =
                        self.plant_spacing_for_biome(biome);
                    let plant = if Self::is_savanna(biome) {
                        if h.is_multiple_of(grass_spacing) {
                            self.decorations.short_grass
                        } else {
                            None
                        }
                    } else if h.is_multiple_of(1021) {
                        self.decorations.pumpkin
                    } else if self.biomes.swamp.contains(biome) {
                        if h.is_multiple_of(dandelion_spacing) {
                            self.decorations.blue_orchid
                        } else if h.is_multiple_of(grass_spacing) {
                            self.decorations.short_grass
                        } else {
                            None
                        }
                    } else if h.is_multiple_of(dandelion_spacing) {
                        self.decorations.dandelion
                    } else if h.is_multiple_of(poppy_spacing) {
                        self.decorations.poppy
                    } else if h.is_multiple_of(grass_spacing) {
                        self.decorations.short_grass
                    } else {
                        None
                    };
                    if let Some(plant) = plant
                        && chunk.get_block(lx, base_y, lz) == Some(self.air)
                    {
                        self.place_single(chunk, lx, base_y, lz, plant, &mut touched);
                    }
                }
                if self.biomes.is_ocean(biome)
                    && checked_y_offset(height, 2)
                        .is_some_and(|minimum_top| plan.top_non_air > minimum_top)
                {
                    if h.is_multiple_of(31) && self.place_kelp_column(chunk, plan, h, &mut touched)
                    {
                        continue;
                    }
                    if h.is_multiple_of(17)
                        && let Some(seagrass) = self.decorations.seagrass
                        && chunk.get_block(lx, base_y, lz) == Some(self.water)
                    {
                        self.place_single(chunk, lx, base_y, lz, seagrass, &mut touched);
                    }
                }
            }
        }
        self.initialize_leaf_distances(chunk, &mut leaves);
        self.apply_sediments(chunk, columns);
        for lz in 0..16u8 {
            for lx in 0..16u8 {
                if let Some(top) = touched[lz as usize * 16 + lx as usize] {
                    self.refresh_known_top_column(chunk, lx, lz, top);
                }
            }
        }
    }

    fn initialize_leaf_distances(&self, chunk: &mut Chunk, leaves: &mut Vec<(u8, i32, u8)>) {
        let palettes = &self.decorations.leaf_distances;
        let neighbours = |(x, y, z): (u8, i32, u8)| {
            [
                (-1, 0, 0),
                (1, 0, 0),
                (0, -1, 0),
                (0, 1, 0),
                (0, 0, -1),
                (0, 0, 1),
            ]
            .into_iter()
            .filter_map(move |(dx, dy, dz)| {
                let x = x.checked_add_signed(dx)?;
                let y = y.checked_add(dy)?;
                let z = z.checked_add_signed(dz)?;
                (x < 16 && z < 16).then_some((x, y, z))
            })
        };
        // Resolve the complete generated leaf graph before publishing the chunk.
        // Persistent remains false; later log removal uses ordinary scheduled decay.
        leaves.retain(|&(x, y, z)| {
            let state = chunk
                .get_block(x, y, z)
                .expect("generated leaf is in the chunk");
            let Some(palette) = palettes
                .iter()
                .flatten()
                .find(|palette| palette[6] == state)
            else {
                return false;
            };
            let supported = neighbours((x, y, z)).any(|(nx, ny, nz)| {
                chunk.get_block(nx, ny, nz).is_some_and(|state| {
                    self.decorations
                        .leaf_supports
                        .get(state.0 as usize)
                        .copied()
                        .unwrap_or(false)
                })
            });
            if supported {
                chunk.set_block(x, y, z, palette[0]);
            }
            supported
        });
        let mut start = 0;
        for distance in 2..=6 {
            let end = leaves.len();
            for index in start..end {
                for (x, y, z) in neighbours(leaves[index]) {
                    let Some(state) = chunk.get_block(x, y, z) else {
                        continue;
                    };
                    if let Some(palette) = palettes
                        .iter()
                        .flatten()
                        .find(|palette| palette[6] == state)
                    {
                        chunk.set_block(x, y, z, palette[distance - 1]);
                        leaves.push((x, y, z));
                    }
                }
            }
            start = end;
        }
    }

    fn tree_site_is_stable(&self, plan: &ColumnPlan, heights: &mut HeightContext<'_>) -> bool {
        // Keep the trunk and nearby canopy footprint off sharp terrain ledges.
        // Neighbour heights come from the generation-local context — one shared
        // sampler + one plan memo — instead of 25 independent `surface_height`
        // queries that each re-run the density query and the village lookup.
        for dz in -2..=2 {
            for dx in -2..=2 {
                let Some(wx) = plan.wx.checked_add(dx) else {
                    return false;
                };
                let Some(wz) = plan.wz.checked_add(dz) else {
                    return false;
                };
                let neighbour = heights.adjusted_height(wx, wz);
                if (neighbour - plan.height).abs() > 1 {
                    return false;
                }
            }
        }
        true
    }

    fn is_savanna(biome: &Identifier) -> bool {
        biome.path().contains("savanna")
    }

    fn is_cold_forest(biome: &Identifier) -> bool {
        biome.path().contains("taiga") || biome.path() == "grove"
    }

    fn tree_density_allows(&self, plan: &ColumnPlan) -> bool {
        if let Some(rule) = self.tree_rules.get(&plan.biome) {
            return plan.vegetation_density >= rule.density_threshold;
        }
        let threshold = if self.biomes.jungle.contains(&plan.biome) {
            -0.55
        } else if self.biomes.temperate_forest.contains(&plan.biome) {
            -0.25
        } else if self.biomes.swamp.contains(&plan.biome) {
            // Swamp trees root in wet soil wherever the tightened swamp band
            // applies; previously swamp had no branch here at all, which made
            // every swamp column barren by construction.
            -0.45
        } else if Self::is_cold_forest(&plan.biome) || Self::is_savanna(&plan.biome) {
            -0.05
        } else if self.biomes.grassland.contains(&plan.biome) {
            0.0
        } else {
            return false;
        };
        plan.vegetation_density >= threshold
    }

    /// Columns per tree a biome carries everywhere, before the vegetation
    /// field's denser groves (vanilla vegetation is per biome, never barren
    /// regions): plains `trees_plains` one tree in ~20 chunks, savanna
    /// `trees_savanna` ~1.1 per chunk, swamp 2 per chunk; forests, taiga and
    /// jungle keep their full spacing everywhere.
    fn base_tree_spacing(&self, biome: &Identifier) -> Option<u64> {
        if self.biomes.grassland.contains(biome) {
            Some(GRASSLAND_SPARSE_TREE_SPACING)
        } else if Self::is_savanna(biome) {
            Some(233)
        } else if self.biomes.swamp.contains(biome) {
            Some(127)
        } else if self.biomes.jungle.contains(biome)
            || self.biomes.temperate_forest.contains(biome)
            || Self::is_cold_forest(biome)
        {
            self.tree_spacing_for_biome(biome)
        } else {
            None
        }
    }

    fn tree_spacing_for_biome(&self, biome: &Identifier) -> Option<u64> {
        if let Some(rule) = self.tree_rules.get(biome) {
            return Some(rule.spacing);
        }
        if self.biomes.jungle.contains(biome) {
            // Vanilla jungle vegetation makes about 50 attempts per chunk,
            // mixing trees with a low bush layer rather than sparse trunks.
            Some(5)
        } else if self.biomes.temperate_forest.contains(biome) {
            Some(37)
        } else if self.biomes.swamp.contains(biome) {
            Some(31)
        } else if Self::is_cold_forest(biome) {
            Some(47)
        } else if Self::is_savanna(biome) {
            Some(113)
        } else if self.biomes.grassland.contains(biome) {
            Some(173)
        } else {
            None
        }
    }

    fn plant_spacing_for_biome(&self, biome: &Identifier) -> (u64, u64, u64) {
        if Self::is_savanna(biome) {
            (31, 127, 137)
        } else if self.biomes.jungle.contains(biome) {
            (13, 89, 97)
        } else if self.biomes.swamp.contains(biome) {
            (15, 71, 79)
        } else if self.biomes.temperate_forest.contains(biome) {
            (29, 127, 137)
        } else {
            // Common lowlands at 1/19 grass read as featureless lawns from
            // any distance; 1/15 keeps the existing moderate-cover test cap.
            (15, 61, 67)
        }
    }

    fn place_cactus(
        &self,
        chunk: &mut Chunk,
        lx: u8,
        base_y: i32,
        lz: u8,
        touched: &mut [Option<i32>; 256],
    ) -> bool {
        let Some(cactus) = self.decorations.cactus else {
            return false;
        };
        let height =
            1 + (feature_hash(self.seed, lx as i32, base_y, lz as i32, 0xCA_C7) % 3) as i32;
        let Some(top_exclusive) = checked_y_offset(base_y, height) else {
            return false;
        };
        for y in base_y..top_exclusive {
            if chunk.get_block(lx, y, lz) != Some(self.air) {
                return false;
            }
        }
        for y in base_y..top_exclusive {
            self.place_single(chunk, lx, y, lz, cactus, touched);
        }
        true
    }

    fn place_sugar_cane(
        &self,
        chunk: &mut Chunk,
        lx: u8,
        base_y: i32,
        lz: u8,
        touched: &mut [Option<i32>; 256],
    ) -> bool {
        let Some(sugar_cane) = self.decorations.sugar_cane else {
            return false;
        };
        let Some(below_y) = checked_y_offset(base_y, -1) else {
            return false;
        };
        if chunk.get_block(lx, base_y, lz) != Some(self.air)
            || !self.has_adjacent_water(chunk, lx, below_y, lz)
        {
            return false;
        }
        self.place_single(chunk, lx, base_y, lz, sugar_cane, touched);
        if let Some(above_y) = checked_y_offset(base_y, 1)
            && chunk.get_block(lx, above_y, lz) == Some(self.air)
        {
            self.place_single(chunk, lx, above_y, lz, sugar_cane, touched);
        }
        true
    }

    fn has_adjacent_water(&self, chunk: &Chunk, lx: u8, y: i32, lz: u8) -> bool {
        [(1i8, 0i8), (-1, 0), (0, 1), (0, -1)]
            .into_iter()
            .any(|(dx, dz)| {
                let x = lx as i8 + dx;
                let z = lz as i8 + dz;
                if !(0..16).contains(&x) || !(0..16).contains(&z) {
                    return false;
                }
                let x = x as u8;
                let z = z as u8;
                chunk.get_block(x, y, z) == Some(self.water)
                    || checked_y_offset(y, 1).and_then(|above_y| chunk.get_block(x, above_y, z))
                        == Some(self.water)
            })
    }

    fn place_kelp_column(
        &self,
        chunk: &mut Chunk,
        plan: &ColumnPlan,
        hash: u64,
        touched: &mut [Option<i32>; 256],
    ) -> bool {
        let lx = plan.lx;
        let lz = plan.lz;
        let Some(base_y) = checked_y_offset(plan.height, 1) else {
            return false;
        };
        let water_top = plan.top_non_air;
        let Some(kelp) = self.decorations.kelp else {
            return false;
        };
        let stem = self.decorations.kelp_plant.unwrap_or(kelp);
        let available = i64::from(water_top) - i64::from(base_y) + 1;
        if available < 2 {
            return false;
        }
        let height = i64::from(2 + (hash % 5) as i32).min(available);
        let Ok(top_y) = i32::try_from(i64::from(base_y) + height - 1) else {
            return false;
        };
        for y in base_y..=top_y {
            if chunk.get_block(lx, y, lz) != Some(self.water) {
                return false;
            }
        }
        for y in base_y..top_y {
            self.place_single(chunk, lx, y, lz, stem, touched);
        }
        self.place_single(chunk, lx, top_y, lz, kelp, touched);
        true
    }

    fn place_single(
        &self,
        chunk: &mut Chunk,
        lx: u8,
        y: i32,
        lz: u8,
        state: BlockStateId,
        touched: &mut [Option<i32>; 256],
    ) {
        if chunk.set_block(lx, y, lz, state).is_some() {
            let touched = &mut touched[lz as usize * 16 + lx as usize];
            *touched = Some(touched.map_or(y, |top| top.max(y)));
        }
    }

    fn refresh_known_top_column(&self, chunk: &mut Chunk, lx: u8, lz: u8, top: i32) {
        let current = chunk
            .heightmaps
            .get("MOTION_BLOCKING")
            .map(|heightmap| {
                i64::from(heightmap.get(lx, lz)) + i64::from(self.geometry.min_y()) - 1
            })
            .unwrap_or(i64::from(self.geometry.min_y()));
        let top = i64::from(top).max(current);
        let Ok(top) = i32::try_from(top) else {
            return;
        };
        let Some(value) = heightmap_value_for_top(self.geometry, top) else {
            return;
        };
        if let Some(mb) = chunk.heightmaps.get_mut("MOTION_BLOCKING") {
            mb.set(lx, lz, value);
        }
        if let Some(ws) = chunk.heightmaps.get_mut("WORLD_SURFACE") {
            ws.set(lx, lz, value);
        }
        chunk.highest_opaque.set(lx, lz, value);
    }

    fn refresh_structure_column(&self, chunk: &mut Chunk, lx: u8, lz: u8) {
        let top = (self.geometry.min_y()..self.geometry.max_y())
            .rev()
            .find(|&y| {
                chunk
                    .get_block(lx, y, lz)
                    .is_some_and(|state| state != self.air)
            })
            .unwrap_or(self.geometry.min_y());
        let Some(value) = heightmap_value_for_top(self.geometry, top) else {
            return;
        };
        if let Some(mb) = chunk.heightmaps.get_mut("MOTION_BLOCKING") {
            mb.set(lx, lz, value);
        }
        if let Some(ws) = chunk.heightmaps.get_mut("WORLD_SURFACE") {
            ws.set(lx, lz, value);
        }
        chunk.highest_opaque.set(lx, lz, value);
    }

    fn base_stone_for_y(&self, lx: u8, y: i32, lz: u8, pos: ChunkPos) -> BlockStateId {
        if y <= DEEPSLATE_SOLID_Y {
            return self.deepslate;
        }
        if y >= DEEPSLATE_TOP_Y {
            return self.stone;
        }
        let wx = world_block_coordinate(pos.x, lx);
        let wz = world_block_coordinate(pos.z, lz);
        // Vanilla's vertical_gradient is certain at Y=0 and zero at Y=8.
        // Keep the existing coordinate hash, but use a linear probability.
        let deepslate_chance = (DEEPSLATE_TOP_Y - y) as u64;
        let span = (DEEPSLATE_TOP_Y - DEEPSLATE_SOLID_Y) as u64;
        if feature_hash(self.seed, wx, y, wz, 0xD33F) % span < deepslate_chance {
            self.deepslate
        } else {
            self.stone
        }
    }

    fn is_cave_cell(&self, x: i32, y: i32, z: i32, surface_y: i32) -> bool {
        if let Some((_, density)) = self.vanilla() {
            return y < surface_y && density.density(x, y, z) <= 0.0;
        }
        self.density_router().is_cave(x, y, z, surface_y)
    }

    #[cfg(test)]
    fn ore_for(
        &self,
        x: i32,
        y: i32,
        z: i32,
        base: BlockStateId,
        biome: &Identifier,
    ) -> BlockStateId {
        let h = feature_hash(self.seed, x, y, z, 0x0A_E0);
        for rule in self.ores.rules() {
            if rule.matches(h, y, biome) {
                return self.ore_variant(base, rule.normal, rule.deepslate);
            }
        }
        base
    }

    fn ore_variant(
        &self,
        base: BlockStateId,
        stone_ore: BlockStateId,
        deepslate_ore: BlockStateId,
    ) -> BlockStateId {
        if base == self.deepslate {
            deepslate_ore
        } else {
            stone_ore
        }
    }
}

fn feature_hash(seed: i64, x: i32, y: i32, z: i32, salt: u64) -> u64 {
    let mut h = seed as u64 ^ salt;
    h ^= (x as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h = h.rotate_left(17);
    h ^= (y as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h = h.rotate_left(23);
    h ^= (z as i64 as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h.wrapping_mul(0x94D0_49BB_1331_11EB) ^ (h >> 31)
}

impl TerrainGenerator {
    /// Far-terrain LOD summary of a never-generated column: the router's
    /// cave-free surface on the column's lattice corner and the biome there.
    /// Villages, carvers and decoration are not applied; nothing is cached.
    #[must_use]
    pub fn lod_column(&self, world_x: i32, world_z: i32) -> (i32, Identifier) {
        let clamp = |y: i32| y.clamp(self.geometry.min_y(), self.geometry.max_y() - 1);
        if let Some((climate, density)) = self.vanilla()
            && let Some(y) = density.lod_surface_height(world_x, world_z)
        {
            let y = clamp(y);
            let biome = Identifier::parse(climate.sample(world_x, y, world_z).biome)
                .expect("validated vanilla biome identifier");
            return (y, biome);
        }
        let sample = self.diagnostic_sample_uncached(world_x, world_z, None);
        (clamp(sample.surface_y), sample.biome)
    }

    /// The far-terrain generation program native clients may run for
    /// never-generated columns (see [`crate::vanilla_density::program`]);
    /// `Ok(None)` without vanilla content or the LOD surface shape. It
    /// reveals the seed's terrain noise: send it only on operator opt-in.
    pub fn lod_program(&self) -> Result<Option<Vec<u8>>, String> {
        match self.vanilla() {
            Some((climate, density)) => crate::vanilla_density::program::encode(density, climate),
            None => Ok(None),
        }
    }

    /// Evaluate chunk density on the GPU in batches shared by every thread
    /// generating chunks (call after [`Self::load_vanilla_worldgen`]).
    /// Returns the adapter's name; errors leave generation on the CPU.
    pub fn enable_gpu(&mut self) -> Result<String, String> {
        #[cfg(feature = "gpu")]
        {
            let (_, density) = self
                .vanilla()
                .ok_or("GPU worldgen needs vanilla worldgen content")?;
            let gpu = crate::vanilla_density::gpu::GpuDensity::new(density)?;
            let adapter = gpu.adapter().to_owned();
            self.gpu = Some(
                gpu_service::GpuSolidService::spawn(gpu)
                    .map_err(|error| format!("worldgen GPU thread: {error}"))?,
            );
            Ok(adapter)
        }
        #[cfg(not(feature = "gpu"))]
        Err("this build has no GPU worldgen (feature `gpu`)".into())
    }
}

impl ChunkGenerator for TerrainGenerator {
    fn village_sites_in_region(
        &self,
        min_chunk: (i32, i32),
        max_chunk: (i32, i32),
    ) -> Vec<mc_world::GeneratedVillageSite> {
        TerrainGenerator::village_sites_in_region(self, min_chunk, max_chunk)
    }

    fn structure_sites_in_region(
        &self,
        min: (i32, i32),
        max: (i32, i32),
    ) -> Vec<mc_world::chunk::GeneratedStructureSite> {
        let mut sites = self
            .overworld_structures
            .as_ref()
            .map_or_else(Vec::new, |structures| {
                structures.sites(
                    min,
                    max,
                    |x, z| self.world_surface_y(self.surface_height(x, z)),
                    |x, z| self.surface_height(x, z),
                    |x, y, z| self.noise_biome(x, y, z),
                )
            });
        let surface = |x, z| self.first_free_height(x, z);
        let biome = |x, y, z| self.noise_biome(x, y, z);
        for source in &self.jigsaw_sources {
            sites.extend(source.structure_sites_in_region(min, max, &surface, &biome));
        }
        sites
    }

    fn noise_biome(&self, world_x: i32, y: i32, world_z: i32) -> Option<Identifier> {
        Some(TerrainGenerator::noise_biome(self, world_x, y, world_z))
    }

    fn surface_height(&self, world_x: i32, world_z: i32) -> Option<i32> {
        Some(Self::surface_height(self, world_x, world_z))
    }

    fn lod_column(&self, world_x: i32, world_z: i32) -> Option<(i32, Identifier)> {
        Some(Self::lod_column(self, world_x, world_z))
    }

    fn lod_program(&self) -> Result<Option<Vec<u8>>, String> {
        Self::lod_program(self)
    }

    fn generate(&self, pos: ChunkPos) -> Chunk {
        #[cfg(feature = "gpu")]
        if let Some(grid) = self.gpu.as_ref().and_then(|gpu| gpu.solids_around(pos)) {
            return self.generate_with_solids(pos, Some(&grid));
        }
        match self.vanilla() {
            Some((_, density)) => {
                let grid = self
                    .solid_cache
                    .around(pos, |pos| chunk_solids(density, pos));
                self.generate_with_solids(pos, Some(&grid))
            }
            None => self.generate_with_solids(pos, None),
        }
    }
}

/// Which blocks of the chunk at `pos` the density makes solid, on the CPU.
fn chunk_solids(density: &VanillaDensity, pos: ChunkPos) -> ChunkSolids {
    let mut values = vec![0.0; density.height() as usize * 256];
    density.fill_density_chunk(
        world_block_coordinate(pos.x, 0),
        world_block_coordinate(pos.z, 0),
        &mut values,
    );
    ChunkSolids::from_densities(&values, density.height() as usize)
}

impl TerrainGenerator {
    /// [`ChunkGenerator::generate`] reading which blocks the density makes
    /// solid from `solids` (computed elsewhere, e.g. the GPU worldgen
    /// experiment) for this chunk and any held neighbour; anything not held
    /// is computed here.
    pub fn generate_with_solids(&self, pos: ChunkPos, solids: Option<&SolidGrid>) -> Chunk {
        #[cfg(test)]
        let mut stage_clock = profile_tests::StageClock::new();
        let mut chunk =
            Chunk::empty_with_geometry(pos, self.air, self.biomes.default.clone(), self.geometry);
        chunk
            .heightmaps
            .insert("MOTION_BLOCKING".into(), Heightmap::zeroed());
        chunk
            .heightmaps
            .insert("WORLD_SURFACE".into(), Heightmap::zeroed());
        #[cfg(test)]
        stage_clock.end(5);
        let native_solids = self.vanilla().map(|(climate, noise)| {
            let min_x = world_block_coordinate(pos.x, 0);
            let min_z = world_block_coordinate(pos.z, 0);
            let values = match solids.and_then(|solids| solids.chunk(pos)) {
                Some(held) => std::borrow::Cow::Borrowed(held),
                None => std::borrow::Cow::Owned(chunk_solids(noise, pos)),
            };
            // Density offsets inside the geometry: the top search's range.
            let low = (self.geometry.min_y() - noise.min_y()).max(0);
            let high = (self.geometry.max_y() - noise.min_y()).clamp(0, noise.height());
            let density = noise;
            #[cfg(test)]
            stage_clock.end(0);
            // Block columns grouped by biome quart: one column sampler then
            // reuses each quart's Y-free climate fields for its 16 columns.
            let mut sampler = climate.column_sampler();
            let mut parsed: Vec<(&str, Identifier)> = Vec::new();
            for order in 0..256 {
                let (quart, inner) = (order / 16, order % 16);
                let idx = (quart / 4 * 4 + inner / 4) * 16 + quart % 4 * 4 + inner % 4;
                let x = min_x + (idx % 16) as i32;
                let z = min_z + (idx / 16) as i32;
                let top = values
                    .top_below(idx, high as usize)
                    .filter(|offset| *offset as i32 >= low)
                    .map_or(self.geometry.min_y(), |offset| {
                        density.min_y() + offset as i32
                    });
                let native = sampler.sample(x, top, z);
                let biome = match parsed.iter().find(|(name, _)| *name == native.biome) {
                    Some((_, biome)) => biome.clone(),
                    None => {
                        let biome = Identifier::parse(native.biome)
                            .expect("validated vanilla biome identifier");
                        parsed.push((native.biome, biome.clone()));
                        biome
                    }
                };
                let vegetation_density = self
                    .biome_supports_land_vegetation(&biome)
                    .then(|| self.vegetation_density(x, z, f64::from(native.climate[1])));
                self.diagnostic_cache.insert(
                    x,
                    z,
                    TerrainDiagnosticSample {
                        surface_y: top,
                        biome,
                        vegetation_density,
                    },
                );
            }
            values
        });
        #[cfg(test)]
        stage_clock.end(6);
        let village = self.village_plans_for_chunk(pos);
        #[cfg(test)]
        stage_clock.end(4);
        let columns = std::array::from_fn(|idx| {
            let lx = (idx % 16) as u8;
            let lz = (idx / 16) as u8;
            self.plan_column(pos, lx, lz, village.as_ref())
        });
        // The generation-local height context: ready central-chunk SOURCE
        // heights (the diagnostic cache's own entries the columns planned
        // from, so no second derivation can disagree) plus ONE shared native
        // halo sampler for lazily reached neighbours plus the village-plan
        // memo. Tree, ore-halo and structure readers consult it instead of
        // re-running the full height + plan pipeline per column.
        //
        // The context's sampler covers exactly the columns a chunk fill can
        // read: the halo's reach is `ORE_COLUMN_HALO` (ore veins) inside the
        // ±2 tree-stability footprint; the sampler bound is the union.
        let source_columns: [(i32, i32, i32); 256] = std::array::from_fn(|idx| {
            let plan = &columns[idx];
            // The diagnostic cache already holds this column's SOURCE surface
            // (the fill-density pass inserted it above); `plan_column` read
            // the same entry, so no second derivation can disagree.
            let base = self
                .diagnostic_cache
                .get(plan.wx, plan.wz)
                .map(|sample| sample.surface_y)
                .unwrap_or_else(|| self.base_surface_y(plan.wx, plan.wz));
            (plan.wx, plan.wz, base)
        });
        let mut heights = HeightContext::for_chunk(
            self,
            pos,
            &source_columns,
            self.vanilla().map(|(_, density)| {
                let min_x = world_block_coordinate(pos.x, 0) as i64 - i64::from(ORE_COLUMN_HALO);
                let min_z = world_block_coordinate(pos.z, 0) as i64 - i64::from(ORE_COLUMN_HALO);
                let clamp =
                    |value: i64| value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
                density.surface_height_sampler(
                    clamp(min_x),
                    clamp(min_z),
                    clamp(min_x + 16 + i64::from(ORE_COLUMN_HALO) * 2 - 1),
                    clamp(min_z + 16 + i64::from(ORE_COLUMN_HALO) * 2 - 1),
                )
            }),
        );
        if let (Some(solids), Some((_, density))) = (solids, self.vanilla()) {
            heights = heights.with_solids(solids, density.min_y());
        }

        // Apply the same CPU aquifer decisions to CPU and GPU solid grids.
        let mut aquifer = self.vanilla().map(|(_, density)| {
            crate::vanilla_density::aquifer::Aquifer::new(density, self.seed as u64)
        });

        for plan in &columns {
            if let (Some(values), Some((_, density))) = (&native_solids, self.vanilla()) {
                let idx = usize::from(plan.lz) * 16 + usize::from(plan.lx);
                let badlands = plan.biome.path().contains("badlands");
                for y in self.geometry.min_y()..=plan.top_non_air {
                    let offset = y - density.min_y();
                    let solid = offset >= 0
                        && offset < density.height()
                        && values.solid(idx, offset as usize);
                    let state = if y == self.geometry.min_y() {
                        self.bedrock
                    } else if y == plan.height {
                        plan.surface
                    } else if y > plan.height {
                        self.water
                    } else if solid || y >= plan.dirt_start {
                        if badlands && y >= 64 {
                            let band =
                                (y.div_euclid(3) + self.seed.rem_euclid(8) as i32).rem_euclid(8);
                            self.terracotta_layers[band as usize]
                        } else if y >= plan.dirt_start {
                            plan.fill
                        } else {
                            self.base_stone_for_y(plan.lx, y, plan.lz, pos)
                        }
                    } else {
                        use crate::vanilla_density::aquifer::Fluid;
                        match aquifer
                            .as_mut()
                            .expect("vanilla aquifer")
                            .substance(plan.wx, y, plan.wz)
                        {
                            Fluid::Air => self.air,
                            Fluid::Water => self.water,
                            Fluid::Lava => self.cave_lava,
                            Fluid::Solid => self.base_stone_for_y(plan.lx, y, plan.lz, pos),
                        }
                    };
                    let _ = chunk.set_block(plan.lx, y, plan.lz, state);
                }
            } else {
                self.fill_column(&mut chunk, plan);
            }
            // Heightmap value: Y of the first air cell above the
            // Heightmaps store offsets from this dimension's minimum Y.
            let Some(world_surface) = heightmap_value_for_top(self.geometry, plan.top_non_air)
            else {
                continue;
            };
            let Some(motion_blocking) = heightmap_value_for_top(self.geometry, plan.height) else {
                continue;
            };
            if let Some(mb) = chunk.heightmaps.get_mut("MOTION_BLOCKING") {
                mb.set(plan.lx, plan.lz, motion_blocking);
            }
            if let Some(ws) = chunk.heightmaps.get_mut("WORLD_SURFACE") {
                ws.set(plan.lx, plan.lz, world_surface);
            }
            chunk.highest_opaque.set(plan.lx, plan.lz, motion_blocking);
        }
        #[cfg(test)]
        stage_clock.end(1);
        if self.geological_ores.is_some() {
            self.apply_caves(&mut chunk, &columns);
            #[cfg(test)]
            stage_clock.end(2);
            if let Some(geological_ores) = &self.geological_ores {
                geological_ores.apply(self, &mut chunk);
            }
        } else {
            #[cfg(test)]
            stage_clock.end(3);
            {
                let mut ore_cache = self.ore_column_cache(&chunk, &columns, &mut heights);
                self.apply_caves_with_cache(&mut chunk, &columns, Some(&mut ore_cache));
                #[cfg(test)]
                stage_clock.end(2);
                self.apply_ores_with_cache(&mut chunk, &mut ore_cache);
            }
        }
        #[cfg(test)]
        stage_clock.end(3);
        self.assign_biomes(&mut chunk, &columns);
        #[cfg(test)]
        stage_clock.end(6);
        if let Some(plans) = village.as_ref() {
            {
                self.apply_village_pieces(&mut chunk, plans, &mut heights);
            }
        }
        self.apply_jigsaw_structures(&mut chunk);
        #[cfg(test)]
        stage_clock.end(4);
        {
            self.apply_decorations(&mut chunk, &columns, &mut heights);
        }
        #[cfg(test)]
        stage_clock.end(7);
        if let Some(structures) = &self.overworld_structures {
            let heights_cell = std::cell::RefCell::new(&mut heights);
            let touched = structures.paste(
                &mut chunk,
                |x, z| heights_cell.borrow_mut().structure_surface(x, z),
                |x, z| heights_cell.borrow_mut().adjusted_height(x, z),
                |x, y, z| self.noise_biome(x, y, z),
            );
            for (index, changed) in touched.into_iter().enumerate() {
                if changed {
                    self.refresh_structure_column(
                        &mut chunk,
                        (index % 16) as u8,
                        (index / 16) as u8,
                    );
                }
            }
        }
        #[cfg(test)]
        stage_clock.end(8);
        chunk.status = "minecraft:full".into();
        chunk.mark_dirty();
        #[cfg(test)]
        stage_clock.end(5);
        chunk
    }
}

/// Palette index of `biome` in a biome section, appended on first use.
fn palette_slot(palette: &mut Vec<Identifier>, biome: Identifier) -> usize {
    palette
        .iter()
        .position(|entry| entry == &biome)
        .unwrap_or_else(|| {
            palette.push(biome);
            palette.len() - 1
        })
}

#[cfg(test)]
#[path = "terrain/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "terrain/deepslate_tests.rs"]
mod deepslate_tests;

#[cfg(test)]
#[path = "terrain/coastal_tests.rs"]
mod coastal_tests;

#[cfg(test)]
#[path = "terrain/village_water_tests.rs"]
mod village_water_tests;

#[cfg(test)]
#[path = "terrain/village_seed_tests.rs"]
mod village_seed_tests;

#[cfg(test)]
#[path = "terrain/profile_tests.rs"]
mod profile_tests;

#[cfg(test)]
#[path = "terrain/environment_tests.rs"]
mod environment_tests;

#[cfg(test)]
#[path = "terrain/modern_jigsaw_tests.rs"]
mod modern_jigsaw_tests;
