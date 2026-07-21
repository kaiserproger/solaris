use super::campfire::campfire_cooking_states_from_chunk;
use super::session::{
    PreparedChunkClaim, PreparedChunkClaimResult, SessionPreparedChunkClaimResult,
};
use super::*;
use mc_world::light::compute_chunk_light_in;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct ChunkBuildTiming {
    chunk_data_ms: u64,
    heightmap_ms: u64,
    light_compute_ms: u64,
    light_encode_ms: u64,
}

impl ChunkBuildTiming {
    fn add(&mut self, other: ChunkBuildTiming) {
        self.chunk_data_ms += other.chunk_data_ms;
        self.heightmap_ms += other.heightmap_ms;
        self.light_compute_ms += other.light_compute_ms;
        self.light_encode_ms += other.light_encode_ms;
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct ChunkWriteTiming {
    packet_encode_ms: u64,
    frame_ms: u64,
    socket_write_ms: u64,
    framed_bytes: usize,
}

#[derive(Debug, Default, Clone, Copy)]
struct PressureFlushTiming {
    runs: usize,
    planned_chunks: usize,
    flushed_chunks: usize,
    plan_ms: u64,
    write_ms: u64,
    commit_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChunkStreamStep {
    Progress,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChunkPrepareBudgetClass {
    Load,
    Generate,
}

impl ChunkPrepareBudgetClass {
    fn stop_reason(self) -> ChunkPipelineStopReason {
        match self {
            Self::Load => ChunkPipelineStopReason::LoadBudget,
            Self::Generate => ChunkPipelineStopReason::GenerateBudget,
        }
    }
}

const INITIAL_CHUNK_MIN_RING: i32 = 2;
const CHUNK_STAGE_SLOW_MS: u64 = 50;
const CHUNK_BACKPRESSURE_MAX_RETRIES: usize = 16;
const PREPARED_IN_FLIGHT_DEFERRAL_LIMIT: usize = 2;
const PRESSURE_FLUSH_STALE_REGION_RETRIES: usize = 3;
const PREWARM_EDGE_RING_LIMIT: usize = 40;
const PREWARM_PREPARED_CACHE_LIMIT: usize = 64;
static PRESSURE_FLUSH_COORDINATOR: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
pub(super) struct ChunkStreamState {
    world: WorldHandle,
    world_read: Option<mc_world::WorldReadView>,
    world_mutation: Option<mc_world::WorldMutationView>,
    chunk_source: Option<mc_world::ChunkSourceView>,
    biomes: Arc<Registry>,
    blocks: Arc<BlockRegistry>,
    block_light: Option<Arc<BlockLightTable>>,
    items: Arc<ItemRegistry>,
    tags: Arc<TagsData>,
    recipes: Arc<Vec<mc_data::recipes::Recipe>>,
    block_entity_types: Arc<mc_data::block_entity_types::BlockEntityTypeRegistry>,
    passive_herd_surface: Option<mc_world::BlockStateId>,
    passive_herd_fallback_surfaces: Arc<Vec<mc_world::BlockStateId>>,
    passive_herd_water: Arc<Vec<mc_world::BlockStateId>>,
    passive_herd_passable: Arc<Vec<BlockStateId>>,
    passive_spawn_rules: Arc<mc_data::biomes::BiomeSpawnRules>,
    entity_types: Arc<mc_data::entity_types::EntityTypeRegistry>,
    spawn_monsters: bool,
    compression: Compression,
    sessions: Arc<SessionRegistry>,
    simulation: Option<SimulationHandle>,
    session_id: SessionId,
    resources: ChunkPipelineResources,
    active_generation: Arc<AtomicU64>,
    result_tx: mpsc::Sender<ChunkPrepareResult>,
    result_rx: mpsc::Receiver<ChunkPrepareResult>,
    progress_notify: Arc<tokio::sync::Notify>,
    ready: BTreeMap<u32, ChunkPrepareResult>,
    prewarm_in_flight: HashSet<(i32, i32)>,
    pressure_retries: HashMap<(i32, i32), usize>,
    policy: ChunkPipelinePolicy,
    configured_prepare_batch_size: usize,
    prepare_limit_stop_reason: ChunkPipelineStopReason,
    runtime_control: Option<crate::RuntimeControlHandle>,
    chunk_pressure_source: Option<crate::control_plane::RuntimeControlChunkPressureSource>,
    first_chunk_sla_source: Option<crate::control_plane::RuntimeControlFirstChunkSlaSource>,
    first_chunk_sla_target_ms: u64,
    first_chunk_sla_active: bool,
    result_queue_size: usize,
    chunk_queue_saturated: bool,
    center_cx: i32,
    center_cz: i32,
    direction_yaw: f32,
    view_distance: i32,
    client_view_distance_cap: i32,
    runtime_view_distance_limit: i32,
    scheduler: ChunkScheduler,
    staged: HashSet<(i32, i32)>,
    loaded: HashSet<(i32, i32)>,
    started: Instant,
    fetch_ms: u64,
    build_timing: ChunkBuildTiming,
    packet_encode_ms: u64,
    frame_ms: u64,
    socket_write_ms: u64,
    max_fetch_ms: u64,
    max_chunk_data_ms: u64,
    max_heightmap_ms: u64,
    max_light_compute_ms: u64,
    max_light_compute_chunk: Option<(i32, i32)>,
    max_light_compute_revision: Option<u64>,
    max_light_encode_ms: u64,
    max_packet_encode_ms: u64,
    max_frame_ms: u64,
    max_socket_write_ms: u64,
    slow_fetch_chunks: usize,
    slow_light_compute_chunks: usize,
    slow_packet_encode_chunks: usize,
    slow_frame_chunks: usize,
    slow_socket_write_chunks: usize,
    framed_bytes: usize,
    first_chunk_ms: Option<u64>,
    ring1_complete_ms: Option<u64>,
    ring2_complete_ms: Option<u64>,
    ring_emitted: Vec<usize>,
    emitted: usize,
    absent: usize,
    pressure_abandoned: usize,
    pressure_staged_by_chunk: HashMap<(i32, i32), HashSet<(i32, i32)>>,
    pressure_flush_runs: usize,
    pressure_flush_planned_chunks: usize,
    pressure_flush_flushed_chunks: usize,
    pressure_flush_plan_ms: u64,
    pressure_flush_write_ms: u64,
    pressure_flush_commit_ms: u64,
    max_pressure_flush_plan_ms: u64,
    max_pressure_flush_write_ms: u64,
    max_pressure_flush_commit_ms: u64,
    memory_pressure_shed_runs: usize,
    memory_pressure_shed_ready: usize,
    memory_pressure_shed_in_flight: usize,
    memory_pressure_active: bool,
    bytes: usize,
    dispatch_turns: usize,
    yielded_turns: usize,
    dispatched: usize,
    prewarm_dispatched: usize,
    max_in_flight: usize,
    max_ready: usize,
    last_stop_reason: ChunkPipelineStopReason,
    wait_for_first_chunk: bool,
    summary_logged: bool,
}

#[derive(Debug, Clone)]
pub(super) struct PreparedChunkFrame {
    pub(super) frame: Bytes,
    pub(super) light: Option<ChunkLight>,
    pub(super) herd_spawns: Vec<HerdSpawn>,
    pub(super) hydrated_campfires: Vec<(mc_world::BlockPos, CampfireCookingState)>,
    pub(super) packet_data_len: usize,
    pub(super) build_timing: ChunkBuildTiming,
    pub(super) write_timing: ChunkWriteTiming,
}

#[derive(Clone, Copy)]
struct LandSpawnSurfaces<'a> {
    preferred: mc_world::BlockStateId,
    fallbacks: &'a [mc_world::BlockStateId],
}

impl PreparedChunkFrame {
    fn prepared_cache_hit(&self) -> Self {
        let mut cached = self.clone();
        let framed_bytes = cached.write_timing.framed_bytes;
        cached.build_timing = ChunkBuildTiming::default();
        cached.write_timing = ChunkWriteTiming::default();
        cached.write_timing.framed_bytes = framed_bytes;
        cached
    }
}

enum ChunkPrepareOutcome {
    Ready(Box<PreparedChunkFrame>),
    Absent,
    Backpressured,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmitReadyResult {
    SentPacket,
    DrainedNoPacket,
    Blocked,
    Empty,
}

#[derive(Debug, Clone, Copy)]
enum PreparedChunkFence {
    Claimed(PreparedChunkClaim),
    CachedRevision(u64),
}

impl PreparedChunkFence {
    fn revision(self) -> u64 {
        match self {
            Self::Claimed(claim) => claim.revision,
            Self::CachedRevision(revision) => revision,
        }
    }
}

struct PreparedChunkClaimLease {
    sessions: Arc<SessionRegistry>,
    chunk: (i32, i32),
    claim: Option<PreparedChunkClaim>,
}

impl PreparedChunkClaimLease {
    fn new(sessions: Arc<SessionRegistry>, chunk: (i32, i32), claim: PreparedChunkClaim) -> Self {
        Self {
            sessions,
            chunk,
            claim: Some(claim),
        }
    }

    fn claim(&self) -> PreparedChunkClaim {
        self.claim.expect("prepared claim lease is armed")
    }

    fn disarm(&mut self) {
        self.claim = None;
    }
}

impl Drop for PreparedChunkClaimLease {
    fn drop(&mut self) {
        if let Some(claim) = self.claim.take() {
            self.sessions
                .release_prepared_chunk_claim(self.chunk, claim);
        }
    }
}

struct ChunkPrepareResult {
    request: crate::ChunkRequest,
    prepare_claim: Option<PreparedChunkFence>,
    fetch_ms: u64,
    pressure_flush: PressureFlushTiming,
    staged: Vec<(i32, i32)>,
    outcome: ChunkPrepareOutcome,
}

pub(super) fn passive_chunk_spawns(chunk: (i32, i32)) -> bool {
    if chunk == (0, 0) {
        return true;
    }
    let h = herd_hash(chunk, 0, 0x4845_5244);
    h.is_multiple_of(9)
}

pub(super) fn hostile_chunk_spawns(chunk: (i32, i32)) -> bool {
    if chunk == (0, 0) {
        return true;
    }
    let h = herd_hash(chunk, 0, 0x484F_5354_494C_4500);
    h.is_multiple_of(8)
}

fn herd_hash(chunk: (i32, i32), slot: u8, salt: u64) -> u64 {
    let mut h = salt;
    h ^= (chunk.0 as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h = h.rotate_left(23);
    h ^= (chunk.1 as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h = h.rotate_left(17);
    h ^= (slot as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h.wrapping_mul(0x94D0_49BB_1331_11EB) ^ (h >> 31)
}

fn natural_sheep_color(
    climate: mc_data::biomes::SheepColorClimate,
    chunk: (i32, i32),
    slot: u8,
) -> mc_entity::SheepColor {
    let outer_roll = (herd_hash(chunk, slot, 0x5348_4545_505F_434C) % 100) as u32;
    let common_roll = (herd_hash(chunk, slot, 0x5049_4E4B_5F52_4F4C) % 500) as u32;
    sheep_color_for_rolls(climate, outer_roll, common_roll)
}

fn sheep_color_for_rolls(
    climate: mc_data::biomes::SheepColorClimate,
    outer_roll: u32,
    common_roll: u32,
) -> mc_entity::SheepColor {
    use mc_data::biomes::SheepColorClimate;
    use mc_entity::SheepColor;

    debug_assert!(outer_roll < 100);
    debug_assert!(common_roll < 500);
    let common = |default| {
        if common_roll < 499 {
            default
        } else {
            SheepColor::Pink
        }
    };
    match climate {
        SheepColorClimate::Temperate => match outer_roll {
            0..=4 => SheepColor::Black,
            5..=9 => SheepColor::Gray,
            10..=14 => SheepColor::LightGray,
            15..=17 => SheepColor::Brown,
            _ => common(SheepColor::White),
        },
        SheepColorClimate::Warm => match outer_roll {
            0..=4 => SheepColor::Gray,
            5..=9 => SheepColor::LightGray,
            10..=14 => SheepColor::White,
            15..=17 => SheepColor::Black,
            _ => common(SheepColor::Brown),
        },
        SheepColorClimate::Cold => match outer_roll {
            0..=4 => SheepColor::LightGray,
            5..=9 => SheepColor::Gray,
            10..=14 => SheepColor::White,
            15..=17 => SheepColor::Brown,
            _ => common(SheepColor::Black),
        },
    }
}

pub(super) fn herd_uuid(chunk: (i32, i32), slot: u8) -> uuid::Uuid {
    let hi = herd_hash(chunk, slot, 0x434F_575F_4845_5244);
    let lo = herd_hash(chunk, slot, 0x5041_5353_4956_4500);
    uuid::Uuid::from_u128(((hi as u128) << 64) | lo as u128)
}

pub(super) fn plan_passive_herd(
    chunk: &Chunk,
    land_surface: Option<mc_world::BlockStateId>,
    land_fallback_surfaces: &[mc_world::BlockStateId],
    water: Option<&[mc_world::BlockStateId]>,
    passable: &[BlockStateId],
    rules: &mc_data::biomes::BiomeSpawnRules,
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
) -> Vec<HerdSpawn> {
    let chunk_pos = (chunk.pos.x, chunk.pos.z);
    let mut spawns = Vec::new();
    if let Some(surface) = land_surface {
        let surfaces = LandSpawnSurfaces {
            preferred: surface,
            fallbacks: land_fallback_surfaces,
        };
        if passive_chunk_spawns(chunk_pos) {
            plan_group_spawns(
                chunk,
                surfaces,
                passable,
                "creature",
                rules,
                entity_types,
                &mut spawns,
            );
        }
        plan_hostile_spawns(chunk, surfaces, passable, rules, entity_types, &mut spawns);
    }
    if let Some(water) = water.filter(|states| !states.is_empty()) {
        plan_water_group_spawns(
            chunk,
            water,
            "water_ambient",
            rules,
            entity_types,
            &mut spawns,
        );
        plan_water_group_spawns(
            chunk,
            water,
            "water_creature",
            rules,
            entity_types,
            &mut spawns,
        );
    }
    spawns
}

fn plan_hostile_spawns(
    chunk: &Chunk,
    surfaces: LandSpawnSurfaces<'_>,
    passable: &[BlockStateId],
    rules: &mc_data::biomes::BiomeSpawnRules,
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
    out: &mut Vec<HerdSpawn>,
) {
    let chunk_pos = (chunk.pos.x, chunk.pos.z);
    if !hostile_chunk_spawns(chunk_pos) {
        return;
    }
    let slot_base = out.len() as u8;
    let h = herd_hash(chunk_pos, slot_base, 0x5A4F_4D42_4945_0000);
    let Some((lx, y, lz)) = herd_spawn_surface(chunk, surfaces, passable, h) else {
        return;
    };
    let Some(biome) = chunk_biome_at(chunk, lx, y, lz) else {
        return;
    };
    for (hostile_index, entry) in rules
        .entries(biome, "monster")
        .iter()
        .filter(|entry| entity_type_is_hostile(entity_types, &entry.entity_type))
        .take(3)
        .enumerate()
    {
        let Some(entity_type_id) = entity_types
            .id_of(&entry.entity_type)
            .and_then(|id| i32::try_from(id).ok())
        else {
            continue;
        };
        let slot = slot_base + hostile_index as u8;
        let offset = herd_hash(chunk_pos, slot, 0x484F_5354_494C_4500);
        out.push(HerdSpawn {
            chunk: chunk_pos,
            slot,
            entity_type_id,
            entity_type_name: entry.entity_type.as_str().to_string(),
            position: Vec3::new(
                f64::from(chunk.pos.x * 16 + i32::from(lx)) + safe_land_spawn_offset(offset),
                f64::from(y + 1),
                f64::from(chunk.pos.z * 16 + i32::from(lz)) + safe_land_spawn_offset(offset >> 2),
            ),
            hostile: true,
            sheep_color: None,
        });
    }
}

fn entity_type_is_hostile(
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
    entity_type: &Identifier,
) -> bool {
    entity_types
        .facts_of(entity_type)
        .is_some_and(|facts| facts.category.is_hostile())
}

fn plan_group_spawns(
    chunk: &Chunk,
    surfaces: LandSpawnSurfaces<'_>,
    passable: &[BlockStateId],
    group: &str,
    rules: &mc_data::biomes::BiomeSpawnRules,
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
    out: &mut Vec<HerdSpawn>,
) {
    let chunk_pos = (chunk.pos.x, chunk.pos.z);
    let slot_base = out.len() as u8;
    let h = herd_hash(chunk_pos, slot_base, 0x5350_4157_4E00_0000);
    let Some((lx, y, lz)) = herd_spawn_surface(chunk, surfaces, passable, h) else {
        return;
    };
    let Some(biome) = chunk_biome_at(chunk, lx, y, lz) else {
        return;
    };
    let Some(entry) = choose_biome_spawn(rules.entries(biome, group), chunk_pos, slot_base) else {
        return;
    };
    let Some(entity_type_id) = entity_types
        .id_of(&entry.entity_type)
        .and_then(|id| i32::try_from(id).ok())
    else {
        return;
    };
    let count = herd_entry_count(entry, chunk_pos, slot_base).min(6);
    for i in 0..count {
        let slot = slot_base + i as u8;
        let offset = herd_hash(chunk_pos, slot, 0x4F46_4653_4554_0000);
        out.push(HerdSpawn {
            chunk: chunk_pos,
            slot,
            entity_type_id,
            entity_type_name: entry.entity_type.as_str().to_string(),
            position: Vec3::new(
                f64::from(chunk.pos.x * 16 + i32::from(lx)) + safe_land_spawn_offset(offset),
                f64::from(y + 1),
                f64::from(chunk.pos.z * 16 + i32::from(lz)) + safe_land_spawn_offset(offset >> 2),
            ),
            hostile: false,
            sheep_color: (entry.entity_type.as_str() == "minecraft:sheep")
                .then(|| natural_sheep_color(rules.sheep_color_climate(biome), chunk_pos, slot)),
        });
    }
}

fn plan_water_group_spawns(
    chunk: &Chunk,
    water: &[mc_world::BlockStateId],
    group: &str,
    rules: &mc_data::biomes::BiomeSpawnRules,
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
    out: &mut Vec<HerdSpawn>,
) {
    let chunk_pos = (chunk.pos.x, chunk.pos.z);
    let slot_base = out.len() as u8;
    let h = herd_hash(chunk_pos, slot_base, 0x5741_5445_5200_0000);
    let lx = 3 + (h as u8 % 10);
    let lz = 3 + ((h >> 8) as u8 % 10);
    let Some(spawn_y) = water_spawn_y(chunk, lx, lz, water) else {
        return;
    };
    let Some(biome) = chunk_biome_at(chunk, lx, spawn_y, lz) else {
        return;
    };
    let Some(entry) = choose_biome_spawn(rules.entries(biome, group), chunk_pos, slot_base) else {
        return;
    };
    let Some(entity_type_id) = entity_types
        .id_of(&entry.entity_type)
        .and_then(|id| i32::try_from(id).ok())
    else {
        return;
    };
    let count = herd_entry_count(entry, chunk_pos, slot_base).min(6);
    for i in 0..count {
        let slot = slot_base + i as u8;
        out.push(HerdSpawn {
            chunk: chunk_pos,
            slot,
            entity_type_id,
            entity_type_name: entry.entity_type.as_str().to_string(),
            position: Vec3::new(
                f64::from(chunk.pos.x * 16 + i32::from(lx)) + 0.5,
                f64::from(spawn_y),
                f64::from(chunk.pos.z * 16 + i32::from(lz)) + 0.5,
            ),
            hostile: false,
            sheep_color: None,
        });
    }
}

fn water_spawn_y(chunk: &Chunk, lx: u8, lz: u8, water: &[mc_world::BlockStateId]) -> Option<i32> {
    let mut best_run = None;
    let mut current_start = None;
    for y in mc_world::MIN_Y..=DEFAULT_SEA_LEVEL {
        if chunk
            .get_block(lx, y, lz)
            .is_some_and(|state| water.contains(&state))
        {
            current_start.get_or_insert(y);
            continue;
        }
        if let Some(start) = current_start.take() {
            remember_water_run(&mut best_run, start, y - 1);
        }
    }
    if let Some(start) = current_start.take() {
        remember_water_run(&mut best_run, start, DEFAULT_SEA_LEVEL);
    }

    best_run.map(|(start, end)| start + (end - start) / 2)
}

fn remember_water_run(best_run: &mut Option<(i32, i32)>, start: i32, end: i32) {
    let len = end - start;
    if best_run
        .map(|(best_start, best_end)| len > best_end - best_start)
        .unwrap_or(true)
    {
        *best_run = Some((start, end));
    }
}

fn choose_biome_spawn(
    entries: &[mc_data::biomes::BiomeSpawnEntry],
    chunk: (i32, i32),
    slot: u8,
) -> Option<&mc_data::biomes::BiomeSpawnEntry> {
    let total: u32 = entries.iter().map(|entry| entry.weight).sum();
    if total == 0 {
        return None;
    }
    let mut pick = (herd_hash(chunk, slot, 0x5745_4947_4854_0000) % u64::from(total)) as u32;
    for entry in entries {
        if pick < entry.weight {
            return Some(entry);
        }
        pick -= entry.weight;
    }
    entries.last()
}

fn herd_entry_count(
    entry: &mc_data::biomes::BiomeSpawnEntry,
    chunk: (i32, i32),
    slot: u8,
) -> usize {
    let min = entry.min_count.min(entry.max_count).max(1);
    let max = entry.max_count.max(min);
    let span = max - min + 1;
    (min + (herd_hash(chunk, slot, 0x434F_554E_5400_0000) as u32 % span)) as usize
}

fn chunk_biome_at(chunk: &Chunk, lx: u8, y: i32, lz: u8) -> Option<&mc_data::Identifier> {
    let geometry = chunk.geometry();
    if !(geometry.min_y()..geometry.max_y()).contains(&y) {
        return None;
    }
    let chunk_y = (y - geometry.min_y()) as usize;
    let section = chunk.biomes.get(chunk_y / mc_world::SECTION_DIM)?;
    let local_y = (chunk_y % mc_world::SECTION_DIM) as u8 / mc_world::BIOME_DIM as u8;
    Some(section.get(lx / 4, local_y, lz / 4))
}

fn herd_surface_y(
    chunk: &Chunk,
    lx: u8,
    lz: u8,
    surface: mc_world::BlockStateId,
    fallback_surfaces: &[BlockStateId],
    passable: &[BlockStateId],
) -> Option<(i32, BlockStateId)> {
    if let Some(y) = chunk.highest_opaque_y(lx, lz)
        && chunk.get_block(lx, y, lz) == Some(surface)
    {
        return Some((y, surface));
    }
    if let Some(y) = (mc_world::MIN_Y..mc_world::MAX_Y)
        .rev()
        .find(|&y| chunk.get_block(lx, y, lz) == Some(surface))
    {
        return Some((y, surface));
    }
    herd_land_surface_y(chunk, lx, lz, fallback_surfaces, passable)
}

fn herd_spawn_surface(
    chunk: &Chunk,
    surfaces: LandSpawnSurfaces<'_>,
    passable: &[BlockStateId],
    h: u64,
) -> Option<(u8, i32, u8)> {
    for attempt in 0..100u64 {
        let candidate = h.wrapping_add(attempt.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let lx = 3 + (candidate as u8 % 10);
        let lz = 3 + ((candidate >> 8) as u8 % 10);
        let Some((y, actual_surface)) = herd_surface_y(
            chunk,
            lx,
            lz,
            surfaces.preferred,
            surfaces.fallbacks,
            passable,
        ) else {
            continue;
        };
        if herd_spawn_clearance(chunk, lx, y + 1, lz, actual_surface, passable) {
            return Some((lx, y, lz));
        }
    }
    for attempt in 0..100u64 {
        let candidate = h.wrapping_add(attempt.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let lx = 3 + (candidate as u8 % 10);
        let lz = 3 + ((candidate >> 8) as u8 % 10);
        let Some((y, actual_surface)) = herd_surface_y(
            chunk,
            lx,
            lz,
            surfaces.preferred,
            surfaces.fallbacks,
            passable,
        ) else {
            continue;
        };
        if herd_spawn_minimal_clearance(chunk, lx, y + 1, lz, actual_surface, passable) {
            return Some((lx, y, lz));
        }
    }
    None
}

fn herd_land_surface_y(
    chunk: &Chunk,
    lx: u8,
    lz: u8,
    fallback_surfaces: &[BlockStateId],
    passable: &[BlockStateId],
) -> Option<(i32, BlockStateId)> {
    let y = chunk.highest_opaque_y(lx, lz)?;
    let state = chunk.get_block(lx, y, lz)?;
    if passable.contains(&state) || !fallback_surfaces.contains(&state) {
        return None;
    }
    if (y + 1..=y + 2).all(|air_y| {
        chunk
            .get_block(lx, air_y, lz)
            .is_some_and(|state| passable.contains(&state))
    }) {
        Some((y, state))
    } else {
        None
    }
}

fn safe_land_spawn_offset(bits: u64) -> f64 {
    0.48 + (bits & 3) as f64 * 0.01
}

fn herd_spawn_clearance(
    chunk: &Chunk,
    lx: u8,
    spawn_y: i32,
    lz: u8,
    surface: BlockStateId,
    passable: &[BlockStateId],
) -> bool {
    for dx in -1..=1 {
        for dz in -1..=1 {
            let x = i32::from(lx) + dx;
            let z = i32::from(lz) + dz;
            if !(0..mc_world::SECTION_DIM as i32).contains(&x)
                || !(0..mc_world::SECTION_DIM as i32).contains(&z)
            {
                return false;
            }
            let x = x as u8;
            let z = z as u8;
            if chunk.get_block(x, spawn_y - 1, z) != Some(surface) {
                return false;
            }
            if !(spawn_y..=spawn_y + 1).all(|y| {
                chunk
                    .get_block(x, y, z)
                    .is_some_and(|state| passable.contains(&state))
            }) {
                return false;
            }
        }
    }
    true
}

fn herd_spawn_minimal_clearance(
    chunk: &Chunk,
    lx: u8,
    spawn_y: i32,
    lz: u8,
    surface: BlockStateId,
    passable: &[BlockStateId],
) -> bool {
    if chunk.get_block(lx, spawn_y - 1, lz) != Some(surface) {
        return false;
    }
    if !(spawn_y..=spawn_y + 1).all(|y| {
        chunk
            .get_block(lx, y, lz)
            .is_some_and(|state| passable.contains(&state))
    }) {
        return false;
    }
    [(1, 0), (-1, 0), (0, 1), (0, -1)]
        .into_iter()
        .any(|(dx, dz)| {
            let x = i32::from(lx) + dx;
            let z = i32::from(lz) + dz;
            (0..mc_world::SECTION_DIM as i32).contains(&x)
                && (0..mc_world::SECTION_DIM as i32).contains(&z)
                && chunk.get_block(x as u8, spawn_y - 1, z as u8) == Some(surface)
                && (spawn_y..=spawn_y + 1).all(|y| {
                    chunk
                        .get_block(x as u8, y, z as u8)
                        .is_some_and(|state| passable.contains(&state))
                })
        })
}

pub(crate) fn passive_entity_passable_blocks(blocks: &BlockRegistry) -> Vec<BlockStateId> {
    blocks
        .states()
        .filter(|state| passable_block_name(state.block.id.as_str()))
        .map(|state| state.id)
        .collect()
}

pub(crate) fn passive_herd_fallback_surface_blocks(blocks: &BlockRegistry) -> Vec<BlockStateId> {
    blocks
        .states()
        .filter(|state| passive_herd_fallback_surface_name(state.block.id.as_str()))
        .map(|state| state.id)
        .collect()
}

fn passive_herd_fallback_surface_name(name: &str) -> bool {
    matches!(
        name,
        "minecraft:dirt"
            | "minecraft:coarse_dirt"
            | "minecraft:podzol"
            | "minecraft:sand"
            | "minecraft:red_sand"
            | "minecraft:snow_block"
            | "minecraft:moss_block"
            | "minecraft:mycelium"
    )
}

pub(crate) fn passable_block_name(name: &str) -> bool {
    matches!(
        name,
        "minecraft:air"
            | "minecraft:short_grass"
            | "minecraft:tall_grass"
            | "minecraft:short_dry_grass"
            | "minecraft:tall_dry_grass"
            | "minecraft:fern"
            | "minecraft:large_fern"
            | "minecraft:dead_bush"
            | "minecraft:bush"
            | "minecraft:firefly_bush"
            | "minecraft:dandelion"
            | "minecraft:poppy"
            | "minecraft:blue_orchid"
            | "minecraft:allium"
            | "minecraft:azure_bluet"
            | "minecraft:red_tulip"
            | "minecraft:orange_tulip"
            | "minecraft:white_tulip"
            | "minecraft:pink_tulip"
            | "minecraft:oxeye_daisy"
            | "minecraft:cornflower"
            | "minecraft:lily_of_the_valley"
            | "minecraft:wither_rose"
            | "minecraft:torchflower"
            | "minecraft:open_eyeblossom"
            | "minecraft:closed_eyeblossom"
            | "minecraft:sunflower"
            | "minecraft:lilac"
            | "minecraft:rose_bush"
            | "minecraft:peony"
            | "minecraft:pitcher_plant"
            | "minecraft:pink_petals"
            | "minecraft:wildflowers"
            | "minecraft:sugar_cane"
            | "minecraft:wheat"
            | "minecraft:carrots"
            | "minecraft:potatoes"
            | "minecraft:beetroots"
            | "minecraft:torchflower_crop"
            | "minecraft:pitcher_crop"
            | "minecraft:melon_stem"
            | "minecraft:attached_melon_stem"
            | "minecraft:pumpkin_stem"
            | "minecraft:attached_pumpkin_stem"
            | "minecraft:sweet_berry_bush"
            | "minecraft:nether_wart"
            | "minecraft:kelp"
            | "minecraft:kelp_plant"
            | "minecraft:seagrass"
            | "minecraft:tall_seagrass"
            | "minecraft:bubble_column"
    )
}

pub(super) fn desired_chunk_set(
    center_cx: i32,
    center_cz: i32,
    view_distance: i32,
) -> HashSet<(i32, i32)> {
    spiral_chunks(center_cx, center_cz, view_distance).collect()
}

impl ChunkStreamState {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        world: WorldHandle,
        biomes: Arc<Registry>,
        blocks: Arc<BlockRegistry>,
        block_light: Option<Arc<BlockLightTable>>,
        items: Arc<ItemRegistry>,
        tags: Arc<TagsData>,
        recipes: Arc<Vec<mc_data::recipes::Recipe>>,
        block_entity_types: Arc<mc_data::block_entity_types::BlockEntityTypeRegistry>,
        passive_herd_surface: Option<mc_world::BlockStateId>,
        passive_herd_fallback_surfaces: Arc<Vec<mc_world::BlockStateId>>,
        passive_herd_water: Arc<Vec<mc_world::BlockStateId>>,
        passive_herd_passable: Arc<Vec<BlockStateId>>,
        passive_spawn_rules: Arc<mc_data::biomes::BiomeSpawnRules>,
        entity_types: Arc<mc_data::entity_types::EntityTypeRegistry>,
        compression: Compression,
        sessions: Arc<SessionRegistry>,
        session_id: SessionId,
        center_cx: i32,
        center_cz: i32,
        direction_yaw: f32,
        view_distance: i32,
        resources: ChunkPipelineResources,
        policy: ChunkPipelinePolicy,
    ) -> Self {
        let vd = view_distance.max(0);
        let (result_tx, result_rx) = mpsc::channel(policy.chunk_result_queue_size);
        let progress_notify = Arc::new(tokio::sync::Notify::new());
        let scheduler = ChunkScheduler::new(prioritized_spiral(
            center_cx,
            center_cz,
            view_distance,
            direction_yaw,
        ));
        let active_generation = Arc::new(AtomicU64::new(scheduler.current_generation().0));
        Self {
            world,
            world_read: None,
            world_mutation: None,
            chunk_source: None,
            biomes,
            blocks,
            block_light,
            items,
            tags,
            recipes,
            block_entity_types,
            passive_herd_surface,
            passive_herd_fallback_surfaces,
            passive_herd_water,
            passive_herd_passable,
            passive_spawn_rules,
            entity_types,
            spawn_monsters: true,
            compression,
            sessions,
            simulation: None,
            session_id,
            resources,
            active_generation,
            result_tx,
            result_rx,
            progress_notify,
            ready: BTreeMap::new(),
            prewarm_in_flight: HashSet::new(),
            pressure_retries: HashMap::new(),
            policy,
            configured_prepare_batch_size: policy.chunk_prepare_batch_size.max(1),
            prepare_limit_stop_reason: ChunkPipelineStopReason::BatchLimit,
            runtime_control: None,
            chunk_pressure_source: None,
            first_chunk_sla_source: None,
            first_chunk_sla_target_ms: u64::MAX,
            first_chunk_sla_active: false,
            result_queue_size: policy.chunk_result_queue_size,
            chunk_queue_saturated: false,
            center_cx,
            center_cz,
            direction_yaw,
            view_distance: vd,
            client_view_distance_cap: vd,
            runtime_view_distance_limit: vd,
            scheduler,
            staged: HashSet::new(),
            loaded: HashSet::new(),
            started: Instant::now(),
            fetch_ms: 0,
            build_timing: ChunkBuildTiming::default(),
            packet_encode_ms: 0,
            frame_ms: 0,
            socket_write_ms: 0,
            max_fetch_ms: 0,
            max_chunk_data_ms: 0,
            max_heightmap_ms: 0,
            max_light_compute_ms: 0,
            max_light_compute_chunk: None,
            max_light_compute_revision: None,
            max_light_encode_ms: 0,
            max_packet_encode_ms: 0,
            max_frame_ms: 0,
            max_socket_write_ms: 0,
            slow_fetch_chunks: 0,
            slow_light_compute_chunks: 0,
            slow_packet_encode_chunks: 0,
            slow_frame_chunks: 0,
            slow_socket_write_chunks: 0,
            framed_bytes: 0,
            first_chunk_ms: None,
            ring1_complete_ms: None,
            ring2_complete_ms: None,
            ring_emitted: vec![0; (vd + 1) as usize],
            emitted: 0,
            absent: 0,
            pressure_abandoned: 0,
            pressure_staged_by_chunk: HashMap::new(),
            pressure_flush_runs: 0,
            pressure_flush_planned_chunks: 0,
            pressure_flush_flushed_chunks: 0,
            pressure_flush_plan_ms: 0,
            pressure_flush_write_ms: 0,
            pressure_flush_commit_ms: 0,
            max_pressure_flush_plan_ms: 0,
            max_pressure_flush_write_ms: 0,
            max_pressure_flush_commit_ms: 0,
            memory_pressure_shed_runs: 0,
            memory_pressure_shed_ready: 0,
            memory_pressure_shed_in_flight: 0,
            memory_pressure_active: false,
            bytes: 0,
            dispatch_turns: 0,
            yielded_turns: 0,
            dispatched: 0,
            prewarm_dispatched: 0,
            max_in_flight: 0,
            max_ready: 0,
            last_stop_reason: ChunkPipelineStopReason::QueueEmpty,
            wait_for_first_chunk: true,
            summary_logged: false,
        }
    }

    pub(super) fn with_world_read(mut self, world_read: Option<mc_world::WorldReadView>) -> Self {
        self.world_read = world_read;
        self
    }

    pub(super) fn with_spawn_monsters(mut self, spawn_monsters: bool) -> Self {
        self.spawn_monsters = spawn_monsters;
        self
    }

    pub(super) fn with_world_mutation(
        mut self,
        world_mutation: Option<mc_world::WorldMutationView>,
    ) -> Self {
        self.world_mutation = world_mutation;
        self
    }

    pub(super) fn with_chunk_source(
        mut self,
        chunk_source: Option<mc_world::ChunkSourceView>,
    ) -> Self {
        self.chunk_source = chunk_source;
        self
    }

    pub(super) fn with_simulation(mut self, simulation: SimulationHandle) -> Self {
        self.simulation = Some(simulation);
        self
    }

    pub(super) fn with_runtime_control(
        mut self,
        runtime_control: Option<crate::RuntimeControlHandle>,
    ) -> Self {
        if let Some(control) = runtime_control.as_ref() {
            self.first_chunk_sla_target_ms = control.snapshot().policy.target_first_chunk_ms;
            self.chunk_pressure_source = Some(control.chunk_pressure_source());
            self.first_chunk_sla_source = Some(control.first_chunk_sla_source());
        }
        self.runtime_control = runtime_control;
        self
    }

    pub(super) fn is_complete(&self) -> bool {
        self.scheduler.is_complete()
    }

    pub(super) fn progress_notify(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.progress_notify)
    }

    pub(super) fn has_immediate_work(&self) -> bool {
        !self.ready.is_empty()
            || !self.result_rx.is_empty()
            || (self.scheduler.queued_len() > 0
                && matches!(
                    self.last_stop_reason,
                    ChunkPipelineStopReason::BatchLimit
                        | ChunkPipelineStopReason::TimeBudget
                        | ChunkPipelineStopReason::SendBudget
                        | ChunkPipelineStopReason::LoadBudget
                        | ChunkPipelineStopReason::GenerateBudget
                ))
    }

    pub(super) fn replan_center(
        &mut self,
        center_cx: i32,
        center_cz: i32,
        direction_yaw: f32,
    ) -> Vec<(i32, i32)> {
        if (self.center_cx, self.center_cz) == (center_cx, center_cz) {
            if (self.direction_yaw - direction_yaw).abs() >= 22.5 && !self.scheduler.is_complete() {
                self.scheduler.reprioritize_queued(prioritized_spiral(
                    center_cx,
                    center_cz,
                    self.view_distance,
                    direction_yaw,
                ));
            }
            self.direction_yaw = direction_yaw;
            return Vec::new();
        }
        let desired = desired_chunk_set(center_cx, center_cz, self.view_distance);
        let unloads: Vec<_> = self.loaded.difference(&desired).copied().collect();
        for chunk in &unloads {
            self.loaded.remove(chunk);
        }
        let mut visibility = self.sessions.replace_view(
            self.session_id,
            (center_cx, center_cz),
            self.view_distance,
            desired,
        );
        visibility.extend(self.sessions.mark_unloaded(self.session_id, &unloads));
        dispatch_visibility_commands(visibility);
        self.center_cx = center_cx;
        self.center_cz = center_cz;
        self.direction_yaw = direction_yaw;
        self.clear_ready();
        self.reset_pressure_tracking();
        self.reset_prewarm_tracking();
        self.scheduler.replace_view(prioritized_spiral(
            center_cx,
            center_cz,
            self.view_distance,
            self.direction_yaw,
        ));
        self.active_generation
            .store(self.scheduler.current_generation().0, Ordering::Release);
        self.reset_window_metrics();
        unloads
    }

    pub(super) fn replan_view_distance(
        &mut self,
        view_distance: i32,
        direction_yaw: f32,
    ) -> Vec<(i32, i32)> {
        self.client_view_distance_cap = view_distance.max(0);
        let effective_view_distance = self
            .runtime_view_distance_limit
            .min(self.client_view_distance_cap);
        self.replan_effective_view_distance(effective_view_distance, direction_yaw)
    }

    fn replan_effective_view_distance(
        &mut self,
        view_distance: i32,
        direction_yaw: f32,
    ) -> Vec<(i32, i32)> {
        let view_distance = view_distance.max(0);
        if self.view_distance == view_distance {
            return Vec::new();
        }
        self.view_distance = view_distance;
        self.direction_yaw = direction_yaw;

        let desired = desired_chunk_set(self.center_cx, self.center_cz, self.view_distance);
        let unloads: Vec<_> = self.loaded.difference(&desired).copied().collect();
        for chunk in &unloads {
            self.loaded.remove(chunk);
        }
        let mut visibility = self.sessions.replace_view(
            self.session_id,
            (self.center_cx, self.center_cz),
            self.view_distance,
            desired,
        );
        visibility.extend(self.sessions.mark_unloaded(self.session_id, &unloads));
        dispatch_visibility_commands(visibility);
        self.clear_ready();
        self.reset_pressure_tracking();
        self.reset_prewarm_tracking();
        self.scheduler.replay_view(prioritized_spiral(
            self.center_cx,
            self.center_cz,
            self.view_distance,
            self.direction_yaw,
        ));
        self.active_generation
            .store(self.scheduler.current_generation().0, Ordering::Release);
        self.reset_window_metrics();
        unloads
    }

    pub(super) fn replay_current_view(&mut self, direction_yaw: f32) {
        let loaded: Vec<_> = self.loaded.drain().collect();
        dispatch_visibility_commands(self.sessions.mark_unloaded(self.session_id, &loaded));

        let desired = desired_chunk_set(self.center_cx, self.center_cz, self.view_distance);
        dispatch_visibility_commands(self.sessions.replace_view(
            self.session_id,
            (self.center_cx, self.center_cz),
            self.view_distance,
            desired,
        ));
        self.direction_yaw = direction_yaw;
        self.clear_ready();
        self.reset_pressure_tracking();
        self.reset_prewarm_tracking();
        self.scheduler.replay_view(prioritized_spiral(
            self.center_cx,
            self.center_cz,
            self.view_distance,
            self.direction_yaw,
        ));
        self.active_generation
            .store(self.scheduler.current_generation().0, Ordering::Release);
        self.reset_window_metrics();
    }

    fn reset_window_metrics(&mut self) {
        self.recover_runtime_control_sources();
        self.staged.clear();
        self.started = Instant::now();
        self.fetch_ms = 0;
        self.build_timing = ChunkBuildTiming::default();
        self.packet_encode_ms = 0;
        self.frame_ms = 0;
        self.socket_write_ms = 0;
        self.max_fetch_ms = 0;
        self.max_chunk_data_ms = 0;
        self.max_heightmap_ms = 0;
        self.max_light_compute_ms = 0;
        self.max_light_compute_chunk = None;
        self.max_light_compute_revision = None;
        self.max_light_encode_ms = 0;
        self.max_packet_encode_ms = 0;
        self.max_frame_ms = 0;
        self.max_socket_write_ms = 0;
        self.slow_fetch_chunks = 0;
        self.slow_light_compute_chunks = 0;
        self.slow_packet_encode_chunks = 0;
        self.slow_frame_chunks = 0;
        self.slow_socket_write_chunks = 0;
        self.framed_bytes = 0;
        self.first_chunk_ms = None;
        self.ring1_complete_ms = None;
        self.ring2_complete_ms = None;
        self.ring_emitted = vec![0; (self.view_distance.max(0) + 1) as usize];
        self.emitted = 0;
        self.absent = 0;
        self.pressure_abandoned = 0;
        self.reset_pressure_tracking();
        self.pressure_flush_runs = 0;
        self.pressure_flush_planned_chunks = 0;
        self.pressure_flush_flushed_chunks = 0;
        self.pressure_flush_plan_ms = 0;
        self.pressure_flush_write_ms = 0;
        self.pressure_flush_commit_ms = 0;
        self.max_pressure_flush_plan_ms = 0;
        self.max_pressure_flush_write_ms = 0;
        self.max_pressure_flush_commit_ms = 0;
        self.bytes = 0;
        self.dispatch_turns = 0;
        self.yielded_turns = 0;
        self.dispatched = 0;
        self.prewarm_dispatched = 0;
        self.max_in_flight = 0;
        self.max_ready = 0;
        self.last_stop_reason = ChunkPipelineStopReason::QueueEmpty;
        self.wait_for_first_chunk = false;
        self.summary_logged = false;
    }

    fn set_stop_reason(&mut self, reason: ChunkPipelineStopReason) {
        self.last_stop_reason = reason;
        self.resources.record_stop_reason(reason);
    }

    pub(super) async fn step<W>(
        &mut self,
        writer: &mut W,
        light_cache: &mut LightCache,
    ) -> Result<ChunkStreamStep, ConnectionError>
    where
        W: AsyncWriteExt + Unpin,
    {
        let result = self.step_inner(writer, light_cache).await;
        if result.is_err() {
            self.recover_runtime_control_sources();
        }
        result
    }

    async fn step_inner<W>(
        &mut self,
        writer: &mut W,
        light_cache: &mut LightCache,
    ) -> Result<ChunkStreamStep, ConnectionError>
    where
        W: AsyncWriteExt + Unpin,
    {
        self.drain_ready();
        let unloads = self.observe_runtime_control();
        for (chunk_x, chunk_z) in unloads {
            light_cache.remove(ChunkPos {
                x: chunk_x,
                z: chunk_z,
            });
            write_packet(
                writer,
                &ForgetLevelChunk { chunk_x, chunk_z },
                self.compression,
            )
            .await?;
        }
        if !self.memory_pressure_active {
            self.dispatch_available().await;
        }
        self.drain_ready();

        let made_send_progress = self.emit_ready_batch(writer, light_cache).await?;
        let initial_target = initial_window_target(self.view_distance);
        if self.emitted + self.absent >= initial_target || self.scheduler.is_complete() {
            self.wait_for_first_chunk = false;
        }

        if self.scheduler.is_complete() {
            self.dispatch_forward_prewarm();
            self.set_stop_reason(ChunkPipelineStopReason::Complete);
            self.recover_runtime_control_sources();
            return Ok(ChunkStreamStep::Complete);
        }
        if !made_send_progress {
            self.yielded_turns += 1;
        }

        Ok(ChunkStreamStep::Progress)
    }

    async fn emit_ready_batch<W>(
        &mut self,
        writer: &mut W,
        light_cache: &mut LightCache,
    ) -> Result<bool, ConnectionError>
    where
        W: AsyncWriteExt + Unpin,
    {
        let limit = self.policy.chunk_send_rate.max(1) as usize;
        let mut sent = 0usize;
        while sent < limit {
            match self.emit_next_ready(writer, light_cache).await? {
                EmitReadyResult::SentPacket => sent += 1,
                EmitReadyResult::DrainedNoPacket => {}
                EmitReadyResult::Blocked | EmitReadyResult::Empty => break,
            }
        }
        if sent == limit && !self.ready.is_empty() {
            self.set_stop_reason(ChunkPipelineStopReason::SendBudget);
        }
        Ok(sent > 0)
    }

    fn observe_runtime_control(&mut self) -> Vec<(i32, i32)> {
        let Some(runtime_control) = self.runtime_control.clone() else {
            return Vec::new();
        };
        // Ready results remain scheduler in-flight until publication, so the
        // scheduler count already includes every occupied result slot.
        let queued_chunks = self.scheduler.in_flight_len();
        let snapshot = runtime_control.snapshot();
        let queue_capacity = self.result_queue_size.max(1);
        let saturated = queued_chunks.saturating_mul(100)
            >= queue_capacity.saturating_mul(snapshot.policy.queue_pressure_percent as usize);
        self.set_chunk_queue_saturated(saturated);

        let memory = runtime_control.memory_pressure_observation();
        let memory_pressure_active = if memory.available && memory.sample.limit_mb > 0 {
            memory.sample.used_mb.saturating_mul(100)
                >= memory
                    .sample
                    .limit_mb
                    .saturating_mul(u64::from(snapshot.policy.memory_pressure_percent))
        } else {
            memory.failures > 0
        };
        let should_shed_memory = memory_pressure_active && !self.memory_pressure_active;
        if should_shed_memory {
            self.shed_memory_pressure_work();
        }
        let unloads = self.apply_runtime_control_limits(snapshot.limits);
        self.memory_pressure_active = memory_pressure_active;
        if memory_pressure_active {
            self.set_stop_reason(ChunkPipelineStopReason::MemoryPressure);
        }
        unloads
    }

    fn set_chunk_queue_saturated(&mut self, saturated: bool) {
        if saturated == self.chunk_queue_saturated {
            return;
        }
        self.chunk_queue_saturated = saturated;
        if self
            .chunk_pressure_source
            .as_mut()
            .is_some_and(|source| !source.set_saturated(saturated))
        {
            debug!(saturated, "runtime control signal consumer closed");
        }
    }

    fn set_first_chunk_sla_active(&mut self, active: bool) {
        if active == self.first_chunk_sla_active {
            return;
        }
        self.first_chunk_sla_active = active;
        if self
            .first_chunk_sla_source
            .as_mut()
            .is_some_and(|source| !source.set_active(active))
        {
            debug!(active, "runtime control signal consumer closed");
        }
    }

    fn recover_runtime_control_sources(&mut self) {
        self.set_chunk_queue_saturated(false);
        self.set_first_chunk_sla_active(false);
    }

    fn apply_runtime_control_limits(
        &mut self,
        limits: crate::RuntimeControlLimits,
    ) -> Vec<(i32, i32)> {
        self.policy.chunk_send_rate = limits.chunk_send_rate.max(1);
        self.policy.chunk_load_rate = limits.chunk_load_rate.max(1);
        self.policy.chunk_generate_rate = limits.chunk_generate_rate.max(1);
        self.policy.chunk_prepare_batch_size = self.configured_prepare_batch_size.max(1);
        self.prepare_limit_stop_reason = ChunkPipelineStopReason::BatchLimit;
        self.runtime_view_distance_limit = limits.view_distance.max(0);
        let effective_view_distance = self
            .runtime_view_distance_limit
            .min(self.client_view_distance_cap);
        if effective_view_distance != self.view_distance {
            return self
                .replan_effective_view_distance(effective_view_distance, self.direction_yaw);
        }
        Vec::new()
    }

    async fn dispatch_available(&mut self) {
        self.dispatch_turns += 1;
        let started = Instant::now();
        let mut dispatched_this_turn = 0usize;
        let mut load_dispatched_this_turn = 0usize;
        let mut generate_dispatched_this_turn = 0usize;
        let mut budget_deferrals = 0usize;
        let mut claim_deferrals = 0usize;
        loop {
            if self.scheduler.in_flight_len() >= self.result_queue_size {
                self.set_stop_reason(ChunkPipelineStopReason::QueueFull);
                break;
            }
            if self.memory_pressure_active {
                self.set_stop_reason(ChunkPipelineStopReason::MemoryPressure);
                break;
            }
            if dispatched_this_turn >= self.policy.chunk_prepare_batch_size {
                self.set_stop_reason(self.prepare_limit_stop_reason);
                break;
            }
            if self.policy.chunk_prepare_budget_ms > 0
                && started.elapsed().as_millis() as u64 >= self.policy.chunk_prepare_budget_ms
            {
                self.set_stop_reason(ChunkPipelineStopReason::TimeBudget);
                break;
            }
            let Some(request) = self.scheduler.poll_next() else {
                let stop_reason = if self.scheduler.in_flight_len() == 0 {
                    ChunkPipelineStopReason::Complete
                } else {
                    ChunkPipelineStopReason::QueueEmpty
                };
                self.set_stop_reason(stop_reason);
                break;
            };
            let prepare_claim = match self.sessions.prepared_chunk_or_wait_for_earlier_session(
                (request.chunk_x, request.chunk_z),
                self.session_id,
            ) {
                SessionPreparedChunkClaimResult::Cached(prepared, revision) => {
                    self.accept_result(ChunkPrepareResult {
                        request,
                        prepare_claim: Some(PreparedChunkFence::CachedRevision(revision)),
                        fetch_ms: 0,
                        pressure_flush: PressureFlushTiming::default(),
                        staged: Vec::new(),
                        outcome: ChunkPrepareOutcome::Ready(Box::new(
                            prepared.prepared_cache_hit(),
                        )),
                    });
                    dispatched_this_turn += 1;
                    self.dispatched += 1;
                    budget_deferrals = 0;
                    claim_deferrals = 0;
                    continue;
                }
                SessionPreparedChunkClaimResult::Claimed(claim) => claim,
                SessionPreparedChunkClaimResult::WaitingForEarlierSession => {
                    if !self.scheduler.defer_front(request) {
                        self.set_stop_reason(ChunkPipelineStopReason::QueueEmpty);
                        break;
                    }
                    self.set_stop_reason(ChunkPipelineStopReason::QueueEmpty);
                    break;
                }
                SessionPreparedChunkClaimResult::InFlight => {
                    if !self.scheduler.defer(request) {
                        self.set_stop_reason(ChunkPipelineStopReason::QueueEmpty);
                        break;
                    }
                    claim_deferrals += 1;
                    if claim_deferrals >= PREPARED_IN_FLIGHT_DEFERRAL_LIMIT
                        || claim_deferrals >= self.scheduler.queued_len().max(1)
                    {
                        self.set_stop_reason(ChunkPipelineStopReason::QueueEmpty);
                        break;
                    }
                    continue;
                }
            };
            let budget_class = self.classify_prepare_budget(request).await;
            let budget_exhausted = match budget_class {
                ChunkPrepareBudgetClass::Load => {
                    load_dispatched_this_turn >= self.policy.chunk_load_rate as usize
                }
                ChunkPrepareBudgetClass::Generate => {
                    generate_dispatched_this_turn >= self.policy.chunk_generate_rate as usize
                }
            };
            if budget_exhausted {
                self.release_prepare_claim(
                    (request.chunk_x, request.chunk_z),
                    Some(PreparedChunkFence::Claimed(prepare_claim)),
                );
                let stop_reason = budget_class.stop_reason();
                if !self.scheduler.defer(request) {
                    self.set_stop_reason(stop_reason);
                    break;
                }
                budget_deferrals += 1;
                if budget_deferrals >= self.scheduler.queued_len().max(1) {
                    self.set_stop_reason(stop_reason);
                    break;
                }
                continue;
            }
            match budget_class {
                ChunkPrepareBudgetClass::Load => load_dispatched_this_turn += 1,
                ChunkPrepareBudgetClass::Generate => generate_dispatched_this_turn += 1,
            }
            self.spawn_prepare_worker(request, prepare_claim);
            dispatched_this_turn += 1;
            self.dispatched += 1;
            budget_deferrals = 0;
            claim_deferrals = 0;
        }
        self.max_in_flight = self.max_in_flight.max(self.scheduler.in_flight_len());
    }

    async fn classify_prepare_budget(&self, request: ChunkRequest) -> ChunkPrepareBudgetClass {
        let position = ChunkPos {
            x: request.chunk_x,
            z: request.chunk_z,
        };
        if let Some(chunk_source) = self.chunk_source.as_ref() {
            return match chunk_source.source_for(position) {
                mc_world::ChunkPrepareSource::Generator => ChunkPrepareBudgetClass::Generate,
                mc_world::ChunkPrepareSource::Resident
                | mc_world::ChunkPrepareSource::RegionFile
                | mc_world::ChunkPrepareSource::Absent => ChunkPrepareBudgetClass::Load,
            };
        }
        if self.world_read.as_ref().is_some_and(|world_read| {
            world_read
                .snapshot_chunks(&[position])
                .chunk(position)
                .is_some()
        }) {
            return ChunkPrepareBudgetClass::Load;
        }
        let storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::ChunkPrepare,
            "chunk prepare budget classify",
            Instant::now(),
            self.world.lock().await,
        );
        match storage.plan_chunk_snapshot_without_generation(position) {
            mc_world::ChunkSnapshotPlan::Cached(_) => ChunkPrepareBudgetClass::Load,
            mc_world::ChunkSnapshotPlan::Load(plan) if plan.has_load_source() => {
                ChunkPrepareBudgetClass::Load
            }
            mc_world::ChunkSnapshotPlan::Load(_) if storage.generator().is_some() => {
                ChunkPrepareBudgetClass::Generate
            }
            mc_world::ChunkSnapshotPlan::Load(_) => ChunkPrepareBudgetClass::Load,
        }
    }

    fn shed_memory_pressure_work(&mut self) {
        let ready = self.ready.len();
        let active = self.scheduler.in_flight_len().saturating_sub(ready);
        self.set_stop_reason(ChunkPipelineStopReason::MemoryPressure);
        if ready == 0 && active == 0 {
            return;
        }

        self.clear_ready();
        self.reset_pressure_tracking();
        self.reset_prewarm_tracking();
        self.scheduler.replace_view(prioritized_spiral(
            self.center_cx,
            self.center_cz,
            self.view_distance,
            self.direction_yaw,
        ));
        self.active_generation
            .store(self.scheduler.current_generation().0, Ordering::Release);
        self.memory_pressure_shed_runs += 1;
        self.memory_pressure_shed_ready += ready;
        self.memory_pressure_shed_in_flight += active;
    }

    fn dispatch_forward_prewarm(&mut self) {
        if self.view_distance <= 0
            || self.memory_pressure_active
            || self
                .runtime_control
                .as_ref()
                .is_some_and(|control| control.snapshot().draining)
            || self
                .sessions
                .has_later_session_at_center(self.session_id, (self.center_cx, self.center_cz))
        {
            return;
        }
        let mut batch = Vec::new();
        let session_registration_epoch = self.sessions.session_registration_epoch();
        let Some(player_pose) = self.sessions.player_pose(self.session_id) else {
            return;
        };
        for (sequence, coord) in prewarm_edge_batch_chunks(
            self.center_cx,
            self.center_cz,
            self.view_distance,
            self.direction_yaw,
            player_pose,
        )
        .into_iter()
        .enumerate()
        {
            if self.loaded.contains(&coord) || self.prewarm_in_flight.contains(&coord) {
                continue;
            }
            let prepare_claim = match self.sessions.prepared_chunk_or_claim(coord) {
                PreparedChunkClaimResult::Cached | PreparedChunkClaimResult::InFlight => {
                    continue;
                }
                PreparedChunkClaimResult::Claimed(claim) => claim,
            };
            let request = ChunkRequest {
                chunk_x: coord.0,
                chunk_z: coord.1,
                priority: ChunkPriority {
                    ring: (self.view_distance + 1) as u32,
                    sequence: sequence as u32,
                },
                generation: self.scheduler.current_generation(),
            };
            self.prewarm_in_flight.insert(coord);
            self.prewarm_dispatched += 1;
            batch.push((request, prepare_claim));
        }
        if !batch.is_empty() {
            self.spawn_prewarm_batch_worker(batch, session_registration_epoch);
        }
    }

    fn spawn_prepare_worker(&self, request: ChunkRequest, prepare_claim: PreparedChunkClaim) {
        let world = Arc::clone(&self.world);
        let world_read = self.world_read.clone();
        let world_mutation = self.world_mutation.clone();
        let biomes = Arc::clone(&self.biomes);
        let blocks = Arc::clone(&self.blocks);
        let block_light = self.block_light.as_ref().map(Arc::clone);
        let items = Arc::clone(&self.items);
        let tags = Arc::clone(&self.tags);
        let recipes = Arc::clone(&self.recipes);
        let block_entity_types = Arc::clone(&self.block_entity_types);
        let passive_herd_surface = self.passive_herd_surface;
        let passive_herd_fallback_surfaces = Arc::clone(&self.passive_herd_fallback_surfaces);
        let passive_herd_water = Arc::clone(&self.passive_herd_water);
        let passive_herd_passable = Arc::clone(&self.passive_herd_passable);
        let passive_spawn_rules = Arc::clone(&self.passive_spawn_rules);
        let entity_types = Arc::clone(&self.entity_types);
        let resources = self.resources.clone();
        let prepare_task = resources.begin_prepare_task();
        let active_generation = Arc::clone(&self.active_generation);
        let compression = self.compression;
        let current_tick = self.sessions.simulation_tick();
        let sessions = Arc::clone(&self.sessions);
        let tx = self.result_tx.clone();
        let progress_notify = Arc::clone(&self.progress_notify);
        tokio::spawn(async move {
            let _prepare_task = prepare_task;
            let mut claim = PreparedChunkClaimLease::new(
                sessions,
                (request.chunk_x, request.chunk_z),
                prepare_claim,
            );
            let worker = tokio::spawn(async move {
                let request_admission = match resources.acquire_prepare_request().await {
                    Ok(permit) => permit,
                    Err(_) => {
                        return ChunkPrepareResult {
                            request,
                            prepare_claim: None,
                            fetch_ms: 0,
                            pressure_flush: PressureFlushTiming::default(),
                            staged: Vec::new(),
                            outcome: ChunkPrepareOutcome::Failed(
                                "chunk request admission closed".into(),
                            ),
                        };
                    }
                };
                let _request_admission = request_admission;
                prepare_chunk_request(
                    request,
                    world,
                    world_read,
                    world_mutation,
                    biomes,
                    blocks,
                    block_light,
                    items,
                    tags,
                    recipes,
                    block_entity_types,
                    passive_herd_surface,
                    passive_herd_fallback_surfaces,
                    passive_herd_water,
                    passive_herd_passable,
                    passive_spawn_rules,
                    entity_types,
                    compression,
                    resources,
                    active_generation,
                    current_tick,
                )
                .await
            });
            let mut result = match worker.await {
                Ok(result) => result,
                Err(error) => ChunkPrepareResult {
                    request,
                    prepare_claim: None,
                    fetch_ms: 0,
                    pressure_flush: PressureFlushTiming::default(),
                    staged: Vec::new(),
                    outcome: ChunkPrepareOutcome::Failed(format!(
                        "chunk prepare worker failed: {error}"
                    )),
                },
            };
            result.prepare_claim = Some(PreparedChunkFence::Claimed(claim.claim()));
            let sent = tx.send(result).await.is_ok();
            progress_notify.notify_one();
            if sent {
                claim.disarm();
            }
        });
    }

    fn spawn_prewarm_batch_worker(
        &self,
        batch: Vec<(ChunkRequest, PreparedChunkClaim)>,
        session_registration_epoch: SessionId,
    ) {
        let world = Arc::clone(&self.world);
        let world_read = self.world_read.clone();
        let world_mutation = self.world_mutation.clone();
        let biomes = Arc::clone(&self.biomes);
        let blocks = Arc::clone(&self.blocks);
        let block_light = self.block_light.as_ref().map(Arc::clone);
        let items = Arc::clone(&self.items);
        let tags = Arc::clone(&self.tags);
        let recipes = Arc::clone(&self.recipes);
        let block_entity_types = Arc::clone(&self.block_entity_types);
        let passive_herd_surface = self.passive_herd_surface;
        let passive_herd_fallback_surfaces = Arc::clone(&self.passive_herd_fallback_surfaces);
        let passive_herd_water = Arc::clone(&self.passive_herd_water);
        let passive_herd_passable = Arc::clone(&self.passive_herd_passable);
        let passive_spawn_rules = Arc::clone(&self.passive_spawn_rules);
        let entity_types = Arc::clone(&self.entity_types);
        let resources = self.resources.clone();
        let prepare_task = resources.begin_prepare_task();
        let compression = self.compression;
        let current_tick = self.sessions.simulation_tick();
        let sessions = Arc::clone(&self.sessions);
        let session_id = self.session_id;
        let prewarm_center = (self.center_cx, self.center_cz);
        let progress_notify = Arc::clone(&self.progress_notify);
        let batch = batch
            .into_iter()
            .map(|(request, claim)| {
                let chunk = (request.chunk_x, request.chunk_z);
                (
                    request,
                    PreparedChunkClaimLease::new(Arc::clone(&sessions), chunk, claim),
                )
            })
            .collect::<Vec<_>>();
        let worker_count = resources.cpu_limit().min(batch.len()).max(1);
        let mut pending = batch.into_iter();
        let mut waves = Vec::new();
        loop {
            let wave = pending.by_ref().take(worker_count).collect::<Vec<_>>();
            if wave.is_empty() {
                break;
            }
            waves.push(wave);
        }
        tokio::spawn(async move {
            let _prepare_task = prepare_task;
            for wave in waves {
                if sessions.session_registration_epoch() > session_registration_epoch
                    || !sessions.session_is_at_center(session_id, prewarm_center)
                {
                    break;
                }
                let mut workers = tokio::task::JoinSet::new();
                for (request, prepare_claim) in wave {
                    let world = Arc::clone(&world);
                    let world_read = world_read.clone();
                    let world_mutation = world_mutation.clone();
                    let biomes = Arc::clone(&biomes);
                    let blocks = Arc::clone(&blocks);
                    let block_light = block_light.as_ref().map(Arc::clone);
                    let items = Arc::clone(&items);
                    let tags = Arc::clone(&tags);
                    let recipes = Arc::clone(&recipes);
                    let block_entity_types = Arc::clone(&block_entity_types);
                    let passive_herd_fallback_surfaces =
                        Arc::clone(&passive_herd_fallback_surfaces);
                    let passive_herd_water = Arc::clone(&passive_herd_water);
                    let passive_herd_passable = Arc::clone(&passive_herd_passable);
                    let passive_spawn_rules = Arc::clone(&passive_spawn_rules);
                    let entity_types = Arc::clone(&entity_types);
                    let resources = resources.clone();
                    let sessions = Arc::clone(&sessions);
                    workers.spawn(async move {
                        if sessions.session_registration_epoch() > session_registration_epoch
                            || !sessions.session_is_at_center(session_id, prewarm_center)
                        {
                            return;
                        }
                        let Ok(_request_admission) = resources.acquire_prepare_request().await
                        else {
                            return;
                        };
                        if sessions.session_registration_epoch() > session_registration_epoch
                            || !sessions.session_is_at_center(session_id, prewarm_center)
                        {
                            return;
                        }
                        let result = prepare_chunk_request(
                            request,
                            Arc::clone(&world),
                            world_read.clone(),
                            world_mutation,
                            Arc::clone(&biomes),
                            Arc::clone(&blocks),
                            block_light.as_ref().map(Arc::clone),
                            Arc::clone(&items),
                            Arc::clone(&tags),
                            Arc::clone(&recipes),
                            Arc::clone(&block_entity_types),
                            passive_herd_surface,
                            Arc::clone(&passive_herd_fallback_surfaces),
                            Arc::clone(&passive_herd_water),
                            Arc::clone(&passive_herd_passable),
                            Arc::clone(&passive_spawn_rules),
                            Arc::clone(&entity_types),
                            compression,
                            resources.clone(),
                            Arc::new(AtomicU64::new(request.generation.0)),
                            current_tick,
                        )
                        .await;
                        if let ChunkPrepareOutcome::Ready(prepared) = result.outcome {
                            sessions.cache_prewarmed_chunk(
                                (request.chunk_x, request.chunk_z),
                                prepare_claim.claim().revision,
                                Arc::new(*prepared),
                                PREWARM_PREPARED_CACHE_LIMIT,
                            );
                        }
                    });
                }
                while let Some(result) = workers.join_next().await {
                    if let Err(error) = result {
                        warn!(?error, "forward prewarm worker failed");
                    }
                }
            }
            progress_notify.notify_one();
        });
    }

    fn drain_ready(&mut self) {
        self.resources
            .observe_result_queue_depth(self.result_rx.len());
        while let Ok(result) = self.result_rx.try_recv() {
            self.accept_result(result);
        }
    }

    fn accept_result(&mut self, result: ChunkPrepareResult) {
        if !self.scheduler.is_current(result.request) {
            self.release_prepare_claim_for_result(&result);
            return;
        }
        if self.ready.contains_key(&result.request.priority.sequence) {
            self.release_prepare_claim_for_result(&result);
            return;
        }
        self.ready.insert(result.request.priority.sequence, result);
        self.max_ready = self.max_ready.max(self.ready.len());
    }

    fn clear_ready(&mut self) {
        let ready = std::mem::take(&mut self.ready);
        for result in ready.into_values() {
            self.release_prepare_claim_for_result(&result);
        }
    }

    fn release_prepare_claim_for_result(&self, result: &ChunkPrepareResult) {
        self.release_prepare_claim(
            (result.request.chunk_x, result.request.chunk_z),
            result.prepare_claim,
        );
    }

    fn release_prepare_claim(&self, chunk: (i32, i32), claim: Option<PreparedChunkFence>) {
        if let Some(PreparedChunkFence::Claimed(claim)) = claim {
            self.sessions.release_prepared_chunk_claim(chunk, claim);
        }
    }

    fn record_pressure_flush(&mut self, timing: PressureFlushTiming) {
        self.pressure_flush_runs += timing.runs;
        self.pressure_flush_planned_chunks += timing.planned_chunks;
        self.pressure_flush_flushed_chunks += timing.flushed_chunks;
        self.pressure_flush_plan_ms += timing.plan_ms;
        self.pressure_flush_write_ms += timing.write_ms;
        self.pressure_flush_commit_ms += timing.commit_ms;
        self.max_pressure_flush_plan_ms = self.max_pressure_flush_plan_ms.max(timing.plan_ms);
        self.max_pressure_flush_write_ms = self.max_pressure_flush_write_ms.max(timing.write_ms);
        self.max_pressure_flush_commit_ms = self.max_pressure_flush_commit_ms.max(timing.commit_ms);
    }

    async fn emit_next_ready<W>(
        &mut self,
        writer: &mut W,
        light_cache: &mut LightCache,
    ) -> Result<EmitReadyResult, ConnectionError>
    where
        W: AsyncWriteExt + Unpin,
    {
        let Some((_, result)) = self.ready.pop_first() else {
            return Ok(EmitReadyResult::Empty);
        };
        let request = result.request;
        let cx = request.chunk_x;
        let cz = request.chunk_z;
        let prepare_claim = result.prepare_claim;
        let prepared_revision = prepare_claim.map(PreparedChunkFence::revision);
        self.fetch_ms += result.fetch_ms;
        self.record_pressure_flush(result.pressure_flush);
        self.max_fetch_ms = self.max_fetch_ms.max(result.fetch_ms);
        if result.fetch_ms >= CHUNK_STAGE_SLOW_MS {
            self.slow_fetch_chunks += 1;
        }

        match result.outcome {
            ChunkPrepareOutcome::Ready(prepared) => {
                if prepared_revision.is_some_and(|revision| {
                    !self
                        .sessions
                        .prepared_revision_is_current((cx, cz), revision)
                }) {
                    self.release_prepare_claim((cx, cz), prepare_claim);
                    if !self.scheduler.defer(request) {
                        warn!(cx, cz, "stale prepared chunk could not be requeued");
                        return Ok(EmitReadyResult::DrainedNoPacket);
                    }
                    return Ok(EmitReadyResult::Blocked);
                }
                self.clear_pressure_tracking((cx, cz));
                self.staged.extend(result.staged);
                if let Some(light) = prepared.light.clone() {
                    light_cache.insert(ChunkPos { x: cx, z: cz }, light);
                }
                let mut write_timing = prepared.write_timing;
                let socket_write_started = Instant::now();
                if let Err(err) = writer.write_all(&prepared.frame).await {
                    self.release_prepare_claim((cx, cz), prepare_claim);
                    return Err(err.into());
                }
                write_timing.socket_write_ms = socket_write_started.elapsed().as_millis() as u64;
                let visibility = if let Some(revision) = prepared_revision {
                    let Some(visibility) = self.sessions.mark_loaded_if_prepared_revision_current(
                        self.session_id,
                        (cx, cz),
                        revision,
                    ) else {
                        light_cache.remove(ChunkPos { x: cx, z: cz });
                        self.release_prepare_claim((cx, cz), prepare_claim);
                        if !self.scheduler.defer(request) {
                            warn!(cx, cz, "invalidated prepared chunk could not be requeued");
                            return Ok(EmitReadyResult::DrainedNoPacket);
                        }
                        return Ok(EmitReadyResult::Blocked);
                    };
                    visibility
                } else {
                    self.sessions.mark_loaded(self.session_id, (cx, cz))
                };
                self.loaded.insert((cx, cz));
                for (position, cooking) in &prepared.hydrated_campfires {
                    self.sessions
                        .restore_campfire_cooking(*position, cooking.clone());
                }
                #[cfg(test)]
                let mut visibility = visibility;
                let herd_spawns =
                    natural_spawns_for_policy(&prepared.herd_spawns, self.spawn_monsters);
                if let Some(simulation) = self.simulation.as_ref() {
                    if let Err(error) = simulation.ensure_chunk_herd((cx, cz), herd_spawns.clone())
                    {
                        warn!(?error, cx, cz, "simulation chunk herd request rejected");
                    }
                } else {
                    #[cfg(test)]
                    {
                        visibility.extend(
                            self.sessions
                                .ensure_chunk_herd_legacy_for_test((cx, cz), &herd_spawns),
                        );
                    }
                    #[cfg(not(test))]
                    {
                        warn!(cx, cz, "chunk stream has no simulation owner");
                    }
                }
                dispatch_visibility_commands(visibility);
                if let Some(revision) = prepared_revision {
                    self.sessions.cache_prepared_chunk_if_current(
                        (cx, cz),
                        revision,
                        Arc::new((*prepared).clone()),
                    );
                }
                self.release_prepare_claim((cx, cz), prepare_claim);
                self.record_stage_maxima(
                    cx,
                    cz,
                    prepared_revision,
                    prepared.build_timing,
                    write_timing,
                );
                self.build_timing.add(prepared.build_timing);
                self.record_emitted(cx, cz, prepared.packet_data_len, write_timing);
            }
            ChunkPrepareOutcome::Absent => {
                self.release_prepare_claim((cx, cz), prepare_claim);
                self.clear_pressure_tracking((cx, cz));
                self.staged.extend(result.staged);
                self.absent += 1;
                info!(cx, cz, "no chunk in storage");
                self.scheduler.mark_finished(request);
                return Ok(EmitReadyResult::DrainedNoPacket);
            }
            ChunkPrepareOutcome::Backpressured => {
                self.release_prepare_claim((cx, cz), prepare_claim);
                self.set_pressure_staged((cx, cz), &result.staged);
                let retries = self.pressure_retries.entry((cx, cz)).or_default();
                *retries += 1;
                if *retries >= CHUNK_BACKPRESSURE_MAX_RETRIES {
                    self.clear_pressure_tracking((cx, cz));
                    self.pressure_abandoned += 1;
                    warn!(
                        cx,
                        cz,
                        retries = CHUNK_BACKPRESSURE_MAX_RETRIES,
                        pressure_abandoned = self.pressure_abandoned,
                        "chunk preparation abandoned after repeated dirty chunk cache pressure"
                    );
                    self.scheduler.mark_finished(request);
                    return Ok(EmitReadyResult::DrainedNoPacket);
                }
                if !self.scheduler.defer(request) {
                    self.clear_pressure_tracking((cx, cz));
                    self.pressure_abandoned += 1;
                    warn!(
                        cx,
                        cz,
                        pressure_abandoned = self.pressure_abandoned,
                        "dirty chunk pressure defer failed after request left in-flight set"
                    );
                    return Ok(EmitReadyResult::Blocked);
                }
                info!(
                    cx,
                    cz,
                    retry = *retries,
                    "chunk preparation deferred by dirty chunk cache pressure"
                );
                return Ok(EmitReadyResult::Blocked);
            }
            ChunkPrepareOutcome::Failed(err) => {
                self.release_prepare_claim((cx, cz), prepare_claim);
                self.clear_pressure_tracking((cx, cz));
                return Err(ConnectionError::ChunkPreparation {
                    chunk_x: cx,
                    chunk_z: cz,
                    reason: err,
                });
            }
        }

        self.scheduler.mark_finished(request);
        Ok(EmitReadyResult::SentPacket)
    }

    fn set_pressure_staged(&mut self, coord: (i32, i32), staged: &[(i32, i32)]) {
        self.pressure_staged_by_chunk
            .insert(coord, staged.iter().copied().collect());
    }

    fn clear_pressure_staged(&mut self, coord: (i32, i32)) {
        self.pressure_staged_by_chunk.remove(&coord);
    }

    fn clear_pressure_tracking(&mut self, coord: (i32, i32)) {
        self.pressure_retries.remove(&coord);
        self.clear_pressure_staged(coord);
    }

    fn reset_pressure_tracking(&mut self) {
        self.pressure_retries.clear();
        self.pressure_staged_by_chunk.clear();
    }

    fn reset_prewarm_tracking(&mut self) {
        self.prewarm_in_flight.clear();
    }

    fn pressure_staged_count(&self) -> usize {
        self.pressure_staged_by_chunk
            .values()
            .flat_map(|staged| staged.iter().copied())
            .collect::<HashSet<_>>()
            .len()
    }

    #[cfg(test)]
    fn pressure_staged_contains(&self, coord: (i32, i32)) -> bool {
        self.pressure_staged_by_chunk
            .values()
            .any(|staged| staged.contains(&coord))
    }

    #[cfg(test)]
    fn pressure_staged_is_empty(&self) -> bool {
        self.pressure_staged_by_chunk
            .values()
            .all(HashSet::is_empty)
    }

    fn record_stage_maxima(
        &mut self,
        cx: i32,
        cz: i32,
        prepared_revision: Option<u64>,
        build_timing: ChunkBuildTiming,
        write_timing: ChunkWriteTiming,
    ) {
        self.max_chunk_data_ms = self.max_chunk_data_ms.max(build_timing.chunk_data_ms);
        self.max_heightmap_ms = self.max_heightmap_ms.max(build_timing.heightmap_ms);
        if build_timing.light_compute_ms > self.max_light_compute_ms {
            self.max_light_compute_ms = build_timing.light_compute_ms;
            self.max_light_compute_chunk = Some((cx, cz));
            self.max_light_compute_revision = prepared_revision;
        }
        self.max_light_encode_ms = self.max_light_encode_ms.max(build_timing.light_encode_ms);
        self.max_packet_encode_ms = self.max_packet_encode_ms.max(write_timing.packet_encode_ms);
        self.max_frame_ms = self.max_frame_ms.max(write_timing.frame_ms);
        self.max_socket_write_ms = self.max_socket_write_ms.max(write_timing.socket_write_ms);
        if build_timing.light_compute_ms >= CHUNK_STAGE_SLOW_MS {
            self.slow_light_compute_chunks += 1;
        }
        if write_timing.packet_encode_ms >= CHUNK_STAGE_SLOW_MS {
            self.slow_packet_encode_chunks += 1;
        }
        if write_timing.frame_ms >= CHUNK_STAGE_SLOW_MS {
            self.slow_frame_chunks += 1;
        }
        if write_timing.socket_write_ms >= CHUNK_STAGE_SLOW_MS {
            self.slow_socket_write_chunks += 1;
        }
    }

    fn record_emitted(
        &mut self,
        cx: i32,
        cz: i32,
        packet_data_len: usize,
        write_timing: ChunkWriteTiming,
    ) {
        self.packet_encode_ms += write_timing.packet_encode_ms;
        self.frame_ms += write_timing.frame_ms;
        self.socket_write_ms += write_timing.socket_write_ms;
        self.framed_bytes += write_timing.framed_bytes;
        self.emitted += 1;
        if self.first_chunk_ms.is_none() {
            let elapsed_ms = self.started.elapsed().as_millis() as u64;
            self.first_chunk_ms = Some(elapsed_ms);
            self.set_first_chunk_sla_active(elapsed_ms > self.first_chunk_sla_target_ms);
        }
        self.record_ring_progress(cx, cz);
        self.bytes += packet_data_len;
    }

    fn record_ring_progress(&mut self, cx: i32, cz: i32) {
        let ring = (cx - self.center_cx).abs().max((cz - self.center_cz).abs()) as usize;
        if let Some(count) = self.ring_emitted.get_mut(ring) {
            *count += 1;
            let needed = if ring == 0 { 1 } else { ring * 8 };
            if *count == needed {
                let elapsed = self.started.elapsed().as_millis() as u64;
                if ring == 1 {
                    self.ring1_complete_ms.get_or_insert(elapsed);
                } else if ring == 2 {
                    self.ring2_complete_ms.get_or_insert(elapsed);
                }
            }
        }
    }

    pub(super) fn log_summary_once(&mut self) {
        if self.summary_logged {
            return;
        }
        self.summary_logged = true;
        info!(
            center_cx = self.center_cx,
            center_cz = self.center_cz,
            direction_yaw = self.direction_yaw,
            view_distance = self.view_distance,
            staged = self.staged.len(),
            emitted = self.emitted,
            absent = self.absent,
            pressure_abandoned = self.pressure_abandoned,
            pressure_staged = self.pressure_staged_count(),
            pressure_flush_runs = self.pressure_flush_runs,
            pressure_flush_planned_chunks = self.pressure_flush_planned_chunks,
            pressure_flush_flushed_chunks = self.pressure_flush_flushed_chunks,
            pressure_flush_plan_ms = self.pressure_flush_plan_ms,
            pressure_flush_write_ms = self.pressure_flush_write_ms,
            pressure_flush_commit_ms = self.pressure_flush_commit_ms,
            max_pressure_flush_plan_ms = self.max_pressure_flush_plan_ms,
            max_pressure_flush_write_ms = self.max_pressure_flush_write_ms,
            max_pressure_flush_commit_ms = self.max_pressure_flush_commit_ms,
            memory_pressure_shed_runs = self.memory_pressure_shed_runs,
            memory_pressure_shed_ready = self.memory_pressure_shed_ready,
            memory_pressure_shed_in_flight = self.memory_pressure_shed_in_flight,
            degraded_delivery = self.pressure_abandoned > 0 || self.absent > 0,
            bytes = self.bytes,
            framed_bytes = self.framed_bytes,
            fetch_ms = self.fetch_ms,
            chunk_data_ms = self.build_timing.chunk_data_ms,
            heightmap_ms = self.build_timing.heightmap_ms,
            light_compute_ms = self.build_timing.light_compute_ms,
            light_encode_ms = self.build_timing.light_encode_ms,
            packet_encode_ms = self.packet_encode_ms,
            frame_ms = self.frame_ms,
            socket_write_ms = self.socket_write_ms,
            max_fetch_ms = self.max_fetch_ms,
            max_chunk_data_ms = self.max_chunk_data_ms,
            max_heightmap_ms = self.max_heightmap_ms,
            max_light_compute_ms = self.max_light_compute_ms,
            max_light_compute_chunk = ?self.max_light_compute_chunk,
            max_light_compute_revision = self.max_light_compute_revision,
            max_light_encode_ms = self.max_light_encode_ms,
            max_packet_encode_ms = self.max_packet_encode_ms,
            max_frame_ms = self.max_frame_ms,
            max_socket_write_ms = self.max_socket_write_ms,
            slow_stage_threshold_ms = CHUNK_STAGE_SLOW_MS,
            slow_fetch_chunks = self.slow_fetch_chunks,
            slow_light_compute_chunks = self.slow_light_compute_chunks,
            slow_packet_encode_chunks = self.slow_packet_encode_chunks,
            slow_frame_chunks = self.slow_frame_chunks,
            slow_socket_write_chunks = self.slow_socket_write_chunks,
            chunk_send_rate = self.policy.chunk_send_rate,
            chunk_load_rate = self.policy.chunk_load_rate,
            chunk_generate_rate = self.policy.chunk_generate_rate,
            chunk_prepare_budget_ms = self.policy.chunk_prepare_budget_ms,
            chunk_prepare_batch_size = self.policy.chunk_prepare_batch_size,
            chunk_io_threads = self.policy.chunk_io_threads,
            chunk_worker_threads = self.policy.chunk_worker_threads,
            chunk_result_queue_size = self.policy.chunk_result_queue_size,
            compression_level = ?self.policy.compression_level,
            dispatch_turns = self.dispatch_turns,
            yielded_turns = self.yielded_turns,
            dispatched = self.dispatched,
            prewarm_dispatched = self.prewarm_dispatched,
            in_flight = self.scheduler.in_flight_len(),
            max_in_flight = self.max_in_flight,
            ready = self.ready.len(),
            max_ready = self.max_ready,
            stop_reason = ?self.last_stop_reason,
            first_chunk_ms = self.first_chunk_ms,
            ring1_complete_ms = self.ring1_complete_ms,
            ring2_complete_ms = self.ring2_complete_ms,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "chunk stream finished",
        );
    }
}

pub(super) fn natural_spawns_for_policy(
    spawns: &[HerdSpawn],
    spawn_monsters: bool,
) -> Vec<HerdSpawn> {
    if spawn_monsters {
        return spawns.to_vec();
    }
    spawns
        .iter()
        .filter(|spawn| !spawn.hostile)
        .cloned()
        .collect()
}

impl Drop for ChunkStreamState {
    fn drop(&mut self) {
        self.recover_runtime_control_sources();
        self.active_generation.store(0, Ordering::Release);
        let cancelled_requests = self.scheduler.queued_len()
            + self.scheduler.in_flight_len()
            + self.prewarm_in_flight.len();
        self.resources
            .record_stream_cancellation(cancelled_requests);
        self.clear_ready();
    }
}

/// Iterate chunk positions around `(center_x, center_z)` outwards
/// to `view_distance` in chebyshev-ring order. The first cell is the
/// centre; subsequent yields are every cell on ring `r = 1`, then
/// every cell on ring `r = 2`, etc. Within a ring the order is
/// row-major over the bounding square — perceptually this still
/// "spreads" because each ring fills before the next starts.
/// Coverage is identical to a row-major scan: `(2*view_distance +
/// 1)²` cells total, each yielded exactly once. Distances above the
/// protocol limit are capped before allocating or iterating.
pub(super) fn spiral_chunks(
    center_x: i32,
    center_z: i32,
    view_distance: i32,
) -> impl Iterator<Item = (i32, i32)> {
    let vd = view_distance.clamp(0, crate::MAX_VIEW_DISTANCE);
    let diameter = (2 * vd + 1) as usize;
    let mut out = Vec::with_capacity(diameter * diameter);
    out.push((center_x, center_z));

    for r in 1..=vd {
        for dx in -r..=r {
            out.push((center_x + dx, center_z - r));
        }
        for dz in (-r + 1)..r {
            out.push((center_x - r, center_z + dz));
            out.push((center_x + r, center_z + dz));
        }
        for dx in -r..=r {
            out.push((center_x + dx, center_z + r));
        }
    }

    out.into_iter()
}

pub(super) fn prioritized_spiral(
    center_x: i32,
    center_z: i32,
    view_distance: i32,
    direction_yaw: f32,
) -> impl Iterator<Item = (i32, i32, ChunkPriority)> {
    let mut chunks: Vec<_> = spiral_chunks(center_x, center_z, view_distance).collect();
    let (forward_x, forward_z) = yaw_forward(direction_yaw);
    chunks.sort_by(|&(left_x, left_z), &(right_x, right_z)| {
        let left_dx = left_x - center_x;
        let left_dz = left_z - center_z;
        let right_dx = right_x - center_x;
        let right_dz = right_z - center_z;
        let left_ring = left_dx.abs().max(left_dz.abs());
        let right_ring = right_dx.abs().max(right_dz.abs());
        left_ring
            .cmp(&right_ring)
            .then_with(|| {
                directional_score(right_dx, right_dz, forward_x, forward_z)
                    .total_cmp(&directional_score(left_dx, left_dz, forward_x, forward_z))
            })
            .then_with(|| {
                directional_lateral(left_dx, left_dz, forward_x, forward_z).total_cmp(
                    &directional_lateral(right_dx, right_dz, forward_x, forward_z),
                )
            })
            .then_with(|| left_z.cmp(&right_z))
            .then_with(|| left_x.cmp(&right_x))
    });
    chunks
        .into_iter()
        .enumerate()
        .map(move |(sequence, (cx, cz))| {
            (
                cx,
                cz,
                ChunkPriority {
                    ring: (cx - center_x).abs().max((cz - center_z).abs()) as u32,
                    sequence: sequence as u32,
                },
            )
        })
}

pub(super) fn prewarm_edge_ring_chunks(
    center_x: i32,
    center_z: i32,
    view_distance: i32,
    direction_yaw: f32,
) -> Vec<(i32, i32)> {
    let vd = view_distance.clamp(0, crate::MAX_VIEW_DISTANCE);
    let radius = vd + 1;
    let (forward_x, forward_z) = yaw_forward(direction_yaw);
    let mut chunks = Vec::new();
    if forward_x.abs() > forward_z.abs() {
        let forward_sign = if forward_x.is_sign_negative() { -1 } else { 1 };
        push_x_prewarm_edge(&mut chunks, center_x, center_z, radius, vd, forward_sign);
        push_x_prewarm_edge(&mut chunks, center_x, center_z, radius, vd, -forward_sign);
    } else {
        let forward_sign = if forward_z.is_sign_negative() { -1 } else { 1 };
        push_z_prewarm_edge(&mut chunks, center_x, center_z, radius, vd, forward_sign);
        push_z_prewarm_edge(&mut chunks, center_x, center_z, radius, vd, -forward_sign);
    }

    let mut remaining = Vec::new();
    for dx in -radius..=radius {
        for dz in -radius..=radius {
            if dx.abs().max(dz.abs()) == radius {
                remaining.push((center_x + dx, center_z + dz));
            }
        }
    }
    remaining.sort_by(|&(left_x, left_z), &(right_x, right_z)| {
        let left_dx = left_x - center_x;
        let left_dz = left_z - center_z;
        let right_dx = right_x - center_x;
        let right_dz = right_z - center_z;
        directional_score(right_dx, right_dz, forward_x, forward_z)
            .total_cmp(&directional_score(left_dx, left_dz, forward_x, forward_z))
            .then_with(|| {
                directional_lateral(left_dx, left_dz, forward_x, forward_z).total_cmp(
                    &directional_lateral(right_dx, right_dz, forward_x, forward_z),
                )
            })
            .then_with(|| left_z.cmp(&right_z))
            .then_with(|| left_x.cmp(&right_x))
    });
    for chunk in remaining {
        push_unique_prewarm_chunk(&mut chunks, chunk);
    }
    chunks
}

fn prewarm_edge_batch_limit(view_distance: i32) -> usize {
    let vd = view_distance.clamp(0, crate::MAX_VIEW_DISTANCE) as usize;
    if vd == 0 {
        return 0;
    }
    (3 * (2 * vd + 1)).min(PREWARM_EDGE_RING_LIMIT)
}

fn prewarm_edge_batch_chunks(
    center_x: i32,
    center_z: i32,
    view_distance: i32,
    direction_yaw: f32,
    player_pose: PlayerPose,
) -> Vec<(i32, i32)> {
    let vd = view_distance.clamp(0, crate::MAX_VIEW_DISTANCE);
    if vd == 0 {
        return Vec::new();
    }
    let radius = vd + 1;
    let (forward_x, forward_z) = yaw_forward(direction_yaw);
    let mut chunks = Vec::with_capacity(prewarm_edge_batch_limit(vd));
    if forward_x.abs() > forward_z.abs() {
        let forward_sign = if forward_x.is_sign_negative() { -1 } else { 1 };
        let local_z = player_pose.z - f64::from(center_z) * 16.0;
        let lateral_sign = if local_z <= 8.0 { -1 } else { 1 };
        let local_x = player_pose.x - f64::from(center_x) * 16.0;
        let mut edges = [
            (
                distance_to_signed_chunk_edge(local_x, forward_sign),
                0u8,
                true,
                forward_sign,
            ),
            (
                distance_to_signed_chunk_edge(local_x, -forward_sign),
                2u8,
                true,
                -forward_sign,
            ),
            (
                distance_to_signed_chunk_edge(local_z, lateral_sign),
                1u8,
                false,
                lateral_sign,
            ),
        ];
        edges.sort_by(|left, right| {
            left.0
                .total_cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
        });
        for (_, _, x_edge, sign) in edges {
            if x_edge {
                push_x_prewarm_edge(&mut chunks, center_x, center_z, radius, vd, sign);
            } else {
                push_z_prewarm_edge(&mut chunks, center_x, center_z, radius, vd, sign);
            }
        }
    } else {
        let forward_sign = if forward_z.is_sign_negative() { -1 } else { 1 };
        let local_x = player_pose.x - f64::from(center_x) * 16.0;
        let lateral_sign = if local_x <= 8.0 { -1 } else { 1 };
        let local_z = player_pose.z - f64::from(center_z) * 16.0;
        let mut edges = [
            (
                distance_to_signed_chunk_edge(local_z, forward_sign),
                0u8,
                false,
                forward_sign,
            ),
            (
                distance_to_signed_chunk_edge(local_z, -forward_sign),
                2u8,
                false,
                -forward_sign,
            ),
            (
                distance_to_signed_chunk_edge(local_x, lateral_sign),
                1u8,
                true,
                lateral_sign,
            ),
        ];
        edges.sort_by(|left, right| {
            left.0
                .total_cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
        });
        for (_, _, x_edge, sign) in edges {
            if x_edge {
                push_x_prewarm_edge(&mut chunks, center_x, center_z, radius, vd, sign);
            } else {
                push_z_prewarm_edge(&mut chunks, center_x, center_z, radius, vd, sign);
            }
        }
    }

    if chunks.len() < prewarm_edge_batch_limit(vd) {
        for chunk in prewarm_edge_ring_chunks(center_x, center_z, vd, direction_yaw) {
            push_unique_prewarm_chunk(&mut chunks, chunk);
            if chunks.len() == prewarm_edge_batch_limit(vd) {
                break;
            }
        }
    }
    chunks
}

fn distance_to_signed_chunk_edge(local: f64, sign: i32) -> f64 {
    let local = local.clamp(0.0, 16.0);
    if sign < 0 { local } else { 16.0 - local }
}

fn push_z_prewarm_edge(
    chunks: &mut Vec<(i32, i32)>,
    center_x: i32,
    center_z: i32,
    radius: i32,
    vd: i32,
    sign: i32,
) {
    let edge_z = center_z + sign * radius;
    for dx in -vd..=vd {
        push_unique_prewarm_chunk(chunks, (center_x + dx, edge_z));
    }
}

fn push_x_prewarm_edge(
    chunks: &mut Vec<(i32, i32)>,
    center_x: i32,
    center_z: i32,
    radius: i32,
    vd: i32,
    sign: i32,
) {
    let edge_x = center_x + sign * radius;
    for dz in -vd..=vd {
        push_unique_prewarm_chunk(chunks, (edge_x, center_z + dz));
    }
}

fn push_unique_prewarm_chunk(chunks: &mut Vec<(i32, i32)>, chunk: (i32, i32)) {
    if !chunks.contains(&chunk) {
        chunks.push(chunk);
    }
}

fn initial_window_target(view_distance: i32) -> usize {
    let ring = view_distance.clamp(0, INITIAL_CHUNK_MIN_RING) as usize;
    (2 * ring + 1).pow(2)
}

fn yaw_forward(yaw: f32) -> (f64, f64) {
    let radians = f64::from(yaw).to_radians();
    (-radians.sin(), radians.cos())
}

fn directional_score(dx: i32, dz: i32, forward_x: f64, forward_z: f64) -> f64 {
    f64::from(dx) * forward_x + f64::from(dz) * forward_z
}

fn directional_lateral(dx: i32, dz: i32, forward_x: f64, forward_z: f64) -> f64 {
    (f64::from(dx) * forward_z - f64::from(dz) * forward_x).abs()
}

#[allow(clippy::too_many_arguments)]
async fn prepare_chunk_request(
    request: ChunkRequest,
    world: WorldHandle,
    world_read: Option<mc_world::WorldReadView>,
    world_mutation: Option<mc_world::WorldMutationView>,
    biomes: Arc<Registry>,
    blocks: Arc<BlockRegistry>,
    block_light: Option<Arc<BlockLightTable>>,
    items: Arc<ItemRegistry>,
    tags: Arc<TagsData>,
    recipes: Arc<Vec<mc_data::recipes::Recipe>>,
    block_entity_types: Arc<mc_data::block_entity_types::BlockEntityTypeRegistry>,
    passive_herd_surface: Option<mc_world::BlockStateId>,
    passive_herd_fallback_surfaces: Arc<Vec<mc_world::BlockStateId>>,
    passive_herd_water: Arc<Vec<mc_world::BlockStateId>>,
    passive_herd_passable: Arc<Vec<BlockStateId>>,
    passive_spawn_rules: Arc<mc_data::biomes::BiomeSpawnRules>,
    entity_types: Arc<mc_data::entity_types::EntityTypeRegistry>,
    compression: Compression,
    resources: ChunkPipelineResources,
    active_generation: Arc<AtomicU64>,
    current_tick: u64,
) -> ChunkPrepareResult {
    if !is_active_request(request, &active_generation) {
        return stale_chunk_result(request, &resources);
    }
    let loaded = match load_chunk_neighbourhood(
        Arc::clone(&world),
        world_read.clone(),
        request.chunk_x,
        request.chunk_z,
        resources.clone(),
        request,
        Arc::clone(&active_generation),
        block_light.is_some(),
    )
    .await
    {
        Ok(loaded) => loaded,
        Err(err) => {
            return ChunkPrepareResult {
                request,
                prepare_claim: None,
                fetch_ms: 0,
                pressure_flush: PressureFlushTiming::default(),
                staged: Vec::new(),
                outcome: ChunkPrepareOutcome::Failed(err),
            };
        }
    };

    if !is_active_request(request, &active_generation) {
        return stale_chunk_result(request, &resources);
    }

    let LoadedNeighbourhood {
        centre,
        neighbourhood,
        staged,
        fetch_ms,
        backpressured,
    } = loaded;

    let Some(centre) = centre else {
        if backpressured {
            let pressure_flush = match flush_dirty_chunks_for_pressure(
                Arc::clone(&world),
                request,
                current_tick,
            )
            .await
            {
                Ok(timing) => timing,
                Err(err) => {
                    warn!(cx = request.chunk_x, cz = request.chunk_z, error = %err, "dirty pressure flush failed");
                    PressureFlushTiming::default()
                }
            };
            return ChunkPrepareResult {
                request,
                prepare_claim: None,
                fetch_ms,
                pressure_flush,
                staged,
                outcome: ChunkPrepareOutcome::Backpressured,
            };
        }
        return ChunkPrepareResult {
            request,
            prepare_claim: None,
            fetch_ms,
            pressure_flush: PressureFlushTiming::default(),
            staged,
            outcome: ChunkPrepareOutcome::Absent,
        };
    };

    if backpressured {
        let pressure_flush = match flush_dirty_chunks_for_pressure(
            Arc::clone(&world),
            request,
            current_tick,
        )
        .await
        {
            Ok(timing) => timing,
            Err(err) => {
                warn!(cx = request.chunk_x, cz = request.chunk_z, error = %err, "dirty pressure flush failed");
                PressureFlushTiming::default()
            }
        };
        return ChunkPrepareResult {
            request,
            prepare_claim: None,
            fetch_ms,
            pressure_flush,
            staged,
            outcome: ChunkPrepareOutcome::Backpressured,
        };
    }

    if !is_active_request(request, &active_generation) {
        return stale_chunk_result(request, &resources);
    }

    let light_sources = (block_light.is_some() && ChunkLight::from_chunk(&centre).is_none())
        .then(|| neighbourhood.clone());

    let cpu_permit = match resources.acquire_cpu().await {
        Ok(permit) => permit,
        Err(_) => {
            return ChunkPrepareResult {
                request,
                prepare_claim: None,
                fetch_ms,
                pressure_flush: PressureFlushTiming::default(),
                staged,
                outcome: ChunkPrepareOutcome::Failed("CPU worker pool closed".into()),
            };
        }
    };

    if !is_active_request(request, &active_generation) {
        return stale_chunk_result(request, &resources);
    }

    let outcome = match tokio::task::spawn_blocking(move || {
        let _permit = cpu_permit;
        let built = if let Some(table) = block_light.as_deref() {
            CHUNK_LIGHT_WORKSPACE.with_borrow_mut(|workspace| {
                build_chunk_packet(
                    centre.as_ref(),
                    &neighbourhood,
                    biomes.as_ref(),
                    blocks.as_ref(),
                    items.as_ref(),
                    tags.as_ref(),
                    recipes.as_ref(),
                    block_entity_types.as_ref(),
                    Some(table),
                    passive_herd_surface,
                    passive_herd_fallback_surfaces.as_ref(),
                    passive_herd_water.as_slice(),
                    passive_herd_passable.as_ref(),
                    passive_spawn_rules.as_ref(),
                    entity_types.as_ref(),
                    Some(workspace),
                    request.chunk_x,
                    request.chunk_z,
                )
            })
        } else {
            build_chunk_packet(
                centre.as_ref(),
                &neighbourhood,
                biomes.as_ref(),
                blocks.as_ref(),
                items.as_ref(),
                tags.as_ref(),
                recipes.as_ref(),
                block_entity_types.as_ref(),
                None,
                passive_herd_surface,
                passive_herd_fallback_surfaces.as_ref(),
                passive_herd_water.as_slice(),
                passive_herd_passable.as_ref(),
                passive_spawn_rules.as_ref(),
                entity_types.as_ref(),
                None,
                request.chunk_x,
                request.chunk_z,
            )
        }
        .map_err(|err| err.to_string())?;
        frame_chunk_packet(built, compression).map_err(|err| err.to_string())
    })
    .await
    {
        Ok(Ok(prepared)) => ChunkPrepareOutcome::Ready(Box::new(prepared)),
        Ok(Err(err)) => ChunkPrepareOutcome::Failed(err),
        Err(err) => ChunkPrepareOutcome::Failed(err.to_string()),
    };

    if is_active_request(request, &active_generation)
        && let ChunkPrepareOutcome::Ready(prepared) = &outcome
        && let Some(light_sources) = light_sources.as_ref()
        && let Some(light) = prepared.light.as_ref()
    {
        publish_computed_light_if_sources_current(
            &world,
            world_read.as_ref(),
            world_mutation.as_ref(),
            ChunkPos {
                x: request.chunk_x,
                z: request.chunk_z,
            },
            light_sources,
            light,
        )
        .await;
    }

    if !is_active_request(request, &active_generation) {
        return stale_chunk_result(request, &resources);
    }

    ChunkPrepareResult {
        request,
        prepare_claim: None,
        fetch_ms,
        pressure_flush: PressureFlushTiming::default(),
        staged,
        outcome,
    }
}

async fn publish_computed_light_if_sources_current(
    world: &WorldHandle,
    world_read: Option<&mc_world::WorldReadView>,
    world_mutation: Option<&mc_world::WorldMutationView>,
    centre: ChunkPos,
    sources: &[[Option<Arc<Chunk>>; 3]; 3],
    light: &ChunkLight,
) -> bool {
    if let Some(world_read) = world_read {
        let Some(world_mutation) = world_mutation else {
            return false;
        };
        let positions = sources
            .iter()
            .enumerate()
            .flat_map(|(dz, row)| {
                row.iter().enumerate().map(move |(dx, _)| ChunkPos {
                    x: centre.x + dx as i32 - 1,
                    z: centre.z + dz as i32 - 1,
                })
            })
            .collect::<Vec<_>>();
        let current = world_read.snapshot_chunks(&positions);
        let mut expected_current = HashMap::with_capacity(positions.len());
        for (position, expected) in positions.into_iter().zip(sources.iter().flatten()) {
            let Some(expected) = expected else {
                return false;
            };
            let Some(current) = current.chunk(position) else {
                return false;
            };
            if current.light_source_token() != expected.light_source_token() {
                return false;
            }
            expected_current.insert(position, Some(current));
        }
        return world_mutation.publish_baked_light_conditionally(
            &expected_current,
            std::iter::once((centre, light)),
        );
    }

    let mut storage = crate::lock_metrics::timed_guard(
        crate::lock_metrics::LockMetricKind::ChunkPrepare,
        "publish computed chunk light",
        Instant::now(),
        world.lock().await,
    );
    for (dz, row) in sources.iter().enumerate() {
        for (dx, expected) in row.iter().enumerate() {
            let Some(expected) = expected else {
                return false;
            };
            let position = ChunkPos {
                x: centre.x + dx as i32 - 1,
                z: centre.z + dz as i32 - 1,
            };
            let Some(current) = storage.cached_chunk_snapshot(position) else {
                return false;
            };
            if current.light_source_token() != expected.light_source_token() {
                return false;
            }
        }
    }
    match storage.set_baked_light(centre, light) {
        Ok(published) => published,
        Err(error) => {
            warn!(error = %error, cx = centre.x, cz = centre.z, "computed chunk light publish failed");
            false
        }
    }
}

fn is_active_request(request: ChunkRequest, active_generation: &AtomicU64) -> bool {
    active_generation.load(Ordering::Acquire) == request.generation.0
}

fn stale_chunk_result(
    request: ChunkRequest,
    resources: &ChunkPipelineResources,
) -> ChunkPrepareResult {
    resources.record_stale_result_rejection();
    ChunkPrepareResult {
        request,
        prepare_claim: None,
        fetch_ms: 0,
        pressure_flush: PressureFlushTiming::default(),
        staged: Vec::new(),
        outcome: ChunkPrepareOutcome::Absent,
    }
}

async fn flush_dirty_chunks_for_pressure(
    world: WorldHandle,
    request: ChunkRequest,
    current_tick: u64,
) -> Result<PressureFlushTiming, String> {
    let _flush_guard = PRESSURE_FLUSH_COORDINATOR
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let mut timing = PressureFlushTiming::default();
    let mut stale_retries = 0usize;
    loop {
        let plan_started = Instant::now();
        let plan = {
            let storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::ChunkPrepare,
                "chunk pressure flush plan",
                Instant::now(),
                world.lock().await,
            );
            if storage.world_root().is_none() || !storage.dirty_chunk_cache_saturated() {
                return Ok(timing);
            }
            storage
                .plan_dirty_flush_at_tick(current_tick)
                .map_err(|err| err.to_string())?
        };
        timing.plan_ms += plan_started.elapsed().as_millis() as u64;
        if plan.is_empty() {
            return Ok(timing);
        }
        let planned_chunks = plan.chunk_count();
        timing.planned_chunks += planned_chunks;
        let write_started = Instant::now();
        let commit = match crate::dirty_flush::write_dirty_flush_blocking_typed(plan).await {
            Ok(commit) => commit,
            Err(err)
                if err.is_stale_region() && stale_retries < PRESSURE_FLUSH_STALE_REGION_RETRIES =>
            {
                stale_retries += 1;
                timing.runs += 1;
                timing.write_ms += write_started.elapsed().as_millis() as u64;
                continue;
            }
            Err(err) => return Err(err.to_string()),
        };
        timing.write_ms += write_started.elapsed().as_millis() as u64;
        let commit_started = Instant::now();
        let install = {
            let mut storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::ChunkPrepare,
                "chunk pressure flush install",
                Instant::now(),
                world.lock().await,
            );
            storage.install_dirty_flush(commit)
        };
        let install = match install {
            Ok(install) => install,
            Err(mc_world::WorldError::StaleRegion(_))
                if stale_retries < PRESSURE_FLUSH_STALE_REGION_RETRIES =>
            {
                stale_retries += 1;
                timing.runs += 1;
                timing.commit_ms += commit_started.elapsed().as_millis() as u64;
                continue;
            }
            Err(error) => return Err(error.to_string()),
        };
        let synced = crate::dirty_flush::sync_dirty_flush_install_blocking_typed(install)
            .await
            .map_err(|error| error.to_string())?;
        let flushed = {
            let mut storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::ChunkPrepare,
                "chunk pressure flush finalize",
                Instant::now(),
                world.lock().await,
            );
            storage.finalize_dirty_flush(synced).cleaned_chunks()
        };
        let commit_ms = commit_started.elapsed().as_millis() as u64;
        timing.runs += 1;
        timing.flushed_chunks += flushed;
        timing.commit_ms += commit_ms;
        info!(
            cx = request.chunk_x,
            cz = request.chunk_z,
            planned_chunks,
            flushed,
            plan_ms = timing.plan_ms,
            write_ms = timing.write_ms,
            commit_ms,
            stale_retries,
            "dirty pressure flush completed"
        );
        return Ok(timing);
    }
}

struct LoadedNeighbourhood {
    centre: Option<Arc<Chunk>>,
    neighbourhood: [[Option<Arc<Chunk>>; 3]; 3],
    staged: Vec<(i32, i32)>,
    fetch_ms: u64,
    backpressured: bool,
}

struct NeighbourSnapshotPlan {
    row: usize,
    column: usize,
    position: ChunkPos,
    source: NeighbourSnapshotSource,
}

enum NeighbourSnapshotSource {
    Cached(Arc<Chunk>),
    Load(mc_world::ChunkDiskLoadPlan),
}

fn plan_missing_neighbour_snapshots(
    storage: &mc_world::WorldStorage,
    centre: ChunkPos,
    neighbourhood: &[[Option<Arc<Chunk>>; 3]; 3],
) -> Vec<NeighbourSnapshotPlan> {
    let mut plans = Vec::with_capacity(8);
    for (row, chunks) in neighbourhood.iter().enumerate() {
        for (column, chunk) in chunks.iter().enumerate() {
            if (row == 1 && column == 1) || chunk.is_some() {
                continue;
            }
            let position = ChunkPos {
                x: centre.x + column as i32 - 1,
                z: centre.z + row as i32 - 1,
            };
            let source = match storage.plan_chunk_snapshot_without_generation(position) {
                mc_world::ChunkSnapshotPlan::Cached(chunk) => {
                    NeighbourSnapshotSource::Cached(chunk)
                }
                mc_world::ChunkSnapshotPlan::Load(plan) => NeighbourSnapshotSource::Load(plan),
            };
            plans.push(NeighbourSnapshotPlan {
                row,
                column,
                position,
                source,
            });
        }
    }
    plans
}

async fn chunk_prepare_can_cache(
    world: &WorldHandle,
    world_read: Option<&mc_world::WorldReadView>,
    position: ChunkPos,
    operation: &'static str,
) -> bool {
    if let Some(world_read) = world_read {
        return world_read.can_cache_new_chunk(position);
    }
    let storage = crate::lock_metrics::timed_guard(
        crate::lock_metrics::LockMetricKind::ChunkPrepare,
        operation,
        Instant::now(),
        world.lock().await,
    );
    storage.can_cache_new_chunk(position)
}

#[allow(clippy::too_many_arguments)]
async fn load_chunk_neighbourhood(
    world: WorldHandle,
    world_read: Option<mc_world::WorldReadView>,
    cx: i32,
    cz: i32,
    resources: ChunkPipelineResources,
    request: ChunkRequest,
    active_generation: Arc<AtomicU64>,
    need_full_neighbourhood: bool,
) -> Result<LoadedNeighbourhood, String> {
    if !is_active_request(request, &active_generation) {
        return Ok(LoadedNeighbourhood {
            centre: None,
            neighbourhood: std::array::from_fn(|_| std::array::from_fn(|_| None)),
            staged: Vec::new(),
            fetch_ms: 0,
            backpressured: false,
        });
    }
    let fetch_started = Instant::now();
    if let Some(world_read) = world_read.as_ref() {
        let centre_pos = ChunkPos { x: cx, z: cz };
        let mut positions = vec![centre_pos];
        if need_full_neighbourhood {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let position = ChunkPos {
                        x: cx + dx,
                        z: cz + dz,
                    };
                    if position != centre_pos {
                        positions.push(position);
                    }
                }
            }
        }
        let snapshot = world_read.snapshot_chunks(&positions);
        if let Some(centre) = snapshot.chunk(centre_pos) {
            let mut neighbourhood: [[Option<Arc<Chunk>>; 3]; 3] =
                std::array::from_fn(|_| std::array::from_fn(|_| None));
            neighbourhood[1][1] = Some(Arc::clone(&centre));
            let mut staged = vec![(cx, cz)];
            if need_full_neighbourhood {
                for (dz, row) in neighbourhood.iter_mut().enumerate() {
                    for (dx, slot) in row.iter_mut().enumerate() {
                        if dx == 1 && dz == 1 {
                            continue;
                        }
                        let ncx = cx + (dx as i32 - 1);
                        let ncz = cz + (dz as i32 - 1);
                        if let Some(chunk) = snapshot.chunk(ChunkPos { x: ncx, z: ncz }) {
                            *slot = Some(chunk);
                            staged.push((ncx, ncz));
                        }
                    }
                }
            }
            if !need_full_neighbourhood || neighbourhood.iter().flatten().all(Option::is_some) {
                return Ok(LoadedNeighbourhood {
                    centre: Some(centre),
                    neighbourhood,
                    staged,
                    fetch_ms: fetch_started.elapsed().as_millis() as u64,
                    backpressured: false,
                });
            }
        }
    }
    let mut neighbourhood: [[Option<Arc<Chunk>>; 3]; 3] =
        std::array::from_fn(|_| std::array::from_fn(|_| None));
    let mut centre = None;
    let mut staged = Vec::new();
    let mut disk_plan = None;
    let mut backpressured = false;

    let generator = {
        let storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::ChunkPrepare,
            "chunk prepare snapshot",
            Instant::now(),
            world.lock().await,
        );
        if !is_active_request(request, &active_generation) {
            return Ok(LoadedNeighbourhood {
                centre: None,
                neighbourhood,
                staged,
                fetch_ms: fetch_started.elapsed().as_millis() as u64,
                backpressured: false,
            });
        }
        match storage.plan_chunk_snapshot_without_generation(ChunkPos { x: cx, z: cz }) {
            mc_world::ChunkSnapshotPlan::Cached(chunk) => {
                centre = Some(Arc::clone(&chunk));
                neighbourhood[1][1] = Some(chunk);
                staged.push((cx, cz));
            }
            mc_world::ChunkSnapshotPlan::Load(plan) => disk_plan = Some(plan),
        }
        for (dz, row) in neighbourhood.iter_mut().enumerate() {
            for (dx, slot) in row.iter_mut().enumerate() {
                if dx == 1 && dz == 1 {
                    continue;
                }
                let ncx = cx + (dx as i32 - 1);
                let ncz = cz + (dz as i32 - 1);
                if let Some(chunk) = storage.cached_chunk_snapshot(ChunkPos { x: ncx, z: ncz }) {
                    *slot = Some(chunk);
                    staged.push((ncx, ncz));
                }
            }
        }
        storage.generator()
    };

    if centre.is_none()
        && let Some(plan) = disk_plan
    {
        match load_chunk_from_disk(
            plan,
            resources.clone(),
            request,
            Arc::clone(&active_generation),
        )
        .await
        {
            Ok(Some(chunk)) => {
                let chunk = {
                    let mut storage = crate::lock_metrics::timed_guard(
                        crate::lock_metrics::LockMetricKind::ChunkPrepare,
                        "chunk prepare disk commit",
                        Instant::now(),
                        world.lock().await,
                    );
                    if !is_active_request(request, &active_generation) {
                        return Ok(LoadedNeighbourhood {
                            centre: None,
                            neighbourhood,
                            staged,
                            fetch_ms: fetch_started.elapsed().as_millis() as u64,
                            backpressured: false,
                        });
                    }
                    storage.try_commit_chunk_snapshot(ChunkPos { x: cx, z: cz }, chunk)
                };
                match chunk {
                    Ok(Some(chunk)) => {
                        centre = Some(Arc::clone(&chunk));
                        neighbourhood[1][1] = Some(chunk);
                        staged.push((cx, cz));
                    }
                    Ok(None) => backpressured = true,
                    Err(err) => {
                        return Err(format!("chunk commit failed at ({cx},{cz}): {err}"));
                    }
                }
            }
            Ok(None) => {}
            Err(err) => return Err(format!("chunk read failed at ({cx},{cz}): {err}")),
        }
    }

    if centre.is_none()
        && !backpressured
        && let Some(generator) = generator.as_ref()
    {
        let can_cache = chunk_prepare_can_cache(
            &world,
            world_read.as_ref(),
            ChunkPos { x: cx, z: cz },
            "chunk prepare generation pressure check",
        )
        .await;
        if !can_cache {
            return Ok(LoadedNeighbourhood {
                centre: None,
                neighbourhood,
                staged,
                fetch_ms: fetch_started.elapsed().as_millis() as u64,
                backpressured: true,
            });
        }
        let chunk = match generate_fresh_chunk(
            Arc::clone(generator),
            ChunkPos { x: cx, z: cz },
            resources.clone(),
            request,
            Arc::clone(&active_generation),
        )
        .await
        {
            Ok(Some(chunk)) => chunk,
            Ok(None) => {
                return Ok(LoadedNeighbourhood {
                    centre: None,
                    neighbourhood,
                    staged,
                    fetch_ms: fetch_started.elapsed().as_millis() as u64,
                    backpressured: false,
                });
            }
            Err(err) => {
                return Err(format!("chunk generation failed at ({cx},{cz}): {err}"));
            }
        };
        let chunk = {
            let mut storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::ChunkPrepare,
                "chunk prepare commit",
                Instant::now(),
                world.lock().await,
            );
            if !is_active_request(request, &active_generation) {
                return Ok(LoadedNeighbourhood {
                    centre: None,
                    neighbourhood,
                    staged,
                    fetch_ms: fetch_started.elapsed().as_millis() as u64,
                    backpressured: false,
                });
            }
            match storage.try_insert_generated_chunk(ChunkPos { x: cx, z: cz }, chunk) {
                Ok(true) => {}
                Ok(false) => backpressured = true,
                Err(err) => {
                    return Err(format!(
                        "generated chunk insert failed at ({cx},{cz}): {err}"
                    ));
                }
            }
            storage.cached_chunk_snapshot(ChunkPos { x: cx, z: cz })
        };
        if let Some(chunk) = chunk {
            centre = Some(Arc::clone(&chunk));
            neighbourhood[1][1] = Some(chunk);
            staged.push((cx, cz));
        }
    }

    if need_full_neighbourhood
        && centre.is_some()
        && let Some(generator) = generator.as_ref()
    {
        let plans = {
            let storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::ChunkPrepare,
                "chunk prepare neighbour snapshot batch",
                Instant::now(),
                world.lock().await,
            );
            if !is_active_request(request, &active_generation) {
                return Ok(LoadedNeighbourhood {
                    centre: None,
                    neighbourhood,
                    staged,
                    fetch_ms: fetch_started.elapsed().as_millis() as u64,
                    backpressured: false,
                });
            }
            plan_missing_neighbour_snapshots(&storage, ChunkPos { x: cx, z: cz }, &neighbourhood)
        };
        for plan in plans {
            let NeighbourSnapshotPlan {
                row,
                column,
                position,
                source,
            } = plan;
            let ncx = position.x;
            let ncz = position.z;
            let disk_plan = match source {
                NeighbourSnapshotSource::Cached(chunk) => {
                    neighbourhood[row][column] = Some(chunk);
                    staged.push((ncx, ncz));
                    continue;
                }
                NeighbourSnapshotSource::Load(plan) => plan,
            };
            if let Some(chunk) = world_read
                .as_ref()
                .and_then(|world_read| world_read.snapshot_chunks(&[position]).chunk(position))
            {
                neighbourhood[row][column] = Some(chunk);
                staged.push((ncx, ncz));
                continue;
            }
            let mut chunk = match load_chunk_from_disk(
                disk_plan,
                resources.clone(),
                request,
                Arc::clone(&active_generation),
            )
            .await
            {
                Ok(Some(chunk)) => Some(chunk),
                Ok(None) => None,
                Err(err) => {
                    return Err(format!(
                        "neighbour chunk read failed at ({ncx},{ncz}): {err}"
                    ));
                }
            };
            if chunk.is_none() {
                let can_generate = chunk_prepare_can_cache(
                    &world,
                    world_read.as_ref(),
                    position,
                    "chunk prepare neighbour generation pressure check",
                )
                .await;
                if !can_generate {
                    return Ok(LoadedNeighbourhood {
                        centre,
                        neighbourhood,
                        staged,
                        fetch_ms: fetch_started.elapsed().as_millis() as u64,
                        backpressured: true,
                    });
                }
                chunk = match generate_fresh_chunk(
                    Arc::clone(generator),
                    position,
                    resources.clone(),
                    request,
                    Arc::clone(&active_generation),
                )
                .await
                {
                    Ok(chunk) => chunk,
                    Err(err) => {
                        return Err(format!(
                            "neighbour chunk generation failed at ({ncx},{ncz}): {err}"
                        ));
                    }
                };
            }
            let Some(chunk) = chunk else {
                continue;
            };
            let committed = {
                let mut storage = crate::lock_metrics::timed_guard(
                    crate::lock_metrics::LockMetricKind::ChunkPrepare,
                    "chunk prepare neighbour commit",
                    Instant::now(),
                    world.lock().await,
                );
                if !is_active_request(request, &active_generation) {
                    return Ok(LoadedNeighbourhood {
                        centre: None,
                        neighbourhood,
                        staged,
                        fetch_ms: fetch_started.elapsed().as_millis() as u64,
                        backpressured: false,
                    });
                }
                storage.try_commit_chunk_snapshot(position, chunk)
            };
            match committed {
                Ok(Some(chunk)) => {
                    neighbourhood[row][column] = Some(chunk);
                    staged.push((ncx, ncz));
                }
                Ok(None) => {
                    return Ok(LoadedNeighbourhood {
                        centre,
                        neighbourhood,
                        staged,
                        fetch_ms: fetch_started.elapsed().as_millis() as u64,
                        backpressured: true,
                    });
                }
                Err(err) => {
                    return Err(format!(
                        "neighbour chunk commit failed at ({ncx},{ncz}): {err}"
                    ));
                }
            }
        }
    }

    Ok(LoadedNeighbourhood {
        centre,
        neighbourhood,
        staged,
        fetch_ms: fetch_started.elapsed().as_millis() as u64,
        backpressured,
    })
}

async fn load_chunk_from_disk(
    plan: mc_world::ChunkDiskLoadPlan,
    resources: ChunkPipelineResources,
    request: ChunkRequest,
    active_generation: Arc<AtomicU64>,
) -> Result<Option<Chunk>, String> {
    if !is_active_request(request, &active_generation) {
        return Ok(None);
    }
    let permit = resources
        .acquire_io()
        .await
        .map_err(|_| "IO worker pool closed".to_string())?;
    if !is_active_request(request, &active_generation) {
        return Ok(None);
    }
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        plan.load()
    })
    .await
    {
        Ok(Ok(chunk)) => Ok(chunk),
        Ok(Err(err)) => Err(err.to_string()),
        Err(err) => Err(err.to_string()),
    }
}

async fn generate_fresh_chunk(
    generator: Arc<dyn mc_world::ChunkGenerator>,
    pos: ChunkPos,
    resources: ChunkPipelineResources,
    request: ChunkRequest,
    active_generation: Arc<AtomicU64>,
) -> Result<Option<Chunk>, String> {
    if !is_active_request(request, &active_generation) {
        return Ok(None);
    }
    let permit = resources
        .acquire_cpu()
        .await
        .map_err(|_| "CPU worker pool closed".to_string())?;
    if !is_active_request(request, &active_generation) {
        return Ok(None);
    }
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut chunk = generator.generate(pos);
        chunk.mark_dirty();
        chunk
    })
    .await
    {
        Ok(chunk) => Ok(Some(chunk)),
        Err(err) => Err(err.to_string()),
    }
}

struct BuiltChunkPacket {
    packet: LevelChunkWithLight,
    light: Option<ChunkLight>,
    herd_spawns: Vec<HerdSpawn>,
    hydrated_campfires: Vec<(mc_world::BlockPos, CampfireCookingState)>,
    timing: ChunkBuildTiming,
}

#[allow(clippy::too_many_arguments)]
fn build_chunk_packet(
    centre: &Chunk,
    neighbourhood: &[[Option<Arc<Chunk>>; 3]; 3],
    biomes: &Registry,
    blocks: &BlockRegistry,
    items: &ItemRegistry,
    tags: &TagsData,
    recipes: &[mc_data::recipes::Recipe],
    block_entity_types: &mc_data::block_entity_types::BlockEntityTypeRegistry,
    block_light: Option<&BlockLightTable>,
    passive_herd_surface: Option<mc_world::BlockStateId>,
    passive_herd_fallback_surfaces: &[mc_world::BlockStateId],
    passive_herd_water: &[mc_world::BlockStateId],
    passive_herd_passable: &[BlockStateId],
    passive_spawn_rules: &mc_data::biomes::BiomeSpawnRules,
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
    workspace: Option<&mut LightWorkspace>,
    cx: i32,
    cz: i32,
) -> Result<BuiltChunkPacket, mc_world::wire::WireError> {
    let mut timing = ChunkBuildTiming::default();

    let chunk_data_started = Instant::now();
    let data = encode_chunk_data(centre, biomes)?;
    timing.chunk_data_ms = chunk_data_started.elapsed().as_millis() as u64;

    let heightmap_started = Instant::now();
    let heightmaps = client_heightmaps(centre)
        .into_iter()
        .map(|h| ChunkHeightmap {
            type_id: h.type_id,
            data: h.data,
        })
        .collect();
    timing.heightmap_ms = heightmap_started.elapsed().as_millis() as u64;

    let mut computed_light = None;
    let light = if block_light.is_some() {
        if let Some(baked) = ChunkLight::from_chunk(centre) {
            let light_encode_started = Instant::now();
            let wire = encode_chunk_light(&baked);
            timing.light_encode_ms = light_encode_started.elapsed().as_millis() as u64;
            computed_light = Some(baked);
            LightData {
                sky_y_mask: wire.sky_y_mask,
                block_y_mask: wire.block_y_mask,
                empty_sky_y_mask: wire.empty_sky_y_mask,
                empty_block_y_mask: wire.empty_block_y_mask,
                sky_updates: wire.sky_updates,
                block_updates: wire.block_updates,
            }
        } else if let (Some(table), Some(ws)) = (block_light, workspace) {
            // Centre slot is the chunk we already have a reference to;
            // off-centre slots come from the staged map.
            let mut refs: [[Option<&Chunk>; 3]; 3] = [[None; 3]; 3];
            for (dz, row) in neighbourhood.iter().enumerate() {
                for (dx, slot) in row.iter().enumerate() {
                    refs[dz][dx] = slot.as_deref();
                }
            }
            refs[1][1] = Some(centre);

            let light_compute_started = Instant::now();
            let computed = compute_chunk_light_in(ws, refs, table);
            timing.light_compute_ms = light_compute_started.elapsed().as_millis() as u64;

            let light_encode_started = Instant::now();
            let wire = encode_chunk_light(&computed);
            timing.light_encode_ms = light_encode_started.elapsed().as_millis() as u64;
            computed_light = Some(computed);
            LightData {
                sky_y_mask: wire.sky_y_mask,
                block_y_mask: wire.block_y_mask,
                empty_sky_y_mask: wire.empty_sky_y_mask,
                empty_block_y_mask: wire.empty_block_y_mask,
                sky_updates: wire.sky_updates,
                block_updates: wire.block_updates,
            }
        } else {
            LightData::empty()
        }
    } else {
        LightData::empty()
    };
    let block_entities = mc_world::wire::client_block_entities(centre, blocks, items)
        .into_iter()
        .filter_map(|record| {
            let type_id = block_entity_types.id_of(&record.type_name)?;
            let packed_xz =
                ((record.pos.x.rem_euclid(16) as u8) << 4) | record.pos.z.rem_euclid(16) as u8;
            Some(BlockEntityInfo {
                packed_xz,
                y: record.pos.y.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
                type_id: i32::try_from(type_id).ok()?,
                nbt: record.nbt,
            })
        })
        .collect();
    let hydrated_campfires = campfire_cooking_states_from_chunk(centre, recipes, items, tags);

    let herd_spawns = plan_passive_herd(
        centre,
        passive_herd_surface,
        passive_herd_fallback_surfaces,
        Some(passive_herd_water),
        passive_herd_passable,
        passive_spawn_rules,
        entity_types,
    );
    Ok(BuiltChunkPacket {
        packet: LevelChunkWithLight {
            chunk_x: cx,
            chunk_z: cz,
            heightmaps,
            data,
            block_entities,
            light,
        },
        light: computed_light,
        herd_spawns,
        hydrated_campfires,
        timing,
    })
}

fn frame_chunk_packet(
    built: BuiltChunkPacket,
    compression: Compression,
) -> Result<PreparedChunkFrame, ConnectionError> {
    let mut timing = ChunkWriteTiming::default();

    let packet_encode_started = Instant::now();
    let mut body = BytesMut::new();
    built.packet.encode(&mut body)?;
    timing.packet_encode_ms = packet_encode_started.elapsed().as_millis() as u64;
    let packet_data_len = built.packet.data.len();

    let frame_started = Instant::now();
    let framed = encode_frame(LevelChunkWithLight::ID, &body, compression)?;
    timing.frame_ms = frame_started.elapsed().as_millis() as u64;
    timing.framed_bytes = framed.len();

    Ok(PreparedChunkFrame {
        frame: framed,
        light: built.light,
        herd_spawns: built.herd_spawns,
        hydrated_campfires: built.hydrated_campfires,
        packet_data_len,
        build_timing: built.timing,
        write_timing: timing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChunkPipelineGeneration;
    use mc_data::Identifier;
    use mc_data::blocks::{BlockReport, BlockStateReport};
    use mc_protocol::frame::Compression;
    use mc_world::{BlockStateId, ChunkGenerator, ChunkPos, WorldStorage};
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use tokio::sync::Mutex;

    fn canonical_entity_type_report() -> Vec<mc_data::entity_types::EntityTypeReport> {
        (0..mc_data::entity_types::ENTITY_TYPE_COUNT as u32)
            .map(|protocol_id| {
                let contract =
                    mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(protocol_id)
                        .expect("canonical 26.1.2 entity contract is dense");
                mc_data::entity_types::EntityTypeReport {
                    id: Identifier::parse(contract.name)
                        .expect("canonical entity contract name is a valid identifier"),
                    protocol_id,
                }
            })
            .collect()
    }

    fn healthy_runtime_control_input() -> crate::RuntimeControlInput {
        crate::RuntimeControlInput {
            tick_ms: 0,
            memory_used_mb: 0,
            memory_limit_mb: 0,
        }
    }

    async fn observe_next_runtime_control_signal(
        control: &crate::RuntimeControlHandle,
        signals: &mut crate::control_plane::RuntimeControlSignalReceiver,
    ) -> crate::AutoscaleDecision {
        let signal = signals
            .recv()
            .await
            .expect("runtime control signal arrives");
        control.observe_signal(signal)
    }

    struct InvalidatePreparedOnWrite {
        sessions: Arc<SessionRegistry>,
        chunk: (i32, i32),
        written: usize,
    }

    impl tokio::io::AsyncWrite for InvalidatePreparedOnWrite {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            bytes: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            let writer = self.get_mut();
            if writer.written == 0 {
                writer
                    .sessions
                    .invalidate_prepared_chunks(&HashSet::from([writer.chunk]));
            }
            writer.written += bytes.len();
            std::task::Poll::Ready(Ok(bytes.len()))
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    struct CountingGenerator {
        calls: Arc<AtomicUsize>,
    }

    impl ChunkGenerator for CountingGenerator {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            self.calls.fetch_add(1, Ordering::AcqRel);
            Chunk::empty(
                pos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            )
        }
    }

    struct GenerationGate {
        started: tokio::sync::Notify,
        released: std::sync::Mutex<bool>,
        released_cv: std::sync::Condvar,
    }

    impl GenerationGate {
        fn new() -> Self {
            Self {
                started: tokio::sync::Notify::new(),
                released: std::sync::Mutex::new(false),
                released_cv: std::sync::Condvar::new(),
            }
        }

        async fn wait_started(&self) {
            self.started.notified().await;
        }

        fn block_until_released(&self) {
            self.started.notify_one();
            let mut released = self.released.lock().unwrap();
            while !*released {
                released = self.released_cv.wait(released).unwrap();
            }
        }

        fn release(&self) {
            *self.released.lock().unwrap() = true;
            self.released_cv.notify_all();
        }
    }

    struct CountingConcurrentGenerator {
        calls: Arc<AtomicUsize>,
        active: Arc<AtomicUsize>,
        max_active: Arc<AtomicUsize>,
        concurrent_call_started: Option<Arc<tokio::sync::Notify>>,
        first_call_gate: Option<Arc<GenerationGate>>,
        gate_first_call: AtomicBool,
    }

    impl ChunkGenerator for CountingConcurrentGenerator {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            self.calls.fetch_add(1, Ordering::AcqRel);
            let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
            self.max_active.fetch_max(active, Ordering::AcqRel);
            if active > 1
                && let Some(concurrent_call_started) = self.concurrent_call_started.as_ref()
            {
                concurrent_call_started.notify_one();
            }
            if self.gate_first_call.swap(false, Ordering::AcqRel)
                && let Some(gate) = self.first_call_gate.as_ref()
            {
                gate.block_until_released();
            }
            self.active.fetch_sub(1, Ordering::AcqRel);
            Chunk::empty(
                pos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            )
        }
    }

    fn test_biome_registry() -> Registry {
        Registry {
            id: Identifier::parse("minecraft:worldgen/biome").unwrap(),
            entries: vec![Identifier::parse("minecraft:plains").unwrap()],
        }
    }

    async fn drive_stream_to_completion<W>(
        stream: &mut ChunkStreamState,
        writer: &mut W,
        light_cache: &mut LightCache,
        timeout: Duration,
        failure: &'static str,
    ) where
        W: AsyncWriteExt + Unpin,
    {
        let progress_notify = stream.progress_notify();
        tokio::time::timeout(timeout, async {
            loop {
                let progress = progress_notify.notified();
                tokio::pin!(progress);
                progress.as_mut().enable();
                if stream.step(writer, light_cache).await.unwrap() == ChunkStreamStep::Complete {
                    return;
                }
                if !stream.has_immediate_work() {
                    progress.await;
                }
            }
        })
        .await
        .expect(failure);
    }

    fn air_block_registry() -> BlockRegistry {
        BlockRegistry::from_report(&[BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 0,
                default: true,
                properties: BTreeMap::new(),
            }],
        }])
        .expect("air registry builds")
    }

    #[test]
    fn herd_surface_prefers_grass_before_fallbacks() {
        let plains = Identifier::parse("minecraft:plains").unwrap();
        let air = BlockStateId(0);
        let grass = BlockStateId(1);
        let dirt = BlockStateId(2);
        let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, air, plains);
        chunk.set_block(8, 63, 8, dirt).unwrap();
        chunk.set_block(8, 64, 8, grass).unwrap();

        assert_eq!(
            herd_surface_y(&chunk, 8, 8, grass, &[dirt], &[air]),
            Some((64, grass))
        );
    }

    #[test]
    fn herd_surface_fallback_allows_explicit_generated_land() {
        let plains = Identifier::parse("minecraft:plains").unwrap();
        let air = BlockStateId(0);
        let grass = BlockStateId(1);
        let dirt = BlockStateId(2);
        let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, air, plains);
        chunk.set_block(8, 64, 8, dirt).unwrap();
        chunk
            .highest_opaque
            .set(8, 8, (64 - mc_world::MIN_Y + 1) as u32);

        assert_eq!(
            herd_surface_y(&chunk, 8, 8, grass, &[dirt], &[air]),
            Some((64, dirt))
        );
    }

    #[test]
    fn herd_surface_fallback_rejects_unnatural_tops() {
        let plains = Identifier::parse("minecraft:plains").unwrap();
        let air = BlockStateId(0);
        let grass = BlockStateId(1);
        let dirt = BlockStateId(2);
        for rejected in [BlockStateId(3), BlockStateId(4), BlockStateId(5)] {
            let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, air, plains.clone());
            chunk.set_block(8, 64, 8, rejected).unwrap();
            chunk
                .highest_opaque
                .set(8, 8, (64 - mc_world::MIN_Y + 1) as u32);

            assert_eq!(herd_surface_y(&chunk, 8, 8, grass, &[dirt], &[air]), None);
        }
        assert!(!passive_herd_fallback_surface_name("minecraft:stone"));
        assert!(!passive_herd_fallback_surface_name("minecraft:oak_planks"));
        assert!(!passive_herd_fallback_surface_name("minecraft:oak_leaves"));
    }

    #[test]
    fn herd_surface_fallback_names_cover_natural_generated_land() {
        for name in [
            "minecraft:dirt",
            "minecraft:coarse_dirt",
            "minecraft:podzol",
            "minecraft:sand",
            "minecraft:red_sand",
            "minecraft:snow_block",
            "minecraft:moss_block",
            "minecraft:mycelium",
        ] {
            assert!(passive_herd_fallback_surface_name(name));
        }
    }

    #[test]
    fn chunk_biome_lookup_uses_custom_geometry_and_rejects_out_of_bounds_y() {
        let plains = Identifier::parse("minecraft:plains").unwrap();
        let bottom = Identifier::parse("minecraft:desert").unwrap();
        let top = Identifier::parse("minecraft:snowy_plains").unwrap();
        let geometry = mc_world::ChunkGeometry::new(0, 256).unwrap();
        let mut chunk =
            Chunk::empty_with_geometry(ChunkPos { x: 0, z: 0 }, BlockStateId(0), plains, geometry);
        chunk.biomes[0] = mc_world::BiomeSection::filled(bottom.clone());
        chunk.biomes[geometry.section_count() - 1] = mc_world::BiomeSection::filled(top.clone());

        assert_eq!(chunk_biome_at(&chunk, 0, -1, 0), None);
        assert_eq!(chunk_biome_at(&chunk, 0, 0, 0), Some(&bottom));
        assert_eq!(chunk_biome_at(&chunk, 15, 255, 15), Some(&top));
        assert_eq!(chunk_biome_at(&chunk, 15, 256, 15), None);
    }

    #[test]
    fn chunk_biome_lookup_preserves_overworld_section_mapping() {
        let plains = Identifier::parse("minecraft:plains").unwrap();
        let bottom = Identifier::parse("minecraft:desert").unwrap();
        let top = Identifier::parse("minecraft:snowy_plains").unwrap();
        let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), plains);
        let section_count = chunk.geometry().section_count();
        chunk.biomes[0] = mc_world::BiomeSection::filled(bottom.clone());
        chunk.biomes[section_count - 1] = mc_world::BiomeSection::filled(top.clone());

        assert_eq!(
            chunk_biome_at(&chunk, 0, chunk.geometry().min_y(), 0),
            Some(&bottom)
        );
        assert_eq!(
            chunk_biome_at(&chunk, 15, chunk.geometry().max_y() - 1, 15),
            Some(&top)
        );
    }

    #[test]
    fn natural_sheep_color_weights_match_vanilla_26_1_2() {
        use mc_data::biomes::SheepColorClimate;
        use mc_entity::SheepColor;

        fn counts(climate: SheepColorClimate) -> [usize; 16] {
            let mut counts = [0; 16];
            for outer_roll in 0..100 {
                for common_roll in 0..500 {
                    let color = sheep_color_for_rolls(climate, outer_roll, common_roll);
                    counts[usize::from(color.id())] += 1;
                }
            }
            counts
        }

        let temperate = counts(SheepColorClimate::Temperate);
        assert_eq!(temperate[usize::from(SheepColor::White.id())], 40_918);
        assert_eq!(temperate[usize::from(SheepColor::Pink.id())], 82);
        assert_eq!(temperate[usize::from(SheepColor::LightGray.id())], 2_500);
        assert_eq!(temperate[usize::from(SheepColor::Gray.id())], 2_500);
        assert_eq!(temperate[usize::from(SheepColor::Brown.id())], 1_500);
        assert_eq!(temperate[usize::from(SheepColor::Black.id())], 2_500);

        let warm = counts(SheepColorClimate::Warm);
        assert_eq!(warm[usize::from(SheepColor::White.id())], 2_500);
        assert_eq!(warm[usize::from(SheepColor::Pink.id())], 82);
        assert_eq!(warm[usize::from(SheepColor::LightGray.id())], 2_500);
        assert_eq!(warm[usize::from(SheepColor::Gray.id())], 2_500);
        assert_eq!(warm[usize::from(SheepColor::Brown.id())], 40_918);
        assert_eq!(warm[usize::from(SheepColor::Black.id())], 1_500);

        let cold = counts(SheepColorClimate::Cold);
        assert_eq!(cold[usize::from(SheepColor::White.id())], 2_500);
        assert_eq!(cold[usize::from(SheepColor::Pink.id())], 82);
        assert_eq!(cold[usize::from(SheepColor::LightGray.id())], 2_500);
        assert_eq!(cold[usize::from(SheepColor::Gray.id())], 2_500);
        assert_eq!(cold[usize::from(SheepColor::Brown.id())], 1_500);
        assert_eq!(cold[usize::from(SheepColor::Black.id())], 40_918);
    }

    #[test]
    fn passive_sheep_plan_carries_biome_color_into_each_spawn() {
        let desert = Identifier::parse("minecraft:desert").unwrap();
        let sheep = Identifier::parse("minecraft:sheep").unwrap();
        let rules = mc_data::biomes::BiomeSpawnRules::from_entries_with_sheep_color_climates(
            BTreeMap::from([(
                desert.clone(),
                BTreeMap::from([(
                    "creature".to_string(),
                    vec![mc_data::biomes::BiomeSpawnEntry {
                        entity_type: sheep.clone(),
                        min_count: 4,
                        max_count: 4,
                        weight: 1,
                    }],
                )]),
            )]),
            BTreeSet::from([desert.clone()]),
            BTreeSet::new(),
        );
        let entity_types = mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(
            &canonical_entity_type_report(),
        )
        .expect("canonical exact 26.1.2 entity type report builds");
        let air = BlockStateId(0);
        let grass = BlockStateId(1);
        let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, air, desert.clone());
        for lx in 3..=12 {
            for lz in 3..=12 {
                chunk.set_block(lx, 64, lz, grass).unwrap();
            }
        }
        let mut spawns = Vec::new();

        plan_group_spawns(
            &chunk,
            LandSpawnSurfaces {
                preferred: grass,
                fallbacks: &[],
            },
            &[air],
            "creature",
            &rules,
            &entity_types,
            &mut spawns,
        );

        assert_eq!(spawns.len(), 4);
        for spawn in spawns {
            assert_eq!(
                spawn.sheep_color,
                Some(natural_sheep_color(
                    rules.sheep_color_climate(&desert),
                    spawn.chunk,
                    spawn.slot,
                ))
            );
        }
    }

    #[test]
    fn prepared_cache_hit_drops_historical_cpu_and_encode_timings() {
        let prepared = PreparedChunkFrame {
            frame: Bytes::from_static(b"chunk-frame"),
            light: None,
            herd_spawns: Vec::new(),
            hydrated_campfires: Vec::new(),
            packet_data_len: 123,
            build_timing: ChunkBuildTiming {
                chunk_data_ms: 10,
                heightmap_ms: 11,
                light_compute_ms: 12,
                light_encode_ms: 13,
            },
            write_timing: ChunkWriteTiming {
                packet_encode_ms: 14,
                frame_ms: 15,
                socket_write_ms: 16,
                framed_bytes: 17,
            },
        };

        let cached = prepared.prepared_cache_hit();

        assert_eq!(cached.frame, prepared.frame);
        assert_eq!(cached.packet_data_len, prepared.packet_data_len);
        assert_eq!(cached.build_timing.chunk_data_ms, 0);
        assert_eq!(cached.build_timing.heightmap_ms, 0);
        assert_eq!(cached.build_timing.light_compute_ms, 0);
        assert_eq!(cached.build_timing.light_encode_ms, 0);
        assert_eq!(cached.write_timing.packet_encode_ms, 0);
        assert_eq!(cached.write_timing.frame_ms, 0);
        assert_eq!(cached.write_timing.socket_write_ms, 0);
        assert_eq!(cached.write_timing.framed_bytes, 17);
    }

    #[tokio::test]
    async fn disabled_monsters_filters_cached_spawn_plan_before_publication() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let sessions = Arc::new(SessionRegistry::new());
        let (tx, _rx) = mpsc::channel(8);
        let desired = desired_chunk_set(0, 0, 0);
        let (session_id, _) = sessions.register(
            &LoggedInProfile {
                uuid: uuid::Uuid::nil(),
                name: "spawn-policy-player".to_owned(),
            },
            (0, 0),
            0,
            desired,
            tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::clone(&sessions),
            session_id,
            0,
            0,
            0.0,
            0,
            ChunkPipelineResources::with_limits(1, 1),
            ChunkPipelinePolicy::default(),
        )
        .with_spawn_monsters(false);
        let request = stream.scheduler.poll_next().expect("chunk request");
        stream.accept_result(ChunkPrepareResult {
            request,
            prepare_claim: None,
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming::default(),
            staged: Vec::new(),
            outcome: ChunkPrepareOutcome::Ready(Box::new(PreparedChunkFrame {
                frame: Bytes::from_static(b"chunk-frame"),
                light: None,
                herd_spawns: vec![
                    HerdSpawn {
                        chunk: (0, 0),
                        slot: 0,
                        entity_type_id: 1,
                        entity_type_name: "minecraft:cow".to_owned(),
                        position: Vec3::new(1.5, 64.0, 1.5),
                        hostile: false,
                        sheep_color: None,
                    },
                    HerdSpawn {
                        chunk: (0, 0),
                        slot: 1,
                        entity_type_id: 2,
                        entity_type_name: "minecraft:zombie".to_owned(),
                        position: Vec3::new(2.5, 64.0, 2.5),
                        hostile: true,
                        sheep_color: None,
                    },
                ],
                hydrated_campfires: Vec::new(),
                packet_data_len: 0,
                build_timing: ChunkBuildTiming::default(),
                write_timing: ChunkWriteTiming::default(),
            })),
        });
        let mut writer = tokio::io::sink();
        let mut light_cache = LightCache::new();

        assert_eq!(
            stream
                .emit_next_ready(&mut writer, &mut light_cache)
                .await
                .unwrap(),
            EmitReadyResult::SentPacket
        );
        let entities = sessions.persisted_entity_records();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].snapshot.type_name, "minecraft:cow");
        sessions.set_world_time_and_update_sleep(NIGHT_START_TICK);
        let entities_after_night = sessions.persisted_entity_records();
        assert_eq!(entities_after_night.len(), 1);
        assert_eq!(entities_after_night[0].snapshot.type_name, "minecraft:cow");
    }

    #[tokio::test]
    async fn dispatch_defers_globally_in_flight_chunk_until_cache_lands() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let sessions = Arc::new(SessionRegistry::new());
        let (tx, _rx) = mpsc::channel(1);
        let profile = LoggedInProfile {
            uuid: uuid::Uuid::nil(),
            name: "claim-waiter".to_string(),
        };
        let desired = desired_chunk_set(0, 0, 0);
        let (session_id, _) = sessions.register(
            &profile,
            (0, 0),
            0,
            desired,
            tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        let policy = ChunkPipelinePolicy {
            chunk_prepare_batch_size: 1,
            chunk_result_queue_size: 1,
            ..ChunkPipelinePolicy::default()
        };
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::clone(&sessions),
            session_id,
            0,
            0,
            0.0,
            0,
            ChunkPipelineResources::with_limits(1, 1),
            policy,
        );
        let claim = match sessions.prepared_chunk_or_claim((0, 0)) {
            PreparedChunkClaimResult::Claimed(claim) => claim,
            other => panic!("expected manual claim, got {other:?}"),
        };

        stream.dispatch_available().await;

        assert_eq!(stream.dispatched, 0);
        assert_eq!(stream.ready.len(), 0);
        assert_eq!(stream.scheduler.in_flight_len(), 0);
        assert_eq!(stream.scheduler.queued_len(), 1);

        sessions.cache_prepared_chunk(
            (0, 0),
            Arc::new(PreparedChunkFrame {
                frame: Bytes::from_static(b"chunk-frame"),
                light: None,
                herd_spawns: Vec::new(),
                hydrated_campfires: Vec::new(),
                packet_data_len: 0,
                build_timing: ChunkBuildTiming::default(),
                write_timing: ChunkWriteTiming::default(),
            }),
        );
        assert!(sessions.release_prepared_chunk_claim((0, 0), claim));

        stream.dispatch_available().await;

        assert_eq!(stream.dispatched, 1);
        assert_eq!(stream.ready.len(), 1);
        assert_eq!(stream.scheduler.in_flight_len(), 1);
    }

    #[tokio::test]
    async fn same_spawn_waiters_do_not_rescan_full_inflight_window() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let sessions = Arc::new(SessionRegistry::new());
        let view_distance = 8;
        let waiter_count = 20usize;
        let desired = desired_chunk_set(0, 0, view_distance);
        assert_eq!(desired.len(), 289);

        let policy = ChunkPipelinePolicy {
            chunk_prepare_batch_size: desired.len(),
            chunk_result_queue_size: 1,
            ..ChunkPipelinePolicy::default()
        };
        let mut streams = Vec::with_capacity(waiter_count);
        for waiter in 0..waiter_count {
            let (tx, _rx) = mpsc::channel(1);
            let profile = LoggedInProfile {
                uuid: uuid::Uuid::from_u128(waiter as u128 + 1),
                name: format!("claim-waiter-{waiter}"),
            };
            let (session_id, _) = sessions.register(
                &profile,
                (0, 0),
                view_distance,
                desired.clone(),
                tx,
                PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
            );
            streams.push(ChunkStreamState::new(
                Arc::clone(&world),
                Arc::new(test_biome_registry()),
                Arc::clone(&registry),
                None,
                Arc::new(ItemRegistry::from_report(&[])),
                Arc::new(TagsData::default()),
                Arc::new(Vec::new()),
                Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
                None,
                Arc::new(Vec::new()),
                Arc::new(Vec::new()),
                Arc::new(Vec::new()),
                Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
                Arc::new(mc_data::entity_types::solaris_required_entity_types()),
                Compression::Disabled,
                Arc::clone(&sessions),
                session_id,
                0,
                0,
                0.0,
                view_distance,
                ChunkPipelineResources::with_limits(1, 1),
                policy,
            ));
        }

        for &chunk in &desired {
            match sessions.prepared_chunk_or_claim(chunk) {
                PreparedChunkClaimResult::Claimed(_) => {}
                other => panic!("expected manual prepared claim for {chunk:?}, got {other:?}"),
            }
        }

        let before = sessions.prepared_chunk_claim_call_count();
        for stream in &mut streams {
            stream.dispatch_available().await;
            assert_eq!(stream.dispatched, 0);
            assert_eq!(stream.ready.len(), 0);
            assert_eq!(stream.scheduler.in_flight_len(), 0);
            assert_eq!(stream.scheduler.queued_len(), desired.len());
        }
        let after = sessions.prepared_chunk_claim_call_count();
        let delta = after.saturating_sub(before);
        let full_rescan_probes = waiter_count as u64 * desired.len() as u64;

        assert!(
            delta <= waiter_count as u64 * 2,
            "same-spawn waiters should stop after bounded in-flight probes; \
             claim_probe_count delta={delta}, bounded={} full_rescan={full_rescan_probes}",
            waiter_count * 2
        );
    }

    #[tokio::test]
    async fn mixed_inflight_prefix_rotates_to_later_cached_chunk() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let sessions = Arc::new(SessionRegistry::new());
        let (tx, _rx) = mpsc::channel(1);
        let profile = LoggedInProfile {
            uuid: uuid::Uuid::nil(),
            name: "mixed-claim-waiter".to_string(),
        };
        let view_distance = 1;
        let desired = desired_chunk_set(0, 0, view_distance);
        let (session_id, _) = sessions.register(
            &profile,
            (0, 0),
            view_distance,
            desired,
            tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        let policy = ChunkPipelinePolicy {
            chunk_prepare_batch_size: 1,
            chunk_result_queue_size: 1,
            ..ChunkPipelinePolicy::default()
        };
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::clone(&sessions),
            session_id,
            0,
            0,
            0.0,
            view_distance,
            ChunkPipelineResources::with_limits(1, 1),
            policy,
        );
        let queued: Vec<_> = prioritized_spiral(0, 0, view_distance, 0.0)
            .take(3)
            .map(|(cx, cz, _)| (cx, cz))
            .collect();
        let first_inflight = queued[0];
        let second_inflight = queued[1];
        let cached_later = queued[2];
        let first_claim = match sessions.prepared_chunk_or_claim(first_inflight) {
            PreparedChunkClaimResult::Claimed(claim) => claim,
            other => panic!("expected first manual claim, got {other:?}"),
        };
        let second_claim = match sessions.prepared_chunk_or_claim(second_inflight) {
            PreparedChunkClaimResult::Claimed(claim) => claim,
            other => panic!("expected second manual claim, got {other:?}"),
        };
        sessions.cache_prepared_chunk(
            cached_later,
            Arc::new(PreparedChunkFrame {
                frame: Bytes::from_static(b"chunk-frame"),
                light: None,
                herd_spawns: Vec::new(),
                hydrated_campfires: Vec::new(),
                packet_data_len: 0,
                build_timing: ChunkBuildTiming::default(),
                write_timing: ChunkWriteTiming::default(),
            }),
        );

        for _ in 0..=PREPARED_IN_FLIGHT_DEFERRAL_LIMIT {
            stream.dispatch_available().await;
            if stream
                .ready
                .values()
                .any(|result| (result.request.chunk_x, result.request.chunk_z) == cached_later)
            {
                break;
            }
        }

        let ready_chunks: Vec<_> = stream
            .ready
            .values()
            .map(|result| (result.request.chunk_x, result.request.chunk_z))
            .collect();
        assert_eq!(ready_chunks, vec![cached_later]);
        assert_eq!(stream.dispatched, 1);
        assert_eq!(stream.scheduler.in_flight_len(), 1);
        assert!(sessions.release_prepared_chunk_claim(first_inflight, first_claim));
        assert!(sessions.release_prepared_chunk_claim(second_inflight, second_claim));
    }

    #[tokio::test]
    async fn stale_prepare_result_releases_global_claim() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let sessions = Arc::new(SessionRegistry::new());
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::clone(&sessions),
            1,
            0,
            0,
            0.0,
            0,
            ChunkPipelineResources::with_limits(1, 1),
            ChunkPipelinePolicy::default(),
        );
        let claim = match sessions.prepared_chunk_or_claim((0, 0)) {
            PreparedChunkClaimResult::Claimed(claim) => claim,
            other => panic!("expected manual claim, got {other:?}"),
        };
        let stale_request = ChunkRequest {
            chunk_x: 0,
            chunk_z: 0,
            priority: ChunkPriority {
                ring: 0,
                sequence: 0,
            },
            generation: ChunkPipelineGeneration(stream.scheduler.current_generation().0 + 1),
        };

        stream.accept_result(ChunkPrepareResult {
            request: stale_request,
            prepare_claim: Some(PreparedChunkFence::Claimed(claim)),
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming::default(),
            staged: Vec::new(),
            outcome: ChunkPrepareOutcome::Absent,
        });

        let replacement = match sessions.prepared_chunk_or_claim((0, 0)) {
            PreparedChunkClaimResult::Claimed(claim) => claim,
            other => panic!("expected released stale claim, got {other:?}"),
        };
        assert!(sessions.release_prepared_chunk_claim((0, 0), replacement));
    }

    #[tokio::test]
    async fn invalidation_during_socket_write_requeues_prepared_result() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let sessions = Arc::new(SessionRegistry::new());
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::clone(&sessions),
            1,
            0,
            0,
            0.0,
            0,
            ChunkPipelineResources::with_limits(1, 1),
            ChunkPipelinePolicy::default(),
        );
        let request = stream.scheduler.poll_next().expect("chunk request");
        let claim = match sessions.prepared_chunk_or_claim((0, 0)) {
            PreparedChunkClaimResult::Claimed(claim) => claim,
            other => panic!("expected manual claim, got {other:?}"),
        };
        stream.accept_result(ChunkPrepareResult {
            request,
            prepare_claim: Some(PreparedChunkFence::Claimed(claim)),
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming::default(),
            staged: Vec::new(),
            outcome: ChunkPrepareOutcome::Ready(Box::new(PreparedChunkFrame {
                frame: Bytes::from_static(b"stale-frame"),
                light: None,
                herd_spawns: Vec::new(),
                hydrated_campfires: Vec::new(),
                packet_data_len: 0,
                build_timing: ChunkBuildTiming::default(),
                write_timing: ChunkWriteTiming::default(),
            })),
        });
        let mut writer = InvalidatePreparedOnWrite {
            sessions: Arc::clone(&sessions),
            chunk: (0, 0),
            written: 0,
        };
        let mut light_cache = LightCache::new();

        assert_eq!(
            stream
                .emit_next_ready(&mut writer, &mut light_cache)
                .await
                .unwrap(),
            EmitReadyResult::Blocked
        );
        assert_eq!(writer.written, b"stale-frame".len());
        assert_eq!(stream.emitted, 0);
        assert!(!stream.loaded.contains(&(0, 0)));
        assert_eq!(stream.scheduler.queued_len(), 1);
    }

    #[tokio::test]
    async fn dropping_stream_releases_ready_prepare_claim() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let sessions = Arc::new(SessionRegistry::new());
        let resources = ChunkPipelineResources::with_limits(1, 1);
        let metrics = resources.metrics();
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::clone(&sessions),
            1,
            0,
            0,
            0.0,
            0,
            resources,
            ChunkPipelinePolicy::default(),
        );
        let request = stream.scheduler.poll_next().expect("chunk request");
        let claim = match sessions.prepared_chunk_or_claim((0, 0)) {
            PreparedChunkClaimResult::Claimed(claim) => claim,
            other => panic!("expected manual claim, got {other:?}"),
        };
        stream.accept_result(ChunkPrepareResult {
            request,
            prepare_claim: Some(PreparedChunkFence::Claimed(claim)),
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming::default(),
            staged: Vec::new(),
            outcome: ChunkPrepareOutcome::Absent,
        });
        assert!(matches!(
            sessions.prepared_chunk_or_claim((0, 0)),
            PreparedChunkClaimResult::InFlight
        ));

        drop(stream);

        let cancellation = metrics.cancellation_snapshot();
        assert_eq!(cancellation.cancelled_streams, 1);
        assert_eq!(cancellation.cancelled_requests, 1);

        let replacement = match sessions.prepared_chunk_or_claim((0, 0)) {
            PreparedChunkClaimResult::Claimed(claim) => claim,
            other => panic!("expected released ready claim, got {other:?}"),
        };
        assert!(sessions.release_prepared_chunk_claim((0, 0), replacement));
    }

    #[tokio::test]
    async fn stale_prepare_request_records_rejection_before_world_work() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let resources = ChunkPipelineResources::with_limits(1, 1);
        let metrics = resources.metrics();
        let request = ChunkRequest {
            chunk_x: 0,
            chunk_z: 0,
            priority: ChunkPriority {
                ring: 0,
                sequence: 0,
            },
            generation: ChunkPipelineGeneration(1),
        };

        let result = prepare_chunk_request(
            request,
            Arc::clone(&world),
            None,
            None,
            Arc::new(test_biome_registry()),
            registry,
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            resources,
            Arc::new(AtomicU64::new(2)),
            0,
        )
        .await;

        assert!(matches!(result.outcome, ChunkPrepareOutcome::Absent));
        assert_eq!(
            metrics.cancellation_snapshot(),
            crate::ChunkPipelineCancellationSnapshot {
                stale_results_rejected: 1,
                ..crate::ChunkPipelineCancellationSnapshot::default()
            }
        );
        assert_eq!(world.lock().await.cache_len(), 0);
    }

    #[test]
    fn build_chunk_packet_uses_baked_section_light_without_recompute() {
        let plains = Identifier::parse("minecraft:plains").unwrap();
        let mut centre = Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), plains);
        centre.set_block(0, 0, 0, BlockStateId(1)).unwrap();
        let mut sky = vec![0; mc_world::chunk::LIGHT_LAYER_BYTES];
        let mut block = vec![0; mc_world::chunk::LIGHT_LAYER_BYTES];
        sky[0] = 0x21;
        block[0] = 0x43;
        centre.section_lights[0].sky = Some(sky.clone());
        centre.section_lights[0].block = Some(block.clone());
        let neighbourhood: [[Option<Arc<Chunk>>; 3]; 3] =
            std::array::from_fn(|_| std::array::from_fn(|_| None));
        let table =
            BlockLightTable::from_arrays("test", vec![0, 0], vec![0, 15], vec![true, false]);
        let mut workspace = LightWorkspace::new();

        let built = build_chunk_packet(
            &centre,
            &neighbourhood,
            &test_biome_registry(),
            &air_block_registry(),
            &ItemRegistry::from_report(&[]),
            &TagsData::default(),
            &[],
            &mc_data::block_entity_types::BlockEntityTypeRegistry::default(),
            Some(&table),
            None,
            &[],
            &[],
            &[],
            &mc_data::biomes::BiomeSpawnRules::default(),
            &mc_data::entity_types::solaris_required_entity_types(),
            Some(&mut workspace),
            0,
            0,
        )
        .expect("chunk packet builds from baked light");

        let light = built
            .light
            .expect("baked light should populate the play light cache");
        assert_eq!(built.timing.light_compute_ms, 0);
        assert_eq!(light.sky.section(0).unwrap()[0], 0x21);
        assert_eq!(light.block.section(0).unwrap()[0], 0x43);
    }

    #[tokio::test]
    async fn prepared_computed_light_is_published_for_later_readers() {
        let registry = Arc::new(air_block_registry());
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 16);
        for z in -1..=1 {
            for x in -1..=1 {
                let pos = ChunkPos { x, z };
                storage
                    .insert_generated_chunk(pos, Chunk::empty(pos, BlockStateId(0), biome.clone()))
                    .unwrap();
            }
        }
        let world_read = storage.read_view();
        let world_mutation = storage.mutation_view();
        let world = Arc::new(Mutex::new(storage));
        let request = ChunkRequest {
            chunk_x: 0,
            chunk_z: 0,
            priority: ChunkPriority {
                ring: 0,
                sequence: 0,
            },
            generation: ChunkPipelineGeneration(1),
        };

        let result = prepare_chunk_request(
            request,
            Arc::clone(&world),
            Some(world_read.clone()),
            Some(world_mutation),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            Some(Arc::new(BlockLightTable::from_arrays(
                "test",
                vec![0],
                vec![0],
                vec![true],
            ))),
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            ChunkPipelineResources::with_limits(1, 1),
            Arc::new(AtomicU64::new(1)),
            0,
        )
        .await;

        assert!(matches!(result.outcome, ChunkPrepareOutcome::Ready(_)));
        let published = world_read
            .snapshot_chunks(&[ChunkPos { x: 0, z: 0 }])
            .chunk(ChunkPos { x: 0, z: 0 })
            .expect("prepared centre remains published");
        assert!(
            ChunkLight::from_section_lights(&published.section_lights).is_some(),
            "computed light must become the shared baked snapshot"
        );
    }

    #[tokio::test]
    async fn computed_light_publish_allows_light_only_changes_but_rejects_blocks() {
        let registry = Arc::new(air_block_registry());
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 16);
        let mut positions = Vec::new();
        for z in -1..=1 {
            for x in -1..=1 {
                let pos = ChunkPos { x, z };
                positions.push(pos);
                storage
                    .insert_generated_chunk(pos, Chunk::empty(pos, BlockStateId(0), biome.clone()))
                    .unwrap();
            }
        }
        let world_read = storage.read_view();
        let world_mutation = storage.mutation_view();
        let snapshot = world_read.snapshot_chunks(&positions);
        let sources = std::array::from_fn(|dz| {
            std::array::from_fn(|dx| {
                snapshot.chunk(ChunkPos {
                    x: dx as i32 - 1,
                    z: dz as i32 - 1,
                })
            })
        });
        let world = Arc::new(Mutex::new(storage));

        world
            .lock()
            .await
            .set_baked_light(ChunkPos { x: 1, z: 0 }, &ChunkLight::filled(15, 0))
            .unwrap();

        assert!(
            publish_computed_light_if_sources_current(
                &world,
                Some(&world_read),
                Some(&world_mutation),
                ChunkPos { x: 0, z: 0 },
                &sources,
                &ChunkLight::filled(15, 0),
            )
            .await
        );
        let centre = world_read
            .snapshot_chunks(&[ChunkPos { x: 0, z: 0 }])
            .chunk(ChunkPos { x: 0, z: 0 })
            .expect("centre remains published");
        assert!(ChunkLight::from_section_lights(&centre.section_lights).is_some());

        let snapshot = world_read.snapshot_chunks(&positions);
        let block_sources = std::array::from_fn(|dz| {
            std::array::from_fn(|dx| {
                snapshot.chunk(ChunkPos {
                    x: dx as i32 - 1,
                    z: dz as i32 - 1,
                })
            })
        });
        world
            .lock()
            .await
            .set_block_at(mc_world::BlockPos { x: 16, y: 64, z: 0 }, BlockStateId(1))
            .unwrap();
        assert!(
            !publish_computed_light_if_sources_current(
                &world,
                Some(&world_read),
                Some(&world_mutation),
                ChunkPos { x: 0, z: 0 },
                &block_sources,
                &ChunkLight::filled(15, 0),
            )
            .await
        );
    }

    #[test]
    fn missing_neighbour_planner_returns_cached_and_load_sources() {
        let registry = Arc::new(air_block_registry());
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let centre = ChunkPos { x: 0, z: 0 };
        let cached_neighbour = ChunkPos { x: 1, z: 0 };
        let mut storage = WorldStorage::in_memory_with_capacity(registry, 16);
        for position in [centre, cached_neighbour] {
            storage
                .insert_generated_chunk(
                    position,
                    Chunk::empty(position, BlockStateId(0), biome.clone()),
                )
                .unwrap();
        }
        let mut neighbourhood: [[Option<Arc<Chunk>>; 3]; 3] =
            std::array::from_fn(|_| std::array::from_fn(|_| None));
        neighbourhood[1][1] = storage.cached_chunk_snapshot(centre);

        let plans = plan_missing_neighbour_snapshots(&storage, centre, &neighbourhood);

        assert_eq!(plans.len(), 8);
        assert_eq!(
            plans
                .iter()
                .filter(|plan| matches!(plan.source, NeighbourSnapshotSource::Cached(_)))
                .count(),
            1
        );
        assert!(plans.iter().any(|plan| {
            plan.position == cached_neighbour
                && matches!(plan.source, NeighbourSnapshotSource::Cached(_))
        }));
        assert_eq!(
            plans
                .iter()
                .filter(|plan| matches!(plan.source, NeighbourSnapshotSource::Load(_)))
                .count(),
            7
        );
    }

    #[tokio::test]
    async fn cached_neighbourhood_snapshot_does_not_wait_for_world_writer() {
        let registry = Arc::new(air_block_registry());
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 16);
        for z in -1..=1 {
            for x in -1..=1 {
                let pos = ChunkPos { x, z };
                storage
                    .insert_generated_chunk(pos, Chunk::empty(pos, BlockStateId(0), biome.clone()))
                    .unwrap();
            }
        }
        let world_read = storage.read_view();
        let world = Arc::new(Mutex::new(storage));
        let request = ChunkRequest {
            chunk_x: 0,
            chunk_z: 0,
            priority: ChunkPriority {
                ring: 0,
                sequence: 0,
            },
            generation: ChunkPipelineGeneration(1),
        };
        let _writer = world.lock().await;

        let loaded = tokio::time::timeout(
            Duration::from_secs(1),
            load_chunk_neighbourhood(
                Arc::clone(&world),
                Some(world_read),
                0,
                0,
                ChunkPipelineResources::with_limits(1, 1),
                request,
                Arc::new(AtomicU64::new(1)),
                true,
            ),
        )
        .await
        .expect("cached immutable snapshot must not wait for world writer")
        .unwrap();

        assert!(loaded.centre.is_some());
        assert!(loaded.neighbourhood.iter().flatten().all(Option::is_some));
        assert_eq!(loaded.staged.len(), 9);
    }

    #[tokio::test]
    async fn generated_prepare_budget_classification_does_not_wait_for_world_writer() {
        let registry = Arc::new(air_block_registry());
        let calls = Arc::new(AtomicUsize::new(0));
        let storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 16)
            .with_generator(Arc::new(CountingGenerator {
                calls: Arc::clone(&calls),
            }));
        let world_read = storage.read_view();
        let chunk_source = storage.chunk_source_view();
        let world = Arc::new(Mutex::new(storage));
        let sessions = Arc::new(SessionRegistry::new());
        let (tx, _rx) = mpsc::channel(1);
        let profile = LoggedInProfile {
            uuid: uuid::Uuid::nil(),
            name: "source-classifier".to_string(),
        };
        let (session_id, _) = sessions.register(
            &profile,
            (0, 0),
            0,
            desired_chunk_set(0, 0, 0),
            tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        let stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            sessions,
            session_id,
            0,
            0,
            0.0,
            0,
            ChunkPipelineResources::with_limits(1, 1),
            ChunkPipelinePolicy::default(),
        )
        .with_world_read(Some(world_read))
        .with_chunk_source(Some(chunk_source));
        let request = ChunkRequest {
            chunk_x: 4,
            chunk_z: -2,
            priority: ChunkPriority {
                ring: 0,
                sequence: 0,
            },
            generation: ChunkPipelineGeneration(1),
        };
        let writer = world.lock().await;

        let class = tokio::time::timeout(
            Duration::from_secs(1),
            stream.classify_prepare_budget(request),
        )
        .await
        .expect("prepare budget classification waited for the world writer");
        drop(writer);

        assert_eq!(class, ChunkPrepareBudgetClass::Generate);
        assert_eq!(calls.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn published_cache_pressure_check_does_not_wait_for_world_writer() {
        let registry = Arc::new(air_block_registry());
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 1)
            .with_generator(Arc::new(CountingGenerator {
                calls: Arc::new(AtomicUsize::new(0)),
            }));
        storage
            .insert_generated_chunk(
                ChunkPos { x: 0, z: 0 },
                Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
            )
            .unwrap();
        let world_read = storage.read_view();
        let world = Arc::new(Mutex::new(storage));
        let writer = world.lock().await;

        let can_cache = tokio::time::timeout(
            Duration::from_secs(1),
            chunk_prepare_can_cache(
                &world,
                Some(&world_read),
                ChunkPos { x: 1, z: 0 },
                "test published cache pressure",
            ),
        )
        .await
        .expect("published pressure check waited for world writer");
        drop(writer);

        assert!(!can_cache);
    }

    #[tokio::test]
    async fn dirty_pressure_defers_generated_stream_chunk_instead_of_absent() {
        let registry = Arc::new(air_block_registry());
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 1)
            .with_generator(Arc::new(CountingGenerator {
                calls: Arc::clone(&calls),
            }));
        storage
            .insert_generated_chunk(
                ChunkPos { x: 0, z: 0 },
                Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
            )
            .unwrap();
        let world = Arc::new(Mutex::new(storage));
        let request = ChunkRequest {
            chunk_x: 1,
            chunk_z: 0,
            priority: ChunkPriority {
                ring: 0,
                sequence: 0,
            },
            generation: ChunkPipelineGeneration(1),
        };

        let loaded = load_chunk_neighbourhood(
            Arc::clone(&world),
            None,
            1,
            0,
            ChunkPipelineResources::with_limits(1, 1),
            request,
            Arc::new(AtomicU64::new(1)),
            false,
        )
        .await
        .unwrap();

        assert!(loaded.centre.is_none());
        assert_eq!(loaded.staged, vec![(0, 0)]);
        assert!(loaded.backpressured);
        assert_eq!(world.lock().await.cache_len(), 1);
        assert_eq!(calls.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn absent_disk_chunk_counts_absent_even_under_dirty_pressure_without_generator() {
        let registry = Arc::new(air_block_registry());
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 1);
        storage
            .insert_generated_chunk(
                ChunkPos { x: 0, z: 0 },
                Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
            )
            .unwrap();
        let world = Arc::new(Mutex::new(storage));
        let request = ChunkRequest {
            chunk_x: 1,
            chunk_z: 0,
            priority: ChunkPriority {
                ring: 0,
                sequence: 0,
            },
            generation: ChunkPipelineGeneration(1),
        };

        let loaded = load_chunk_neighbourhood(
            Arc::clone(&world),
            None,
            1,
            0,
            ChunkPipelineResources::with_limits(1, 1),
            request,
            Arc::new(AtomicU64::new(1)),
            false,
        )
        .await
        .unwrap();

        assert!(loaded.centre.is_none());
        assert_eq!(loaded.staged, vec![(0, 0)]);
        assert!(!loaded.backpressured);
        assert_eq!(world.lock().await.cache_len(), 1);
    }

    #[tokio::test]
    async fn corrupt_saved_chunk_fails_without_generator_fallback() {
        let registry = Arc::new(air_block_registry());
        let calls = Arc::new(AtomicUsize::new(0));
        let temp = tempfile::tempdir().unwrap();
        let region_root = temp.path().join("region");
        std::fs::create_dir_all(&region_root).unwrap();
        let region_path = region_root.join("r.0.0.mca");
        let corrupt = b"corrupt region";
        std::fs::write(&region_path, corrupt).unwrap();
        let storage = WorldStorage::open_with_capacity(temp.path(), Arc::clone(&registry), 16)
            .unwrap()
            .with_generator(Arc::new(CountingGenerator {
                calls: Arc::clone(&calls),
            }));
        let world = Arc::new(Mutex::new(storage));
        let request = ChunkRequest {
            chunk_x: 0,
            chunk_z: 0,
            priority: ChunkPriority {
                ring: 0,
                sequence: 0,
            },
            generation: ChunkPipelineGeneration(1),
        };

        let result = load_chunk_neighbourhood(
            world,
            None,
            0,
            0,
            ChunkPipelineResources::with_limits(1, 1),
            request,
            Arc::new(AtomicU64::new(1)),
            false,
        )
        .await;
        let error = match result {
            Ok(_) => panic!("corrupt saved chunk was replaced by generator fallback"),
            Err(error) => error,
        };

        assert!(error.contains("chunk read failed"), "{error}");
        assert_eq!(calls.load(Ordering::Acquire), 0);
        assert_eq!(std::fs::read(region_path).unwrap(), corrupt);
    }

    #[tokio::test]
    async fn neighbour_dirty_pressure_backpressures_full_neighbourhood() {
        let registry = Arc::new(air_block_registry());
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 1)
            .with_generator(Arc::new(CountingGenerator {
                calls: Arc::clone(&calls),
            }));
        storage
            .insert_generated_chunk(
                ChunkPos { x: 0, z: 0 },
                Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
            )
            .unwrap();
        let world = Arc::new(Mutex::new(storage));
        let request = ChunkRequest {
            chunk_x: 0,
            chunk_z: 0,
            priority: ChunkPriority {
                ring: 0,
                sequence: 0,
            },
            generation: ChunkPipelineGeneration(1),
        };

        let loaded = load_chunk_neighbourhood(
            Arc::clone(&world),
            None,
            0,
            0,
            ChunkPipelineResources::with_limits(1, 1),
            request,
            Arc::new(AtomicU64::new(1)),
            true,
        )
        .await
        .unwrap();

        assert!(loaded.centre.is_some());
        assert!(loaded.neighbourhood[1][1].is_some());
        assert_eq!(loaded.staged, vec![(0, 0)]);
        assert!(loaded.backpressured);
        assert_eq!(calls.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn stream_step_retains_backpressured_chunk_without_absent_success() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 1)
            .with_generator(Arc::new(CountingGenerator {
                calls: Arc::clone(&calls),
            }));
        storage
            .insert_generated_chunk(
                ChunkPos { x: 0, z: 0 },
                Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
            )
            .unwrap();
        let world = Arc::new(Mutex::new(storage));
        let policy = ChunkPipelinePolicy {
            chunk_send_rate: 1,
            chunk_prepare_batch_size: 1,
            chunk_result_queue_size: 1,
            ..ChunkPipelinePolicy::default()
        };
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::new(SessionRegistry::new()),
            1,
            1,
            0,
            0.0,
            0,
            ChunkPipelineResources::with_limits(1, 1),
            policy,
        );
        let mut writer = tokio::io::sink();
        let mut light_cache = LightCache::new();

        let request = stream.scheduler.poll_next().expect("request");
        let result = prepare_chunk_request(
            request,
            Arc::clone(&world),
            None,
            None,
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            ChunkPipelineResources::with_limits(1, 1),
            Arc::clone(&stream.active_generation),
            0,
        )
        .await;
        assert!(matches!(result.outcome, ChunkPrepareOutcome::Backpressured));
        assert_eq!(result.pressure_flush.runs, 0);
        assert_eq!(result.pressure_flush.planned_chunks, 0);
        assert_eq!(result.pressure_flush.flushed_chunks, 0);
        stream.accept_result(result);
        assert_eq!(
            stream
                .emit_next_ready(&mut writer, &mut light_cache)
                .await
                .unwrap(),
            EmitReadyResult::Blocked
        );

        assert!(!stream.is_complete());
        assert!(stream.pressure_retries.contains_key(&(1, 0)));
        assert_eq!(stream.absent, 0);
        assert_eq!(stream.emitted, 0);
        assert_eq!(stream.pressure_abandoned, 0);
        assert!(stream.staged.is_empty());
        assert_eq!(calls.load(Ordering::Acquire), 0);
        assert_eq!(world.lock().await.cache_len(), 1);
    }

    #[tokio::test]
    async fn backpressured_result_counts_staged_chunks_without_marking_success() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::new(SessionRegistry::new()),
            1,
            1,
            0,
            0.0,
            0,
            ChunkPipelineResources::with_limits(1, 1),
            ChunkPipelinePolicy::default(),
        );
        let request = stream.scheduler.poll_next().expect("request");
        stream.accept_result(ChunkPrepareResult {
            request,
            prepare_claim: None,
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming {
                runs: 1,
                planned_chunks: 2,
                flushed_chunks: 1,
                plan_ms: 3,
                write_ms: 5,
                commit_ms: 7,
            },
            staged: vec![(0, 0)],
            outcome: ChunkPrepareOutcome::Backpressured,
        });
        let mut writer = tokio::io::sink();
        let mut light_cache = LightCache::new();

        assert_eq!(
            stream
                .emit_next_ready(&mut writer, &mut light_cache)
                .await
                .unwrap(),
            EmitReadyResult::Blocked
        );

        assert_eq!(stream.pressure_staged_count(), 1);
        assert!(stream.pressure_staged_contains((0, 0)));
        assert_eq!(stream.pressure_abandoned, 0);
        assert_eq!(stream.pressure_flush_runs, 1);
        assert_eq!(stream.pressure_flush_planned_chunks, 2);
        assert_eq!(stream.pressure_flush_flushed_chunks, 1);
        assert_eq!(stream.pressure_flush_plan_ms, 3);
        assert_eq!(stream.pressure_flush_write_ms, 5);
        assert_eq!(stream.pressure_flush_commit_ms, 7);
        assert_eq!(stream.max_pressure_flush_plan_ms, 3);
        assert_eq!(stream.max_pressure_flush_write_ms, 5);
        assert_eq!(stream.max_pressure_flush_commit_ms, 7);
        assert_eq!(stream.absent, 0);
        assert_eq!(stream.emitted, 0);
        assert!(stream.staged.is_empty());
        assert_eq!(stream.scheduler.queued_len(), 1);
        assert_eq!(stream.scheduler.finished_len(), 0);

        for _ in 1..CHUNK_BACKPRESSURE_MAX_RETRIES {
            let request = stream.scheduler.poll_next().expect("deferred request");
            stream.accept_result(ChunkPrepareResult {
                request,
                prepare_claim: None,
                fetch_ms: 0,
                pressure_flush: PressureFlushTiming::default(),
                staged: vec![(0, 0)],
                outcome: ChunkPrepareOutcome::Backpressured,
            });
            let _ = stream
                .emit_next_ready(&mut writer, &mut light_cache)
                .await
                .unwrap();
        }

        assert_eq!(stream.pressure_abandoned, 1);
        assert_eq!(stream.scheduler.finished_len(), 1);
    }

    #[tokio::test]
    async fn backpressured_result_is_not_send_progress() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::new(SessionRegistry::new()),
            1,
            1,
            0,
            0.0,
            0,
            ChunkPipelineResources::with_limits(1, 1),
            ChunkPipelinePolicy::default(),
        );
        let request = stream.scheduler.poll_next().expect("request");
        stream.accept_result(ChunkPrepareResult {
            request,
            prepare_claim: None,
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming::default(),
            staged: vec![(0, 0)],
            outcome: ChunkPrepareOutcome::Backpressured,
        });
        let mut writer = tokio::io::sink();
        let mut light_cache = LightCache::new();

        assert_eq!(stream.yielded_turns, 0);
        assert_eq!(
            stream.step(&mut writer, &mut light_cache).await.unwrap(),
            ChunkStreamStep::Progress
        );

        assert_eq!(stream.yielded_turns, 1);
        assert_eq!(stream.emitted, 0);
        assert_eq!(stream.scheduler.queued_len(), 1);
        assert_eq!(stream.scheduler.finished_len(), 0);

        assert!(stream.replan_center(1, 0, 45.0).is_empty());
        assert!(stream.pressure_retries.contains_key(&(1, 0)));
        assert!(stream.pressure_staged_contains((0, 0)));
    }

    #[tokio::test]
    async fn backpressured_request_redispatches_without_turn_delay() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let policy = ChunkPipelinePolicy {
            chunk_prepare_batch_size: 2,
            chunk_result_queue_size: 2,
            ..ChunkPipelinePolicy::default()
        };
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::new(SessionRegistry::new()),
            1,
            0,
            0,
            0.0,
            0,
            ChunkPipelineResources::with_limits(1, 1),
            policy,
        );
        let request = stream.scheduler.poll_next().expect("request");
        stream.accept_result(ChunkPrepareResult {
            request,
            prepare_claim: None,
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming::default(),
            staged: Vec::new(),
            outcome: ChunkPrepareOutcome::Backpressured,
        });
        let mut writer = tokio::io::sink();
        let mut light_cache = LightCache::new();
        assert_eq!(
            stream
                .emit_next_ready(&mut writer, &mut light_cache)
                .await
                .unwrap(),
            EmitReadyResult::Blocked
        );

        stream.dispatch_available().await;

        assert_eq!(stream.scheduler.in_flight_len(), 1);
    }

    #[tokio::test]
    async fn absent_result_drains_without_counting_as_backpressure_yield() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::new(SessionRegistry::new()),
            1,
            0,
            0,
            0.0,
            0,
            ChunkPipelineResources::with_limits(1, 1),
            ChunkPipelinePolicy::default(),
        );
        let request = stream.scheduler.poll_next().expect("request");
        stream.accept_result(ChunkPrepareResult {
            request,
            prepare_claim: None,
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming::default(),
            staged: Vec::new(),
            outcome: ChunkPrepareOutcome::Absent,
        });
        let mut writer = tokio::io::sink();
        let mut light_cache = LightCache::new();

        assert_eq!(
            stream.step(&mut writer, &mut light_cache).await.unwrap(),
            ChunkStreamStep::Complete
        );

        assert_eq!(stream.absent, 1);
        assert_eq!(stream.emitted, 0);
        assert_eq!(stream.yielded_turns, 0);
        assert_eq!(stream.pressure_abandoned, 0);
    }

    #[tokio::test]
    async fn non_packet_results_do_not_consume_ready_batch_send_budget() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let policy = ChunkPipelinePolicy {
            chunk_send_rate: 4,
            chunk_result_queue_size: 4,
            ..ChunkPipelinePolicy::default()
        };
        let resources = ChunkPipelineResources::with_limits(1, 1);
        let metrics = resources.metrics();
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::new(SessionRegistry::new()),
            1,
            0,
            0,
            0.0,
            1,
            resources,
            policy,
        );
        for _ in 0..2 {
            let request = stream.scheduler.poll_next().expect("queued chunk");
            stream.accept_result(ChunkPrepareResult {
                request,
                prepare_claim: None,
                fetch_ms: 0,
                pressure_flush: PressureFlushTiming::default(),
                staged: Vec::new(),
                outcome: ChunkPrepareOutcome::Absent,
            });
        }
        for _ in 0..2 {
            let request = stream.scheduler.poll_next().expect("queued chunk");
            stream.accept_result(ChunkPrepareResult {
                request,
                prepare_claim: None,
                fetch_ms: 0,
                pressure_flush: PressureFlushTiming::default(),
                staged: Vec::new(),
                outcome: ChunkPrepareOutcome::Ready(Box::new(PreparedChunkFrame {
                    frame: Bytes::from_static(b"prepared-frame"),
                    light: None,
                    herd_spawns: Vec::new(),
                    hydrated_campfires: Vec::new(),
                    packet_data_len: 1,
                    build_timing: ChunkBuildTiming::default(),
                    write_timing: ChunkWriteTiming::default(),
                })),
            });
        }
        stream.apply_runtime_control_limits(crate::RuntimeControlLimits {
            view_distance: 1,
            chunk_send_rate: 1,
            chunk_load_rate: 64,
            chunk_generate_rate: 32,
        });
        let mut writer = tokio::io::sink();
        let mut light_cache = LightCache::new();

        assert!(
            stream
                .emit_ready_batch(&mut writer, &mut light_cache)
                .await
                .unwrap()
        );

        assert_eq!(stream.absent, 2);
        assert_eq!(stream.emitted, 1);
        assert_eq!(stream.ready.len(), 1);
        assert_eq!(stream.last_stop_reason, ChunkPipelineStopReason::SendBudget);
        assert_eq!(metrics.stop_reason_counts().send_budget, 1);
        assert!(
            metrics
                .observed_stop_reasons()
                .contains(&ChunkPipelineStopReason::SendBudget)
        );
    }

    #[tokio::test]
    async fn runtime_control_queue_pressure_scales_live_send_budget() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let policy = ChunkPipelinePolicy {
            chunk_send_rate: 4,
            chunk_result_queue_size: 4,
            ..ChunkPipelinePolicy::default()
        };
        let control = crate::RuntimeControlHandle::new(crate::RuntimeControlConfig {
            policy: crate::AutoscalePolicy {
                min_view_distance: 2,
                max_view_distance: 2,
                min_chunk_send_rate: 1,
                max_chunk_send_rate: 4,
                min_chunk_load_rate: 1,
                max_chunk_load_rate: 64,
                min_chunk_generate_rate: 1,
                max_chunk_generate_rate: 32,
                queue_pressure_percent: 75,
                scale_down_after_ticks: 1,
                ..crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced)
            },
            initial_limits: crate::RuntimeControlLimits {
                view_distance: 2,
                chunk_send_rate: 4,
                chunk_load_rate: 64,
                chunk_generate_rate: 32,
            },
        });
        let mut signals = control
            .take_signal_receiver()
            .expect("test owns runtime control receiver");
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::new(SessionRegistry::new()),
            1,
            0,
            0,
            0.0,
            2,
            ChunkPipelineResources::with_limits(1, 1),
            policy,
        )
        .with_runtime_control(Some(control.clone()));
        for _ in 0..2 {
            let request = stream.scheduler.poll_next().expect("queued chunk");
            stream.accept_result(ChunkPrepareResult {
                request,
                prepare_claim: None,
                fetch_ms: 0,
                pressure_flush: PressureFlushTiming::default(),
                staged: Vec::new(),
                outcome: ChunkPrepareOutcome::Ready(Box::new(PreparedChunkFrame {
                    frame: Bytes::from_static(b"prepared-frame"),
                    light: None,
                    herd_spawns: Vec::new(),
                    hydrated_campfires: Vec::new(),
                    packet_data_len: 1,
                    build_timing: ChunkBuildTiming::default(),
                    write_timing: ChunkWriteTiming::default(),
                })),
            });
        }
        stream.observe_runtime_control();
        assert!(!stream.chunk_queue_saturated);
        let request = stream.scheduler.poll_next().expect("third queued chunk");
        stream.accept_result(ChunkPrepareResult {
            request,
            prepare_claim: None,
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming::default(),
            staged: Vec::new(),
            outcome: ChunkPrepareOutcome::Ready(Box::new(PreparedChunkFrame {
                frame: Bytes::from_static(b"prepared-frame"),
                light: None,
                herd_spawns: Vec::new(),
                hydrated_campfires: Vec::new(),
                packet_data_len: 1,
                build_timing: ChunkBuildTiming::default(),
                write_timing: ChunkWriteTiming::default(),
            })),
        });
        stream.observe_runtime_control();
        assert!(stream.chunk_queue_saturated);
        let owner_decision = observe_next_runtime_control_signal(&control, &mut signals).await;
        assert_eq!(owner_decision.action, crate::AutoscaleAction::ScaleDown);
        stream.observe_runtime_control();
        let mut writer = tokio::io::sink();
        let mut light_cache = LightCache::new();

        assert!(
            stream
                .emit_ready_batch(&mut writer, &mut light_cache)
                .await
                .unwrap()
        );

        let snapshot = control.snapshot();
        assert_eq!(
            snapshot.last_decision.action,
            crate::AutoscaleAction::ScaleDown
        );
        assert_eq!(
            snapshot.last_decision.pressure,
            Some(crate::AutoscalePressure::ChunkQueue)
        );
        assert_eq!(snapshot.limits.chunk_send_rate, 2);
        assert_eq!(stream.emitted, 2);
        assert_eq!(stream.ready.len(), 1);
        assert_eq!(stream.last_stop_reason, ChunkPipelineStopReason::SendBudget);
    }

    #[test]
    fn rotation_reprioritizes_without_cancelling_valid_chunk_work() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let mut stream = ChunkStreamState::new(
            world,
            Arc::new(test_biome_registry()),
            registry,
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::new(SessionRegistry::new()),
            1,
            0,
            0,
            0.0,
            1,
            ChunkPipelineResources::with_limits(1, 1),
            ChunkPipelinePolicy::default(),
        );
        let ready_request = stream.scheduler.poll_next().expect("ready request");
        stream.accept_result(ChunkPrepareResult {
            request: ready_request,
            prepare_claim: None,
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming::default(),
            staged: Vec::new(),
            outcome: ChunkPrepareOutcome::Absent,
        });
        let in_flight_request = stream.scheduler.poll_next().expect("in-flight request");
        let generation = stream.scheduler.current_generation();

        stream.replan_center(0, 0, 90.0);

        assert_eq!(stream.scheduler.current_generation(), generation);
        assert_eq!(stream.ready.len(), 1);
        assert_eq!(stream.scheduler.in_flight_len(), 2);
        assert!(stream.scheduler.is_current(ready_request));
        assert!(stream.scheduler.is_current(in_flight_request));
    }

    #[tokio::test]
    async fn runtime_control_memory_pressure_scales_live_stream_limits() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let memory_pressure = crate::memory_pressure::MemoryPressureHandle::with_sample(
            crate::memory_pressure::MemoryPressureSnapshot {
                used_mb: 900,
                limit_mb: 1_000,
            },
        );
        let control = crate::RuntimeControlHandle::new_with_memory_pressure(
            crate::RuntimeControlConfig {
                policy: crate::AutoscalePolicy {
                    min_view_distance: 2,
                    max_view_distance: 2,
                    min_chunk_send_rate: 1,
                    max_chunk_send_rate: 4,
                    min_chunk_load_rate: 1,
                    max_chunk_load_rate: 64,
                    min_chunk_generate_rate: 1,
                    max_chunk_generate_rate: 32,
                    queue_pressure_percent: 100,
                    memory_pressure_percent: 50,
                    scale_down_after_ticks: 1,
                    ..crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced)
                },
                initial_limits: crate::RuntimeControlLimits {
                    view_distance: 2,
                    chunk_send_rate: 4,
                    chunk_load_rate: 64,
                    chunk_generate_rate: 32,
                },
            },
            memory_pressure,
        );
        let resources = ChunkPipelineResources::with_limits(1, 4);
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::new(SessionRegistry::new()),
            1,
            0,
            0,
            0.0,
            2,
            resources.clone(),
            ChunkPipelinePolicy::default(),
        )
        .with_runtime_control(Some(control.clone()));

        stream.observe_runtime_control();
        assert_eq!(resources.cpu_limit(), 4);
        let owner_decision = control.observe(healthy_runtime_control_input());
        resources.apply_runtime_control_action(owner_decision.action, false);
        stream.observe_runtime_control();
        let snapshot = control.snapshot();

        assert_eq!(
            snapshot.last_decision.pressure,
            Some(crate::AutoscalePressure::Memory)
        );
        assert_eq!(
            snapshot.last_decision.action,
            crate::AutoscaleAction::ScaleDown
        );
        assert_eq!(snapshot.limits.chunk_send_rate, 2);
        assert_eq!(resources.cpu_limit(), 2);
    }

    #[tokio::test]
    async fn runtime_control_memory_pressure_sheds_ready_and_in_flight_work() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let memory_pressure = crate::memory_pressure::MemoryPressureHandle::with_sample(
            crate::memory_pressure::MemoryPressureSnapshot {
                used_mb: 900,
                limit_mb: 1_000,
            },
        );
        let control = crate::RuntimeControlHandle::new_with_memory_pressure(
            crate::RuntimeControlConfig {
                policy: crate::AutoscalePolicy {
                    min_view_distance: 1,
                    max_view_distance: 1,
                    min_chunk_send_rate: 1,
                    max_chunk_send_rate: 4,
                    min_chunk_load_rate: 1,
                    max_chunk_load_rate: 64,
                    min_chunk_generate_rate: 1,
                    max_chunk_generate_rate: 32,
                    queue_pressure_percent: 100,
                    memory_pressure_percent: 50,
                    scale_down_after_ticks: 1,
                    ..crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced)
                },
                initial_limits: crate::RuntimeControlLimits {
                    view_distance: 1,
                    chunk_send_rate: 4,
                    chunk_load_rate: 64,
                    chunk_generate_rate: 32,
                },
            },
            memory_pressure,
        );
        let policy = ChunkPipelinePolicy {
            chunk_send_rate: 4,
            chunk_result_queue_size: 16,
            ..ChunkPipelinePolicy::default()
        };
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::new(SessionRegistry::new()),
            1,
            0,
            0,
            0.0,
            1,
            ChunkPipelineResources::with_limits(1, 1),
            policy,
        )
        .with_runtime_control(Some(control.clone()));
        let mut old_generation = None;
        for _ in 0..3 {
            let request = stream.scheduler.poll_next().expect("queued chunk");
            old_generation.get_or_insert(request.generation);
            stream.accept_result(ChunkPrepareResult {
                request,
                prepare_claim: None,
                fetch_ms: 0,
                pressure_flush: PressureFlushTiming::default(),
                staged: Vec::new(),
                outcome: ChunkPrepareOutcome::Absent,
            });
        }
        let active_request = stream.scheduler.poll_next().expect("active chunk");
        assert_eq!(Some(active_request.generation), old_generation);
        assert_eq!(stream.ready.len(), 3);
        assert_eq!(stream.scheduler.in_flight_len(), 4);

        stream.observe_runtime_control();
        control.observe(healthy_runtime_control_input());
        stream.observe_runtime_control();

        let snapshot = control.snapshot();
        assert_eq!(
            snapshot.last_decision.pressure,
            Some(crate::AutoscalePressure::Memory)
        );
        assert_eq!(stream.ready.len(), 0);
        assert_eq!(stream.scheduler.in_flight_len(), 0);
        assert!(stream.scheduler.queued_len() >= 4);
        assert_eq!(stream.memory_pressure_shed_runs, 1);
        assert_eq!(stream.memory_pressure_shed_ready, 3);
        assert_eq!(stream.memory_pressure_shed_in_flight, 1);
        assert_eq!(
            stream.last_stop_reason,
            ChunkPipelineStopReason::MemoryPressure
        );

        let replayed = stream.scheduler.poll_next().expect("replayed request");
        assert_ne!(Some(replayed.generation), old_generation);
    }

    #[tokio::test]
    async fn runtime_control_memory_pressure_pauses_dispatch_until_pressure_clears() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let memory_pressure = crate::memory_pressure::MemoryPressureHandle::with_sample(
            crate::memory_pressure::MemoryPressureSnapshot {
                used_mb: 900,
                limit_mb: 1_000,
            },
        );
        let control = crate::RuntimeControlHandle::new_with_memory_pressure(
            crate::RuntimeControlConfig {
                policy: crate::AutoscalePolicy {
                    min_view_distance: 1,
                    max_view_distance: 1,
                    min_chunk_send_rate: 1,
                    max_chunk_send_rate: 4,
                    min_chunk_load_rate: 1,
                    max_chunk_load_rate: 64,
                    min_chunk_generate_rate: 1,
                    max_chunk_generate_rate: 32,
                    memory_pressure_percent: 50,
                    scale_down_after_ticks: 1,
                    scale_up_after_ticks: 1,
                    ..crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced)
                },
                initial_limits: crate::RuntimeControlLimits {
                    view_distance: 1,
                    chunk_send_rate: 4,
                    chunk_load_rate: 64,
                    chunk_generate_rate: 32,
                },
            },
            memory_pressure.clone(),
        );
        let policy = ChunkPipelinePolicy {
            chunk_prepare_batch_size: 2,
            chunk_result_queue_size: 4,
            ..ChunkPipelinePolicy::default()
        };
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::new(SessionRegistry::new()),
            1,
            0,
            0,
            0.0,
            1,
            ChunkPipelineResources::with_limits(1, 1),
            policy,
        )
        .with_runtime_control(Some(control));
        let mut writer = tokio::io::sink();
        let mut light_cache = LightCache::new();

        stream.step(&mut writer, &mut light_cache).await.unwrap();

        assert_eq!(stream.scheduler.in_flight_len(), 0);
        assert_eq!(
            stream.last_stop_reason,
            ChunkPipelineStopReason::MemoryPressure
        );

        memory_pressure.set_sample(crate::memory_pressure::MemoryPressureSnapshot {
            used_mb: 100,
            limit_mb: 1_000,
        });
        stream.step(&mut writer, &mut light_cache).await.unwrap();

        assert!(stream.scheduler.in_flight_len() > 0);
        assert_ne!(
            stream.last_stop_reason,
            ChunkPipelineStopReason::MemoryPressure
        );
    }

    #[tokio::test]
    async fn runtime_control_queue_pressure_replans_live_view_distance() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let sessions = Arc::new(SessionRegistry::new());
        let (tx, _rx) = mpsc::channel(8);
        let profile = crate::login::LoggedInProfile {
            uuid: uuid::Uuid::nil(),
            name: "RuntimeViewDistanceAlice".to_string(),
        };
        let old_desired = desired_chunk_set(0, 0, 3);
        let (session_id, _) = sessions.register(
            &profile,
            (0, 0),
            3,
            old_desired,
            tx,
            PlayerPose::new(0.5, 64.0, 0.5),
        );
        let control = crate::RuntimeControlHandle::new(crate::RuntimeControlConfig {
            policy: crate::AutoscalePolicy {
                min_view_distance: 2,
                max_view_distance: 3,
                min_chunk_send_rate: 4,
                max_chunk_send_rate: 4,
                min_chunk_load_rate: 64,
                max_chunk_load_rate: 64,
                min_chunk_generate_rate: 32,
                max_chunk_generate_rate: 32,
                queue_pressure_percent: 1,
                scale_down_after_ticks: 1,
                ..crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced)
            },
            initial_limits: crate::RuntimeControlLimits {
                view_distance: 3,
                chunk_send_rate: 4,
                chunk_load_rate: 64,
                chunk_generate_rate: 32,
            },
        });
        let mut signals = control
            .take_signal_receiver()
            .expect("test owns runtime control receiver");
        let policy = ChunkPipelinePolicy {
            chunk_send_rate: 4,
            chunk_result_queue_size: 4,
            ..ChunkPipelinePolicy::default()
        };
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::clone(&sessions),
            session_id,
            0,
            0,
            0.0,
            3,
            ChunkPipelineResources::with_limits(1, 1),
            policy,
        )
        .with_runtime_control(Some(control.clone()));
        for _ in 0..2 {
            let request = stream.scheduler.poll_next().expect("queued chunk");
            stream.accept_result(ChunkPrepareResult {
                request,
                prepare_claim: None,
                fetch_ms: 0,
                pressure_flush: PressureFlushTiming::default(),
                staged: Vec::new(),
                outcome: ChunkPrepareOutcome::Absent,
            });
        }
        stream.loaded.insert((3, 0));
        sessions.mark_loaded(session_id, (3, 0));

        let (mut client, mut server) = tokio::io::duplex(256);
        let mut light_cache = LightCache::new();

        stream.step(&mut server, &mut light_cache).await.unwrap();
        let owner_decision = observe_next_runtime_control_signal(&control, &mut signals).await;
        assert_eq!(owner_decision.action, crate::AutoscaleAction::ScaleDown);
        stream.step(&mut server, &mut light_cache).await.unwrap();

        let snapshot = control.snapshot();
        assert_eq!(
            snapshot.last_decision.action,
            crate::AutoscaleAction::ScaleDown
        );
        assert_eq!(snapshot.limits.view_distance, 2);
        assert_eq!(stream.view_distance, 2);
        assert!(!stream.loaded.contains(&(3, 0)));
        assert_eq!(sessions.ticketed_chunks_sorted().len(), 25);
        assert!(!sessions.ticketed_chunks_sorted().contains(&(3, 0)));
        assert!(stream.ready.is_empty());

        let mut buf = BytesMut::new();
        let read = tokio::time::timeout(Duration::from_millis(100), client.read_buf(&mut buf))
            .await
            .expect("runtime view-distance shrink should emit a forget-level-chunk packet")
            .unwrap();
        assert!(read > 0);
        let frame = mc_protocol::frame::try_decode_frame(&mut buf, Compression::Disabled)
            .unwrap()
            .expect("forget-level-chunk frame");
        assert_eq!(frame.id, ForgetLevelChunk::ID);
        let packet = ForgetLevelChunk::decode(&mut frame.body.clone()).unwrap();
        assert_eq!(
            packet,
            ForgetLevelChunk {
                chunk_x: 3,
                chunk_z: 0,
            }
        );
    }

    #[test]
    fn runtime_control_view_distance_does_not_exceed_client_cap() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let sessions = Arc::new(SessionRegistry::new());
        let (tx, _rx) = mpsc::channel(8);
        let profile = crate::login::LoggedInProfile {
            uuid: uuid::Uuid::nil(),
            name: "RuntimeViewDistanceCapAlice".to_string(),
        };
        let old_desired = desired_chunk_set(0, 0, 3);
        let (session_id, _) = sessions.register(
            &profile,
            (0, 0),
            3,
            old_desired,
            tx,
            PlayerPose::new(0.5, 64.0, 0.5),
        );
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::clone(&sessions),
            session_id,
            0,
            0,
            0.0,
            3,
            ChunkPipelineResources::with_limits(1, 1),
            ChunkPipelinePolicy::default(),
        );

        stream.replan_view_distance(2, 0.0);
        stream.apply_runtime_control_limits(crate::RuntimeControlLimits {
            view_distance: 3,
            chunk_send_rate: 16,
            chunk_load_rate: 64,
            chunk_generate_rate: 32,
        });

        assert_eq!(stream.view_distance, 2);
        assert_eq!(sessions.ticketed_chunks_sorted().len(), 25);
        assert!(!sessions.ticketed_chunks_sorted().contains(&(3, 0)));
    }

    #[tokio::test]
    async fn runtime_limits_reduce_load_dispatch_budget() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let policy = ChunkPipelinePolicy {
            chunk_prepare_batch_size: 4,
            chunk_result_queue_size: 8,
            ..ChunkPipelinePolicy::default()
        };
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::new(SessionRegistry::new()),
            1,
            0,
            0,
            0.0,
            2,
            ChunkPipelineResources::with_limits(1, 1),
            policy,
        );
        stream.apply_runtime_control_limits(crate::RuntimeControlLimits {
            view_distance: 2,
            chunk_send_rate: 16,
            chunk_load_rate: 1,
            chunk_generate_rate: 64,
        });

        stream.dispatch_available().await;

        assert_eq!(stream.dispatched, 1);
        assert_eq!(stream.scheduler.in_flight_len(), 1);
        assert_eq!(stream.last_stop_reason, ChunkPipelineStopReason::LoadBudget);
    }

    #[tokio::test]
    async fn runtime_limits_reduce_generate_dispatch_budget() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let calls = Arc::new(AtomicUsize::new(0));
        let world = Arc::new(Mutex::new(
            WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 1).with_generator(
                Arc::new(CountingGenerator {
                    calls: Arc::clone(&calls),
                }),
            ),
        ));
        let policy = ChunkPipelinePolicy {
            chunk_prepare_batch_size: 4,
            chunk_result_queue_size: 8,
            ..ChunkPipelinePolicy::default()
        };
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::new(SessionRegistry::new()),
            1,
            0,
            0,
            0.0,
            2,
            ChunkPipelineResources::with_limits(1, 1),
            policy,
        );
        stream.apply_runtime_control_limits(crate::RuntimeControlLimits {
            view_distance: 2,
            chunk_send_rate: 16,
            chunk_load_rate: 64,
            chunk_generate_rate: 1,
        });

        stream.dispatch_available().await;

        assert_eq!(stream.dispatched, 1);
        assert_eq!(stream.scheduler.in_flight_len(), 1);
        assert_eq!(
            stream.last_stop_reason,
            ChunkPipelineStopReason::GenerateBudget
        );
    }

    #[tokio::test]
    async fn low_load_budget_does_not_throttle_generated_dispatches() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let calls = Arc::new(AtomicUsize::new(0));
        let world = Arc::new(Mutex::new(
            WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 1).with_generator(
                Arc::new(CountingGenerator {
                    calls: Arc::clone(&calls),
                }),
            ),
        ));
        let policy = ChunkPipelinePolicy {
            chunk_prepare_batch_size: 4,
            chunk_result_queue_size: 8,
            ..ChunkPipelinePolicy::default()
        };
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::new(SessionRegistry::new()),
            1,
            0,
            0,
            0.0,
            2,
            ChunkPipelineResources::with_limits(1, 1),
            policy,
        );
        stream.apply_runtime_control_limits(crate::RuntimeControlLimits {
            view_distance: 2,
            chunk_send_rate: 16,
            chunk_load_rate: 1,
            chunk_generate_rate: 4,
        });

        stream.dispatch_available().await;

        assert_eq!(stream.dispatched, 4);
        assert_eq!(stream.scheduler.in_flight_len(), 4);
        assert_eq!(stream.last_stop_reason, ChunkPipelineStopReason::BatchLimit);
    }

    #[tokio::test]
    async fn low_generate_budget_does_not_throttle_cached_world_load_dispatches() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 8);
        for chunk in prioritized_spiral(0, 0, 1, 0.0)
            .take(4)
            .map(|(cx, cz, _)| ChunkPos { x: cx, z: cz })
        {
            storage
                .insert_generated_chunk(chunk, Chunk::empty(chunk, BlockStateId(0), biome.clone()))
                .unwrap();
        }
        let world = Arc::new(Mutex::new(storage));
        let policy = ChunkPipelinePolicy {
            chunk_prepare_batch_size: 4,
            chunk_result_queue_size: 8,
            ..ChunkPipelinePolicy::default()
        };
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::new(SessionRegistry::new()),
            1,
            0,
            0,
            0.0,
            1,
            ChunkPipelineResources::with_limits(1, 1),
            policy,
        );
        stream.apply_runtime_control_limits(crate::RuntimeControlLimits {
            view_distance: 1,
            chunk_send_rate: 16,
            chunk_load_rate: 4,
            chunk_generate_rate: 1,
        });

        stream.dispatch_available().await;

        assert_eq!(stream.dispatched, 4);
        assert_eq!(stream.scheduler.in_flight_len(), 4);
        assert_eq!(stream.last_stop_reason, ChunkPipelineStopReason::BatchLimit);
    }

    #[tokio::test]
    async fn low_generate_budget_does_not_throttle_prepared_cache_dispatches() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let sessions = Arc::new(SessionRegistry::new());
        let (tx, _rx) = mpsc::channel(1);
        let profile = LoggedInProfile {
            uuid: uuid::Uuid::nil(),
            name: "cache-budget".to_string(),
        };
        let desired = desired_chunk_set(0, 0, 1);
        let (session_id, _) = sessions.register(
            &profile,
            (0, 0),
            1,
            desired,
            tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        for chunk in prioritized_spiral(0, 0, 1, 0.0)
            .take(4)
            .map(|(cx, cz, _)| (cx, cz))
        {
            sessions.cache_prepared_chunk(
                chunk,
                Arc::new(PreparedChunkFrame {
                    frame: Bytes::from_static(b"chunk-frame"),
                    light: None,
                    herd_spawns: Vec::new(),
                    hydrated_campfires: Vec::new(),
                    packet_data_len: 0,
                    build_timing: ChunkBuildTiming::default(),
                    write_timing: ChunkWriteTiming::default(),
                }),
            );
        }
        let policy = ChunkPipelinePolicy {
            chunk_prepare_batch_size: 4,
            chunk_result_queue_size: 8,
            ..ChunkPipelinePolicy::default()
        };
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            sessions,
            session_id,
            0,
            0,
            0.0,
            1,
            ChunkPipelineResources::with_limits(1, 1),
            policy,
        );
        stream.apply_runtime_control_limits(crate::RuntimeControlLimits {
            view_distance: 1,
            chunk_send_rate: 16,
            chunk_load_rate: 64,
            chunk_generate_rate: 1,
        });

        stream.dispatch_available().await;

        assert_eq!(stream.dispatched, 4);
        assert_eq!(stream.ready.len(), 4);
        assert_eq!(stream.scheduler.in_flight_len(), 4);
        assert_eq!(stream.last_stop_reason, ChunkPipelineStopReason::BatchLimit);
    }

    #[tokio::test]
    async fn dispatch_prewarms_forward_edge_chunks_before_center_crossing() {
        let registry = Arc::new(air_block_registry());
        let calls = Arc::new(AtomicUsize::new(0));
        let world = Arc::new(Mutex::new(
            WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 128).with_generator(
                Arc::new(CountingGenerator {
                    calls: Arc::clone(&calls),
                }),
            ),
        ));
        let sessions = Arc::new(SessionRegistry::new());
        let (tx, _rx) = mpsc::channel(1);
        let profile = LoggedInProfile {
            uuid: uuid::Uuid::nil(),
            name: "forward-prewarm".to_string(),
        };
        let desired = desired_chunk_set(0, 0, 4);
        let (session_id, _) = sessions.register(
            &profile,
            (0, 0),
            4,
            desired,
            tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        let policy = ChunkPipelinePolicy {
            chunk_prepare_batch_size: 16,
            chunk_generate_rate: 16,
            chunk_result_queue_size: 64,
            ..ChunkPipelinePolicy::default()
        };
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::clone(&sessions),
            session_id,
            0,
            0,
            0.0,
            4,
            ChunkPipelineResources::with_limits(2, 2),
            policy,
        );

        let mut writer = tokio::io::sink();
        let mut light_cache = LightCache::new();
        drive_stream_to_completion(
            &mut stream,
            &mut writer,
            &mut light_cache,
            Duration::from_secs(2),
            "visible chunk window should complete before asserting forward prewarm",
        )
        .await;
        assert!(stream.is_complete(), "visible chunk window should complete");

        let forward_edge = (-4..=4).map(|x| (x, 5)).collect::<Vec<_>>();
        for chunk in &forward_edge {
            assert!(
                matches!(
                    sessions.prepared_chunk_or_claim(*chunk),
                    PreparedChunkClaimResult::InFlight | PreparedChunkClaimResult::Cached
                ),
                "forward edge chunk {chunk:?} should be claimed or cached before the client crosses into center_z=1"
            );
        }
        assert!(
            forward_edge
                .iter()
                .all(|chunk| !sessions.ticketed_chunks_sorted().contains(chunk)),
            "forward prewarm must not expand the client's visible/ticketed view"
        );
    }

    #[tokio::test]
    async fn single_client_prewarm_also_covers_opposite_edge_within_batch() {
        let registry = Arc::new(air_block_registry());
        let calls = Arc::new(AtomicUsize::new(0));
        let world = Arc::new(Mutex::new(
            WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 256).with_generator(
                Arc::new(CountingGenerator {
                    calls: Arc::clone(&calls),
                }),
            ),
        ));
        let sessions = Arc::new(SessionRegistry::new());
        let (tx, _rx) = mpsc::channel(1);
        let profile = LoggedInProfile {
            uuid: uuid::Uuid::nil(),
            name: "negative-z-prewarm".to_string(),
        };
        let desired = desired_chunk_set(0, 0, 4);
        let (session_id, _) = sessions.register(
            &profile,
            (0, 0),
            4,
            desired,
            tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        let policy = ChunkPipelinePolicy {
            chunk_prepare_batch_size: 16,
            chunk_generate_rate: 16,
            chunk_result_queue_size: 64,
            ..ChunkPipelinePolicy::default()
        };
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::clone(&sessions),
            session_id,
            0,
            0,
            0.0,
            4,
            ChunkPipelineResources::with_limits(8, 8),
            policy,
        );

        stream.dispatch_forward_prewarm();

        let negative_z_edge = (-4..=4).map(|x| (x, -5)).collect::<Vec<_>>();
        for chunk in &negative_z_edge {
            assert!(
                matches!(
                    sessions.prepared_chunk_or_claim(*chunk),
                    PreparedChunkClaimResult::InFlight | PreparedChunkClaimResult::Cached
                ),
                "single-client prewarm should claim opposite edge chunk {chunk:?} within the background batch"
            );
        }
        assert!(
            negative_z_edge
                .iter()
                .all(|chunk| !sessions.ticketed_chunks_sorted().contains(chunk)),
            "opposite-edge prewarm must not expand the client's visible/ticketed view"
        );
    }

    #[tokio::test]
    async fn healthy_background_observation_does_not_drop_prewarm_after_tick_pressure() {
        let registry = Arc::new(air_block_registry());
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            64,
        )));
        let sessions = Arc::new(SessionRegistry::new());
        let view_distance = 4;
        let (tx, _rx) = mpsc::channel(1);
        let profile = LoggedInProfile {
            uuid: uuid::Uuid::nil(),
            name: "pressure-prewarm".to_string(),
        };
        let (session_id, _) = sessions.register(
            &profile,
            (0, 0),
            view_distance,
            desired_chunk_set(0, 0, view_distance),
            tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        let control = crate::RuntimeControlHandle::new(crate::RuntimeControlConfig {
            policy: crate::AutoscalePolicy {
                min_view_distance: view_distance,
                max_view_distance: view_distance,
                min_chunk_send_rate: 1,
                max_chunk_send_rate: 16,
                min_chunk_load_rate: 1,
                max_chunk_load_rate: 64,
                min_chunk_generate_rate: 1,
                max_chunk_generate_rate: 32,
                target_tick_ms: 1,
                scale_down_after_ticks: 1,
                ..crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced)
            },
            initial_limits: crate::RuntimeControlLimits {
                view_distance,
                chunk_send_rate: 16,
                chunk_load_rate: 64,
                chunk_generate_rate: 32,
            },
        });
        let decision = control.observe(crate::RuntimeControlInput {
            tick_ms: 2,
            memory_used_mb: 0,
            memory_limit_mb: 0,
        });
        assert_eq!(decision.pressure, Some(crate::AutoscalePressure::TickTime));

        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            sessions,
            session_id,
            0,
            0,
            0.0,
            view_distance,
            ChunkPipelineResources::with_limits(2, 2),
            ChunkPipelinePolicy::default(),
        )
        .with_runtime_control(Some(control));

        assert!(stream.observe_runtime_control().is_empty());
        stream.dispatch_forward_prewarm();

        let expected = prewarm_edge_batch_limit(view_distance);
        assert_eq!(stream.prewarm_dispatched, expected);
        assert_eq!(stream.prewarm_in_flight.len(), expected);
    }

    #[test]
    fn prewarm_edge_ring_prioritizes_forward_z_edge() {
        let chunks = prewarm_edge_ring_chunks(0, 0, 4, 0.0);
        let first_edge = chunks.into_iter().take(9).collect::<Vec<_>>();

        for chunk in (-4..=4).map(|x| (x, 5)) {
            assert!(
                first_edge.contains(&chunk),
                "forward edge chunk {chunk:?} should be in the first prewarm edge batch, got {first_edge:?}"
            );
        }
    }

    #[test]
    fn prewarm_batch_adds_the_nearest_lateral_edge_at_playable_distance() {
        let limit = prewarm_edge_batch_limit(4);
        let chunks =
            prewarm_edge_batch_chunks(0, 0, 4, 0.0, PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 8.5));

        assert_eq!(limit, 27);
        assert_eq!(chunks.len(), limit);
        for chunk in (-4..=4).map(|x| (x, 5)).chain((-4..=4).map(|x| (x, -5))) {
            assert!(
                chunks.contains(&chunk),
                "missing likely edge chunk {chunk:?}"
            );
        }
        for chunk in (-4..=4).map(|z| (-5, z)) {
            assert!(chunks.contains(&chunk), "missing west edge chunk {chunk:?}");
        }
        assert!(!chunks.contains(&(5, 0)), "far east edge was prewarmed");
        assert!(
            chunks.iter().position(|chunk| *chunk == (-5, 0))
                < chunks.iter().position(|chunk| *chunk == (0, -5)),
            "the nearby lateral edge must start before the farther opposite edge"
        );
    }

    #[test]
    fn prewarm_batch_uses_east_edge_when_player_is_nearer_east_boundary() {
        let chunks =
            prewarm_edge_batch_chunks(0, 0, 4, 0.0, PlayerPose::new(15.5, DEFAULT_SPAWN_Y, 8.5));

        for chunk in (-4..=4).map(|z| (5, z)) {
            assert!(chunks.contains(&chunk), "missing east edge chunk {chunk:?}");
        }
        assert!(!chunks.contains(&(-5, 0)), "far west edge was prewarmed");
    }

    #[test]
    fn prewarm_edge_ring_caps_untrusted_view_distance() {
        let chunks = prewarm_edge_ring_chunks(0, 0, crate::MAX_VIEW_DISTANCE + 1, 0.0);
        let radius = crate::MAX_VIEW_DISTANCE + 1;

        assert_eq!(chunks.len(), (8 * radius) as usize);
    }

    #[tokio::test]
    async fn forward_prewarm_uses_autoscaler_cpu_limit() {
        let registry = Arc::new(air_block_registry());
        let calls = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let concurrent_call_started = Arc::new(tokio::sync::Notify::new());
        let first_call_gate = Arc::new(GenerationGate::new());
        let world = Arc::new(Mutex::new(
            WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 256).with_generator(
                Arc::new(CountingConcurrentGenerator {
                    calls: Arc::clone(&calls),
                    active: Arc::clone(&active),
                    max_active: Arc::clone(&max_active),
                    concurrent_call_started: Some(Arc::clone(&concurrent_call_started)),
                    first_call_gate: Some(Arc::clone(&first_call_gate)),
                    gate_first_call: AtomicBool::new(true),
                }),
            ),
        ));
        let sessions = Arc::new(SessionRegistry::new());
        let (tx, _rx) = mpsc::channel(1);
        let profile = LoggedInProfile {
            uuid: uuid::Uuid::nil(),
            name: "sequential-prewarm".to_string(),
        };
        let (session_id, _) = sessions.register(
            &profile,
            (0, 0),
            4,
            desired_chunk_set(0, 0, 4),
            tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        let policy = ChunkPipelinePolicy {
            chunk_prepare_batch_size: 16,
            chunk_generate_rate: 16,
            chunk_result_queue_size: 64,
            ..ChunkPipelinePolicy::default()
        };
        let resources = ChunkPipelineResources::with_limits(8, 8);
        resources.apply_runtime_control_action(crate::AutoscaleAction::ScaleDown, false);
        resources.apply_runtime_control_action(crate::AutoscaleAction::ScaleDown, false);
        assert_eq!(resources.cpu_limit(), 2);
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::clone(&sessions),
            session_id,
            0,
            0,
            0.0,
            4,
            resources,
            policy,
        );

        let prewarm_progress = stream.progress_notify();
        let prewarm_settled = prewarm_progress.notified();
        tokio::pin!(prewarm_settled);
        prewarm_settled.as_mut().enable();
        stream.dispatch_forward_prewarm();
        first_call_gate.wait_started().await;
        let parallel_start =
            tokio::time::timeout(Duration::from_secs(2), concurrent_call_started.notified()).await;
        first_call_gate.release();
        parallel_start.expect("prewarm should use the autoscaler CPU allowance concurrently");
        let forward_edge = (-4..=4).map(|x| (x, 5)).collect::<Vec<_>>();
        tokio::time::timeout(Duration::from_secs(3), prewarm_settled)
            .await
            .expect("forward prewarm completion must publish progress");
        assert!(
            forward_edge
                .iter()
                .all(|chunk| sessions.prepared_chunk(*chunk).is_some()),
            "forward prewarm completion event must cover the full edge"
        );

        assert!(
            calls.load(Ordering::Acquire) > 0,
            "prewarm should exercise generated chunk path"
        );
        assert_eq!(max_active.load(Ordering::Acquire), 2);
    }

    #[tokio::test]
    async fn forward_prewarm_releases_remaining_claims_when_new_session_joins() {
        let registry = Arc::new(air_block_registry());
        let calls = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let first_call_gate = Arc::new(GenerationGate::new());
        let world = Arc::new(Mutex::new(
            WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 256).with_generator(
                Arc::new(CountingConcurrentGenerator {
                    calls: Arc::clone(&calls),
                    active: Arc::clone(&active),
                    max_active: Arc::clone(&max_active),
                    concurrent_call_started: None,
                    first_call_gate: Some(Arc::clone(&first_call_gate)),
                    gate_first_call: AtomicBool::new(true),
                }),
            ),
        ));
        let sessions = Arc::new(SessionRegistry::new());
        let (tx, _rx) = mpsc::channel(1);
        let profile = LoggedInProfile {
            uuid: uuid::Uuid::nil(),
            name: "prewarm-owner".to_string(),
        };
        let (session_id, _) = sessions.register(
            &profile,
            (0, 0),
            4,
            desired_chunk_set(0, 0, 4),
            tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        let policy = ChunkPipelinePolicy {
            chunk_prepare_batch_size: 16,
            chunk_generate_rate: 16,
            chunk_result_queue_size: 64,
            ..ChunkPipelinePolicy::default()
        };
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::clone(&sessions),
            session_id,
            0,
            0,
            0.0,
            4,
            ChunkPipelineResources::with_limits(8, 8),
            policy,
        );

        stream.dispatch_forward_prewarm();
        first_call_gate.wait_started().await;

        let (secondary_tx, _secondary_rx) = mpsc::channel(1);
        let secondary_profile = LoggedInProfile {
            uuid: uuid::Uuid::from_u128(1),
            name: "prewarm-visible-join".to_string(),
        };
        let (_secondary_id, _) = sessions.register(
            &secondary_profile,
            (0, 0),
            4,
            desired_chunk_set(0, 0, 4),
            secondary_tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        let prewarm_progress = stream.progress_notify();
        let prewarm_settled = prewarm_progress.notified();
        tokio::pin!(prewarm_settled);
        prewarm_settled.as_mut().enable();
        first_call_gate.release();

        tokio::time::timeout(Duration::from_secs(2), prewarm_settled)
            .await
            .expect("new visible session must wake background prewarm cancellation");

        let forward_edge = (-4..=4).map(|x| (x, 5)).collect::<Vec<_>>();
        let mut cached_count = 0;
        let mut claimable_count = 0;
        for chunk in &forward_edge {
            match sessions.prepared_chunk_or_claim(*chunk) {
                PreparedChunkClaimResult::Cached => {
                    cached_count += 1;
                }
                PreparedChunkClaimResult::Claimed(claim) => {
                    claimable_count += 1;
                    sessions.release_prepared_chunk_claim(*chunk, claim);
                }
                PreparedChunkClaimResult::InFlight => {}
            }
        }

        assert!(
            cached_count < forward_edge.len(),
            "prewarm should not finish the entire forward edge after a newer visible session joins"
        );
        assert!(
            claimable_count > 0,
            "remaining forward edge chunks should be claimable by visible work"
        );
    }

    #[tokio::test]
    async fn crossing_keeps_inflight_nearest_prewarm_result_for_new_visible_edge() {
        let registry = Arc::new(air_block_registry());
        let calls = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let first_call_gate = Arc::new(GenerationGate::new());
        let world = Arc::new(Mutex::new(
            WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 256).with_generator(
                Arc::new(CountingConcurrentGenerator {
                    calls,
                    active,
                    max_active,
                    concurrent_call_started: None,
                    first_call_gate: Some(Arc::clone(&first_call_gate)),
                    gate_first_call: AtomicBool::new(true),
                }),
            ),
        ));
        let sessions = Arc::new(SessionRegistry::new());
        let (tx, _rx) = mpsc::channel(1);
        let profile = LoggedInProfile {
            uuid: uuid::Uuid::nil(),
            name: "crossing-prewarm".to_string(),
        };
        let (session_id, _) = sessions.register(
            &profile,
            (0, 0),
            4,
            desired_chunk_set(0, 0, 4),
            tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::clone(&sessions),
            session_id,
            0,
            0,
            0.0,
            4,
            ChunkPipelineResources::with_limits(8, 8),
            ChunkPipelinePolicy::default(),
        );

        stream.dispatch_forward_prewarm();
        first_call_gate.wait_started().await;

        let prewarm_progress = stream.progress_notify();
        let prewarm_settled = prewarm_progress.notified();
        tokio::pin!(prewarm_settled);
        prewarm_settled.as_mut().enable();
        stream.replan_center(-1, 0, 0.0);
        first_call_gate.release();

        tokio::time::timeout(Duration::from_secs(2), prewarm_settled)
            .await
            .expect("center crossing must settle the old prewarm batch");
        assert!(
            sessions.prepared_chunk((-5, -4)).is_some(),
            "the prewarm already producing the new visible edge must publish its result"
        );
        assert!(
            sessions.prepared_chunk((4, -5)).is_none(),
            "remaining work for the old center must be cancelled"
        );
    }

    #[tokio::test]
    async fn latest_same_center_session_owns_shared_forward_prewarm() {
        let registry = Arc::new(air_block_registry());
        let calls = Arc::new(AtomicUsize::new(0));
        let world = Arc::new(Mutex::new(
            WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 256).with_generator(
                Arc::new(CountingGenerator {
                    calls: Arc::clone(&calls),
                }),
            ),
        ));
        let sessions = Arc::new(SessionRegistry::new());
        let view_distance = 4;
        let (primary_tx, _primary_rx) = mpsc::channel(1);
        let primary = LoggedInProfile {
            uuid: uuid::Uuid::from_u128(1),
            name: "prewarm-primary".to_string(),
        };
        let (primary_session, _) = sessions.register(
            &primary,
            (0, 1),
            view_distance,
            desired_chunk_set(0, 1, view_distance),
            primary_tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 17.5),
        );
        let (secondary_tx, _secondary_rx) = mpsc::channel(1);
        let secondary = LoggedInProfile {
            uuid: uuid::Uuid::from_u128(2),
            name: "prewarm-secondary".to_string(),
        };
        let (secondary_session, _) = sessions.register(
            &secondary,
            (0, 1),
            view_distance,
            desired_chunk_set(0, 1, view_distance),
            secondary_tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 17.5),
        );
        let policy = ChunkPipelinePolicy {
            chunk_prepare_batch_size: 16,
            chunk_generate_rate: 16,
            chunk_result_queue_size: 64,
            ..ChunkPipelinePolicy::default()
        };
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::clone(&sessions),
            primary_session,
            0,
            1,
            0.0,
            view_distance,
            ChunkPipelineResources::with_limits(8, 8),
            policy,
        );

        stream.dispatch_forward_prewarm();

        assert_eq!(stream.prewarm_dispatched, 0);
        assert_eq!(calls.load(Ordering::Acquire), 0);

        stream.session_id = secondary_session;
        stream.dispatch_forward_prewarm();

        assert_eq!(
            stream.prewarm_dispatched,
            prewarm_edge_batch_limit(view_distance)
        );
        for chunk in (-4..=4).map(|x| (x, 6)) {
            match sessions.prepared_chunk_or_claim(chunk) {
                PreparedChunkClaimResult::InFlight | PreparedChunkClaimResult::Cached => {}
                PreparedChunkClaimResult::Claimed(claim) => {
                    sessions.release_prepared_chunk_claim(chunk, claim);
                    panic!("latest same-center session should claim forward chunk {chunk:?}");
                }
            }
        }
    }

    #[tokio::test]
    async fn moved_apart_two_client_stream_prewarms_next_owned_edge() {
        let registry = Arc::new(air_block_registry());
        let calls = Arc::new(AtomicUsize::new(0));
        let world = Arc::new(Mutex::new(
            WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 256).with_generator(
                Arc::new(CountingGenerator {
                    calls: Arc::clone(&calls),
                }),
            ),
        ));
        let sessions = Arc::new(SessionRegistry::new());
        let view_distance = 4;
        let (primary_tx, _primary_rx) = mpsc::channel(1);
        let primary = LoggedInProfile {
            uuid: uuid::Uuid::from_u128(1),
            name: "prewarm-positive-client".to_string(),
        };
        let (_primary_session, _) = sessions.register(
            &primary,
            (0, 1),
            view_distance,
            desired_chunk_set(0, 1, view_distance),
            primary_tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 17.5),
        );
        let (secondary_tx, _secondary_rx) = mpsc::channel(1);
        let secondary = LoggedInProfile {
            uuid: uuid::Uuid::from_u128(2),
            name: "prewarm-negative-client".to_string(),
        };
        let (secondary_session, _) = sessions.register(
            &secondary,
            (0, -1),
            view_distance,
            desired_chunk_set(0, -1, view_distance),
            secondary_tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, -17.5),
        );
        let policy = ChunkPipelinePolicy {
            chunk_prepare_batch_size: 16,
            chunk_generate_rate: 16,
            chunk_result_queue_size: 64,
            ..ChunkPipelinePolicy::default()
        };
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::clone(&sessions),
            secondary_session,
            0,
            -1,
            0.0,
            view_distance,
            ChunkPipelineResources::with_limits(8, 8),
            policy,
        );

        stream.dispatch_forward_prewarm();

        let next_negative_edge = (-4..=4).map(|x| (x, -6)).collect::<Vec<_>>();
        for chunk in &next_negative_edge {
            assert!(
                matches!(
                    sessions.prepared_chunk_or_claim(*chunk),
                    PreparedChunkClaimResult::InFlight | PreparedChunkClaimResult::Cached
                ),
                "moved-apart two-client prewarm should claim next negative edge chunk {chunk:?}"
            );
        }
        assert!(
            next_negative_edge
                .iter()
                .all(|chunk| !sessions.ticketed_chunks_sorted().contains(chunk)),
            "moved-apart prewarm must not expand either client's visible/ticketed view"
        );
    }

    #[tokio::test]
    async fn current_second_client_reuses_then_releases_prepared_spawn_window() {
        let registry = Arc::new(air_block_registry());
        let calls = Arc::new(AtomicUsize::new(0));
        let world = Arc::new(Mutex::new(
            WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 256).with_generator(
                Arc::new(CountingGenerator {
                    calls: Arc::clone(&calls),
                }),
            ),
        ));
        let sessions = Arc::new(SessionRegistry::new());
        let view_distance = 4;
        let policy = ChunkPipelinePolicy {
            chunk_send_rate: 128,
            chunk_load_rate: 128,
            chunk_generate_rate: 128,
            chunk_prepare_batch_size: 32,
            chunk_result_queue_size: 128,
            ..ChunkPipelinePolicy::default()
        };

        let (primary_tx, _primary_rx) = mpsc::channel(1);
        let primary = LoggedInProfile {
            uuid: uuid::Uuid::from_u128(1),
            name: "spawn-cache-primary".to_string(),
        };
        let (primary_session, _) = sessions.register(
            &primary,
            (0, 0),
            view_distance,
            desired_chunk_set(0, 0, view_distance),
            primary_tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        let (secondary_tx, _secondary_rx) = mpsc::channel(1);
        let secondary = LoggedInProfile {
            uuid: uuid::Uuid::from_u128(2),
            name: "spawn-cache-secondary".to_string(),
        };
        let (secondary_session, _) = sessions.register(
            &secondary,
            (0, 0),
            view_distance,
            desired_chunk_set(0, 0, view_distance),
            secondary_tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        let mut primary_stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::clone(&sessions),
            primary_session,
            0,
            0,
            0.0,
            view_distance,
            ChunkPipelineResources::with_limits(4, 4),
            policy,
        );
        let mut writer = tokio::io::sink();
        let mut primary_light_cache = LightCache::new();
        drive_stream_to_completion(
            &mut primary_stream,
            &mut writer,
            &mut primary_light_cache,
            Duration::from_secs(2),
            "primary spawn window should flush",
        )
        .await;
        assert!(primary_stream.is_complete());
        let spawn_window = desired_chunk_set(0, 0, view_distance);
        assert!(
            spawn_window
                .iter()
                .all(|chunk| sessions.prepared_chunk(*chunk).is_some()),
            "prepared frames must remain while the current second subscriber still needs them"
        );
        let calls_after_primary = calls.load(Ordering::Acquire);

        let mut secondary_stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::clone(&sessions),
            secondary_session,
            0,
            0,
            0.0,
            view_distance,
            ChunkPipelineResources::with_limits(4, 4),
            policy,
        );
        let mut secondary_light_cache = LightCache::new();
        drive_stream_to_completion(
            &mut secondary_stream,
            &mut writer,
            &mut secondary_light_cache,
            Duration::from_secs(2),
            "secondary spawn window should flush from shared prepared cache",
        )
        .await;

        assert_eq!(secondary_stream.emitted, 81);
        assert_eq!(secondary_stream.fetch_ms, 0);
        assert_eq!(secondary_stream.slow_fetch_chunks, 0);
        assert_eq!(secondary_stream.build_timing.light_compute_ms, 0);
        assert_eq!(secondary_stream.slow_light_compute_chunks, 0);
        assert_eq!(calls.load(Ordering::Acquire), calls_after_primary);
        assert!(
            spawn_window
                .iter()
                .all(|chunk| sessions.prepared_chunk(*chunk).is_none()),
            "prepared frames must be released after every current subscriber loaded them"
        );
    }

    #[tokio::test]
    async fn waiting_same_spawn_client_keeps_center_first_until_owner_caches_it() {
        let registry = Arc::new(air_block_registry());
        let world = Arc::new(Mutex::new(
            WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 32).with_generator(
                Arc::new(CountingGenerator {
                    calls: Arc::new(AtomicUsize::new(0)),
                }),
            ),
        ));
        let sessions = Arc::new(SessionRegistry::new());
        let view_distance = 1;
        let policy = ChunkPipelinePolicy {
            chunk_prepare_batch_size: 1,
            chunk_send_rate: 16,
            chunk_load_rate: 16,
            chunk_generate_rate: 16,
            chunk_result_queue_size: 16,
            ..ChunkPipelinePolicy::default()
        };

        let (primary_tx, _primary_rx) = mpsc::channel(1);
        let primary = LoggedInProfile {
            uuid: uuid::Uuid::from_u128(1),
            name: "same-spawn-owner".to_string(),
        };
        let (_primary_session, _) = sessions.register(
            &primary,
            (0, 0),
            view_distance,
            desired_chunk_set(0, 0, view_distance),
            primary_tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        let (secondary_tx, _secondary_rx) = mpsc::channel(1);
        let secondary = LoggedInProfile {
            uuid: uuid::Uuid::from_u128(2),
            name: "same-spawn-waiter".to_string(),
        };
        let (secondary_session, _) = sessions.register(
            &secondary,
            (0, 0),
            view_distance,
            desired_chunk_set(0, 0, view_distance),
            secondary_tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        let mut secondary_stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::clone(&sessions),
            secondary_session,
            0,
            0,
            0.0,
            view_distance,
            ChunkPipelineResources::with_limits(1, 1),
            policy,
        );
        let owner_claim = match sessions.prepared_chunk_or_claim((0, 0)) {
            PreparedChunkClaimResult::Claimed(claim) => claim,
            other => panic!("expected owner center claim, got {other:?}"),
        };

        secondary_stream.dispatch_available().await;

        assert_eq!(
            secondary_stream.dispatched, 0,
            "later same-spawn client should not duplicate prepare work before the earlier session has warmed the chunk"
        );
        assert_eq!(secondary_stream.ready.len(), 0);
        assert_eq!(secondary_stream.scheduler.in_flight_len(), 0);
        assert_eq!(secondary_stream.scheduler.queued_len(), 9);

        sessions.cache_prepared_chunk(
            (0, 0),
            Arc::new(PreparedChunkFrame {
                frame: Bytes::from_static(b"center-chunk-frame"),
                light: None,
                herd_spawns: Vec::new(),
                hydrated_campfires: Vec::new(),
                packet_data_len: 0,
                build_timing: ChunkBuildTiming::default(),
                write_timing: ChunkWriteTiming::default(),
            }),
        );
        assert!(sessions.release_prepared_chunk_claim((0, 0), owner_claim));
        secondary_stream.dispatch_available().await;

        let ready = secondary_stream
            .ready
            .values()
            .map(|result| {
                (
                    result.request.priority.sequence,
                    (result.request.chunk_x, result.request.chunk_z),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(ready, vec![(0, (0, 0))]);
    }

    #[tokio::test]
    async fn prepare_worker_panic_publishes_failure_and_releases_claim() {
        let registry = Arc::new(air_block_registry());
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let sessions = Arc::new(SessionRegistry::new());
        let chunk = (i32::MAX, 0);
        let (tx, _rx) = mpsc::channel(1);
        let (session_id, _) = sessions.register(
            &LoggedInProfile {
                uuid: uuid::Uuid::from_u128(1),
                name: "prepare-panic".to_string(),
            },
            chunk,
            0,
            HashSet::from([chunk]),
            tx,
            PlayerPose::new(f64::from(i32::MAX) * 16.0, DEFAULT_SPAWN_Y, 0.5),
        );
        let mut stream = ChunkStreamState::new(
            world,
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::clone(&sessions),
            session_id,
            chunk.0,
            chunk.1,
            0.0,
            0,
            ChunkPipelineResources::with_limits(1, 1),
            ChunkPipelinePolicy {
                chunk_prepare_batch_size: 1,
                chunk_result_queue_size: 1,
                ..ChunkPipelinePolicy::default()
            },
        );
        let progress = stream.progress_notify();
        let worker_finished = progress.notified();
        tokio::pin!(worker_finished);
        worker_finished.as_mut().enable();

        stream.dispatch_available().await;
        tokio::time::timeout(Duration::from_secs(1), worker_finished)
            .await
            .expect("panicked prepare worker must publish a terminal result");
        stream.drain_ready();

        assert_eq!(stream.ready.len(), 1);
        let mut writer = tokio::io::sink();
        let mut light_cache = LightCache::new();
        let error = stream
            .emit_next_ready(&mut writer, &mut light_cache)
            .await
            .expect_err("failed preparation must fail the stream");
        assert!(
            matches!(
                &error,
                ConnectionError::ChunkPreparation {
                    chunk_x,
                    chunk_z,
                    ..
                } if *chunk_x == i32::MAX && *chunk_z == 0
            ),
            "{error}"
        );
        assert!(!stream.scheduler.is_complete());
        let replacement = match sessions.prepared_chunk_or_claim(chunk) {
            PreparedChunkClaimResult::Claimed(claim) => claim,
            other => panic!("panicked worker leaked claim: {other:?}"),
        };
        assert!(sessions.release_prepared_chunk_claim(chunk, replacement));
    }

    #[tokio::test]
    async fn runtime_control_step_scales_prepare_dispatch_before_spawning_workers() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let policy = ChunkPipelinePolicy {
            chunk_prepare_batch_size: 4,
            chunk_result_queue_size: 8,
            chunk_send_rate: 16,
            ..ChunkPipelinePolicy::default()
        };
        let control = crate::RuntimeControlHandle::new(crate::RuntimeControlConfig {
            policy: crate::AutoscalePolicy {
                min_view_distance: 2,
                max_view_distance: 2,
                min_chunk_send_rate: 16,
                max_chunk_send_rate: 16,
                min_chunk_load_rate: 1,
                max_chunk_load_rate: 2,
                min_chunk_generate_rate: 64,
                max_chunk_generate_rate: 64,
                queue_pressure_percent: 1,
                scale_down_after_ticks: 1,
                ..crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced)
            },
            initial_limits: crate::RuntimeControlLimits {
                view_distance: 2,
                chunk_send_rate: 16,
                chunk_load_rate: 2,
                chunk_generate_rate: 64,
            },
        });
        let mut signals = control
            .take_signal_receiver()
            .expect("test owns runtime control receiver");
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::new(SessionRegistry::new()),
            1,
            0,
            0,
            0.0,
            2,
            ChunkPipelineResources::with_limits(1, 1),
            policy,
        )
        .with_runtime_control(Some(control.clone()));
        for _ in 0..4 {
            let request = stream.scheduler.poll_next().expect("queued chunk");
            stream.accept_result(ChunkPrepareResult {
                request,
                prepare_claim: None,
                fetch_ms: 0,
                pressure_flush: PressureFlushTiming::default(),
                staged: Vec::new(),
                outcome: ChunkPrepareOutcome::Absent,
            });
        }
        let mut writer = tokio::io::sink();
        let mut light_cache = LightCache::new();

        stream.observe_runtime_control();
        let owner_decision = observe_next_runtime_control_signal(&control, &mut signals).await;
        assert_eq!(owner_decision.action, crate::AutoscaleAction::ScaleDown);
        stream.step(&mut writer, &mut light_cache).await.unwrap();

        let snapshot = control.snapshot();
        assert_eq!(snapshot.limits.chunk_load_rate, 1);
        assert_eq!(stream.dispatched, 1);
        assert_eq!(stream.scheduler.in_flight_len(), 1);
        assert_eq!(stream.last_stop_reason, ChunkPipelineStopReason::LoadBudget);
    }

    #[tokio::test]
    async fn successful_retry_clears_pressure_staged_for_chunk() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
            Arc::clone(&registry),
            1,
        )));
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::new(SessionRegistry::new()),
            1,
            0,
            0,
            0.0,
            0,
            ChunkPipelineResources::with_limits(1, 1),
            ChunkPipelinePolicy::default(),
        );
        let request = stream.scheduler.poll_next().expect("request");
        stream.accept_result(ChunkPrepareResult {
            request,
            prepare_claim: None,
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming::default(),
            staged: vec![(0, 0), (1, 0)],
            outcome: ChunkPrepareOutcome::Backpressured,
        });
        let mut writer = tokio::io::sink();
        let mut light_cache = LightCache::new();

        assert_eq!(
            stream
                .emit_next_ready(&mut writer, &mut light_cache)
                .await
                .unwrap(),
            EmitReadyResult::Blocked
        );
        assert!(stream.pressure_staged_contains((0, 0)));
        assert!(stream.pressure_staged_contains((1, 0)));

        let request = stream.scheduler.poll_next().expect("deferred request");
        stream.accept_result(ChunkPrepareResult {
            request,
            prepare_claim: None,
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming::default(),
            staged: vec![(0, 0)],
            outcome: ChunkPrepareOutcome::Ready(Box::new(PreparedChunkFrame {
                frame: Bytes::new(),
                light: None,
                herd_spawns: Vec::new(),
                hydrated_campfires: Vec::new(),
                packet_data_len: 0,
                build_timing: ChunkBuildTiming::default(),
                write_timing: ChunkWriteTiming::default(),
            })),
        });

        assert_eq!(
            stream
                .emit_next_ready(&mut writer, &mut light_cache)
                .await
                .unwrap(),
            EmitReadyResult::SentPacket
        );
        assert!(stream.pressure_staged_is_empty());
        assert!(stream.pressure_staged_by_chunk.is_empty());
        assert_eq!(stream.emitted, 1);
    }

    #[tokio::test]
    async fn concurrent_pressure_flush_replans_after_stale_region_replace() {
        let registry = Arc::new(air_block_registry());
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("region")).unwrap();
        let mut storage = WorldStorage::open_with_capacity(temp.path(), Arc::clone(&registry), 32)
            .expect("open world storage");
        for x in 0..32 {
            storage
                .insert_generated_chunk(
                    ChunkPos { x, z: 0 },
                    Chunk::empty(ChunkPos { x, z: 0 }, BlockStateId(0), biome.clone()),
                )
                .unwrap();
        }
        assert!(storage.dirty_chunk_cache_saturated());
        let world = Arc::new(Mutex::new(storage));
        let request = ChunkRequest {
            chunk_x: 32,
            chunk_z: 0,
            priority: ChunkPriority {
                ring: 0,
                sequence: 0,
            },
            generation: ChunkPipelineGeneration(1),
        };
        let mut flushes = Vec::new();
        for _ in 0..64 {
            flushes.push(tokio::spawn(flush_dirty_chunks_for_pressure(
                Arc::clone(&world),
                request,
                0,
            )));
        }

        let mut pressure_flush_runs = 0;
        for flush in flushes {
            pressure_flush_runs += flush
                .await
                .unwrap()
                .expect("pressure flush must replan stale region writes")
                .runs;
        }

        assert_eq!(pressure_flush_runs, 1);
        assert_eq!(world.lock().await.dirty_count(), 0);
    }

    #[tokio::test]
    async fn stream_recovers_deferred_chunk_after_dirty_pressure_clears() {
        let registry = Arc::new(air_block_registry());
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("region")).unwrap();
        let mut storage = WorldStorage::open_with_capacity(temp.path(), Arc::clone(&registry), 1)
            .unwrap()
            .with_generator(Arc::new(CountingGenerator {
                calls: Arc::clone(&calls),
            }));
        storage
            .insert_generated_chunk(
                ChunkPos { x: 0, z: 0 },
                Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
            )
            .unwrap();
        let world = Arc::new(Mutex::new(storage));
        let policy = ChunkPipelinePolicy {
            chunk_send_rate: 1,
            chunk_prepare_batch_size: 1,
            chunk_result_queue_size: 1,
            ..ChunkPipelinePolicy::default()
        };
        let mut stream = ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::new(SessionRegistry::new()),
            1,
            1,
            0,
            0.0,
            0,
            ChunkPipelineResources::with_limits(1, 1),
            policy,
        );
        let mut writer = tokio::io::sink();
        let mut light_cache = LightCache::new();

        let request = stream.scheduler.poll_next().expect("request");
        let result = prepare_chunk_request(
            request,
            Arc::clone(&world),
            None,
            None,
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            ChunkPipelineResources::with_limits(1, 1),
            Arc::clone(&stream.active_generation),
            0,
        )
        .await;
        assert!(matches!(result.outcome, ChunkPrepareOutcome::Backpressured));
        assert_eq!(result.pressure_flush.runs, 1);
        assert_eq!(result.pressure_flush.planned_chunks, 1);
        assert_eq!(result.pressure_flush.flushed_chunks, 1);
        stream.accept_result(result);
        assert_eq!(
            stream
                .emit_next_ready(&mut writer, &mut light_cache)
                .await
                .unwrap(),
            EmitReadyResult::Blocked
        );

        assert_eq!(stream.emitted, 0);
        assert_eq!(stream.pressure_abandoned, 0);
        assert_eq!(calls.load(Ordering::Acquire), 0);
        {
            let storage = world.lock().await;
            assert_eq!(storage.dirty_count(), 0);
        }

        drive_stream_to_completion(
            &mut stream,
            &mut writer,
            &mut light_cache,
            Duration::from_secs(2),
            "deferred chunk should recover after dirty pressure clears",
        )
        .await;

        assert!(stream.is_complete());
        assert_eq!(stream.emitted, 1);
        assert_eq!(stream.absent, 0);
        assert_eq!(stream.pressure_abandoned, 0);
        assert!(stream.pressure_staged_is_empty());
        assert!(stream.pressure_staged_by_chunk.is_empty());
        assert_eq!(calls.load(Ordering::Acquire), 1);
    }
}

#[cfg(test)]
#[path = "chunk_stream_autoscale_tests.rs"]
mod autoscale_tests;

#[cfg(test)]
#[path = "chunk_stream_world_handle_tests.rs"]
mod world_handle_tests;
