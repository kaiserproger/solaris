use super::*;
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
    Cached,
    Load,
    Generate,
}

impl ChunkPrepareBudgetClass {
    fn stop_reason(self) -> ChunkPipelineStopReason {
        match self {
            Self::Cached => ChunkPipelineStopReason::BatchLimit,
            Self::Load => ChunkPipelineStopReason::LoadBudget,
            Self::Generate => ChunkPipelineStopReason::GenerateBudget,
        }
    }
}

const INITIAL_CHUNK_MIN_RING: i32 = 2;
const CHUNK_STAGE_SLOW_MS: u64 = 50;
const CHUNK_BACKPRESSURE_COOLDOWN_TURNS: usize = 8;
const CHUNK_BACKPRESSURE_MAX_COOLDOWN_TURNS: usize = 64;
const CHUNK_BACKPRESSURE_MAX_RETRIES: usize = 16;
pub(super) struct ChunkStreamState {
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
    resources: ChunkPipelineResources,
    active_generation: Arc<AtomicU64>,
    result_tx: mpsc::Sender<ChunkPrepareResult>,
    result_rx: mpsc::Receiver<ChunkPrepareResult>,
    ready: BTreeMap<u32, ChunkPrepareResult>,
    pressure_retries: HashMap<(i32, i32), usize>,
    pressure_cooldowns: HashMap<(i32, i32), usize>,
    policy: ChunkPipelinePolicy,
    configured_prepare_batch_size: usize,
    prepare_limit_stop_reason: ChunkPipelineStopReason,
    runtime_control: Option<crate::RuntimeControlHandle>,
    result_queue_size: usize,
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

struct ChunkPrepareResult {
    request: crate::ChunkRequest,
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
    if passive_chunk_spawns(chunk_pos)
        && let Some(surface) = land_surface
    {
        let surfaces = LandSpawnSurfaces {
            preferred: surface,
            fallbacks: land_fallback_surfaces,
        };
        plan_group_spawns(
            chunk,
            surfaces,
            passable,
            "creature",
            rules,
            entity_types,
            &mut spawns,
        );
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
    if !hostile_spawn_light_allows(chunk, lx, y, lz, passable) {
        return;
    }
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

fn hostile_spawn_light_allows(
    chunk: &Chunk,
    lx: u8,
    y: i32,
    lz: u8,
    passable: &[BlockStateId],
) -> bool {
    if (chunk.pos.x, chunk.pos.z) == (0, 0) {
        return true;
    }
    ((y + 3)..=(y + 8).min(mc_world::MAX_Y - 1)).any(|roof_y| {
        chunk
            .get_block(lx, roof_y, lz)
            .is_some_and(|state| !passable.contains(&state))
    })
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
    let section = ((y - mc_world::MIN_Y) / 16).clamp(0, mc_world::SECTION_COUNT as i32 - 1);
    let section = chunk.biomes.get(section as usize)?;
    let local_y = (y - mc_world::MIN_Y).rem_euclid(16) as u8 / 4;
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
        let scheduler = ChunkScheduler::new(prioritized_spiral(
            center_cx,
            center_cz,
            view_distance,
            direction_yaw,
        ));
        let active_generation = Arc::new(AtomicU64::new(scheduler.current_generation().0));

        Self {
            world,
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
            sessions,
            session_id,
            resources,
            active_generation,
            result_tx,
            result_rx,
            ready: BTreeMap::new(),
            pressure_retries: HashMap::new(),
            pressure_cooldowns: HashMap::new(),
            policy,
            configured_prepare_batch_size: policy.chunk_prepare_batch_size.max(1),
            prepare_limit_stop_reason: ChunkPipelineStopReason::BatchLimit,
            runtime_control: None,
            result_queue_size: policy.chunk_result_queue_size,
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
            max_in_flight: 0,
            max_ready: 0,
            last_stop_reason: ChunkPipelineStopReason::QueueEmpty,
            wait_for_first_chunk: true,
            summary_logged: false,
        }
    }

    pub(super) fn with_runtime_control(
        mut self,
        runtime_control: Option<crate::RuntimeControlHandle>,
    ) -> Self {
        self.runtime_control = runtime_control;
        self
    }

    pub(super) fn is_complete(&self) -> bool {
        self.scheduler.is_complete()
    }

    pub(super) fn replan_center(
        &mut self,
        center_cx: i32,
        center_cz: i32,
        direction_yaw: f32,
    ) -> Vec<(i32, i32)> {
        if (self.center_cx, self.center_cz) == (center_cx, center_cz) {
            if (self.direction_yaw - direction_yaw).abs() >= 22.5 && !self.scheduler.is_complete() {
                self.ready.clear();
                self.reset_pressure_tracking();
                self.scheduler.replace_view(prioritized_spiral(
                    center_cx,
                    center_cz,
                    self.view_distance,
                    direction_yaw,
                ));
                self.active_generation
                    .store(self.scheduler.current_generation().0, Ordering::Release);
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
        self.ready.clear();
        self.reset_pressure_tracking();
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
        self.ready.clear();
        self.reset_pressure_tracking();
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
        self.ready.clear();
        self.reset_pressure_tracking();
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
        self.max_in_flight = 0;
        self.max_ready = 0;
        self.last_stop_reason = ChunkPipelineStopReason::QueueEmpty;
        self.wait_for_first_chunk = false;
        self.summary_logged = false;
    }

    pub(super) async fn step<W>(
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
            self.last_stop_reason = ChunkPipelineStopReason::Complete;
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
        let mut processed = 0usize;
        while processed < limit {
            match self.emit_next_ready(writer, light_cache).await? {
                EmitReadyResult::SentPacket | EmitReadyResult::DrainedNoPacket => processed += 1,
                EmitReadyResult::Blocked | EmitReadyResult::Empty => break,
            }
        }
        if processed == limit && !self.ready.is_empty() {
            self.last_stop_reason = ChunkPipelineStopReason::SendBudget;
        }
        Ok(processed > 0)
    }

    fn observe_runtime_control(&mut self) -> Vec<(i32, i32)> {
        let Some(runtime_control) = self.runtime_control.clone() else {
            return Vec::new();
        };
        let resources = self.resources.metrics().snapshot();
        let decision = runtime_control.observe(crate::RuntimeControlInput {
            tick_ms: 0,
            queued_chunks: self
                .ready
                .len()
                .saturating_add(self.scheduler.in_flight_len()),
            queue_capacity: self.result_queue_size.max(1),
            active_workers: resources.active_cpu,
            worker_capacity: self.policy.chunk_worker_threads.max(1),
            memory_used_mb: 0,
            memory_limit_mb: 0,
            first_chunk_ms: self.first_chunk_ms,
        });
        let memory_pressure_active = decision.pressure == Some(crate::AutoscalePressure::Memory);
        let should_shed_memory =
            memory_pressure_active && decision.action == crate::AutoscaleAction::ScaleDown;
        if should_shed_memory {
            self.shed_memory_pressure_work();
        }
        let unloads = self.apply_runtime_control_limits(decision.limits);
        self.memory_pressure_active = memory_pressure_active;
        if memory_pressure_active {
            self.last_stop_reason = ChunkPipelineStopReason::MemoryPressure;
        }
        unloads
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
        let mut cooldown_deferrals = 0usize;
        let mut budget_deferrals = 0usize;
        loop {
            if self.scheduler.in_flight_len() >= self.result_queue_size {
                self.last_stop_reason = ChunkPipelineStopReason::QueueFull;
                break;
            }
            if self.memory_pressure_active {
                self.last_stop_reason = ChunkPipelineStopReason::MemoryPressure;
                break;
            }
            if dispatched_this_turn >= self.policy.chunk_prepare_batch_size {
                self.last_stop_reason = self.prepare_limit_stop_reason;
                break;
            }
            if self.policy.chunk_prepare_budget_ms > 0
                && started.elapsed().as_millis() as u64 >= self.policy.chunk_prepare_budget_ms
            {
                self.last_stop_reason = ChunkPipelineStopReason::TimeBudget;
                break;
            }
            let Some(request) = self.scheduler.poll_next() else {
                self.last_stop_reason = if self.scheduler.in_flight_len() == 0 {
                    ChunkPipelineStopReason::Complete
                } else {
                    ChunkPipelineStopReason::QueueEmpty
                };
                break;
            };
            let prepared = self
                .sessions
                .prepared_chunk((request.chunk_x, request.chunk_z));
            let budget_class = if prepared.is_some() {
                ChunkPrepareBudgetClass::Cached
            } else {
                self.classify_prepare_budget(request).await
            };
            let budget_exhausted = match budget_class {
                ChunkPrepareBudgetClass::Cached => false,
                ChunkPrepareBudgetClass::Load => {
                    load_dispatched_this_turn >= self.policy.chunk_load_rate as usize
                }
                ChunkPrepareBudgetClass::Generate => {
                    generate_dispatched_this_turn >= self.policy.chunk_generate_rate as usize
                }
            };
            if budget_exhausted {
                let stop_reason = budget_class.stop_reason();
                if !self.scheduler.defer(request) {
                    self.last_stop_reason = stop_reason;
                    break;
                }
                budget_deferrals += 1;
                if budget_deferrals >= self.scheduler.queued_len().max(1) {
                    self.last_stop_reason = stop_reason;
                    break;
                }
                continue;
            }
            if let Some(prepared) = prepared {
                self.accept_result(ChunkPrepareResult {
                    request,
                    fetch_ms: 0,
                    pressure_flush: PressureFlushTiming::default(),
                    staged: Vec::new(),
                    outcome: ChunkPrepareOutcome::Ready(Box::new(prepared.prepared_cache_hit())),
                });
                dispatched_this_turn += 1;
                self.dispatched += 1;
                budget_deferrals = 0;
                cooldown_deferrals = 0;
                continue;
            }
            if self.defer_for_pressure_cooldown(request) {
                cooldown_deferrals += 1;
                if cooldown_deferrals >= self.scheduler.queued_len().max(1) {
                    self.last_stop_reason = ChunkPipelineStopReason::QueueEmpty;
                    break;
                }
                continue;
            }
            match budget_class {
                ChunkPrepareBudgetClass::Cached => {}
                ChunkPrepareBudgetClass::Load => load_dispatched_this_turn += 1,
                ChunkPrepareBudgetClass::Generate => generate_dispatched_this_turn += 1,
            }
            self.spawn_prepare_worker(request);
            dispatched_this_turn += 1;
            self.dispatched += 1;
            budget_deferrals = 0;
            cooldown_deferrals = 0;
        }
        self.max_in_flight = self.max_in_flight.max(self.scheduler.in_flight_len());
    }

    async fn classify_prepare_budget(&self, request: ChunkRequest) -> ChunkPrepareBudgetClass {
        let storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::ChunkPrepare,
            "chunk prepare budget classify",
            Instant::now(),
            self.world.lock().await,
        );
        match storage.plan_chunk_snapshot_without_generation(ChunkPos {
            x: request.chunk_x,
            z: request.chunk_z,
        }) {
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
        let in_flight = self.scheduler.in_flight_len();
        self.last_stop_reason = ChunkPipelineStopReason::MemoryPressure;
        if ready == 0 && in_flight == 0 {
            return;
        }

        self.ready.clear();
        self.reset_pressure_tracking();
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
        self.memory_pressure_shed_in_flight += in_flight;
    }

    fn spawn_prepare_worker(&self, request: ChunkRequest) {
        let world = Arc::clone(&self.world);
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
        let active_generation = Arc::clone(&self.active_generation);
        let compression = self.compression;
        let current_tick = self.sessions.simulation_tick();
        let tx = self.result_tx.clone();
        tokio::spawn(async move {
            let result = prepare_chunk_request(
                request,
                world,
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
            .await;
            let _ = tx.send(result).await;
        });
    }

    fn drain_ready(&mut self) {
        while let Ok(result) = self.result_rx.try_recv() {
            self.accept_result(result);
        }
    }

    fn accept_result(&mut self, result: ChunkPrepareResult) {
        if !self.scheduler.is_current(result.request) {
            return;
        }
        self.ready
            .entry(result.request.priority.sequence)
            .or_insert(result);
        self.max_ready = self.max_ready.max(self.ready.len());
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
        self.fetch_ms += result.fetch_ms;
        self.record_pressure_flush(result.pressure_flush);
        self.max_fetch_ms = self.max_fetch_ms.max(result.fetch_ms);
        if result.fetch_ms >= CHUNK_STAGE_SLOW_MS {
            self.slow_fetch_chunks += 1;
        }

        match result.outcome {
            ChunkPrepareOutcome::Ready(prepared) => {
                self.clear_pressure_tracking((cx, cz));
                self.staged.extend(result.staged);
                if let Some(light) = prepared.light.clone() {
                    light_cache.insert(ChunkPos { x: cx, z: cz }, light);
                }
                let mut write_timing = prepared.write_timing;
                let socket_write_started = Instant::now();
                writer.write_all(&prepared.frame).await?;
                write_timing.socket_write_ms = socket_write_started.elapsed().as_millis() as u64;
                for (position, cooking) in &prepared.hydrated_campfires {
                    self.sessions
                        .restore_campfire_cooking(*position, cooking.clone());
                }
                self.loaded.insert((cx, cz));
                let mut visibility = self.sessions.mark_loaded(self.session_id, (cx, cz));
                visibility.extend(
                    self.sessions
                        .ensure_chunk_herd((cx, cz), &prepared.herd_spawns),
                );
                dispatch_visibility_commands(visibility);
                self.sessions
                    .cache_prepared_chunk((cx, cz), Arc::new((*prepared).clone()));
                self.record_stage_maxima(prepared.build_timing, write_timing);
                self.build_timing.add(prepared.build_timing);
                self.record_emitted(cx, cz, prepared.packet_data_len, write_timing);
            }
            ChunkPrepareOutcome::Absent => {
                self.clear_pressure_tracking((cx, cz));
                self.staged.extend(result.staged);
                self.absent += 1;
                info!(cx, cz, "no chunk in storage");
                self.scheduler.mark_finished(request);
                return Ok(EmitReadyResult::DrainedNoPacket);
            }
            ChunkPrepareOutcome::Backpressured => {
                self.set_pressure_staged((cx, cz), &result.staged);
                let retries = self.pressure_retries.entry((cx, cz)).or_default();
                *retries += 1;
                if *retries > CHUNK_BACKPRESSURE_MAX_RETRIES {
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
                let cooldown = (*retries * CHUNK_BACKPRESSURE_COOLDOWN_TURNS)
                    .min(CHUNK_BACKPRESSURE_MAX_COOLDOWN_TURNS);
                self.pressure_cooldowns.insert((cx, cz), cooldown);
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
                    cooldown_turns = cooldown,
                    "chunk preparation deferred by dirty chunk cache pressure"
                );
                return Ok(EmitReadyResult::Blocked);
            }
            ChunkPrepareOutcome::Failed(err) => {
                self.clear_pressure_tracking((cx, cz));
                warn!(cx, cz, error = %err, "chunk encode failed; skipping");
                self.scheduler.mark_finished(request);
                return Ok(EmitReadyResult::DrainedNoPacket);
            }
        }

        self.scheduler.mark_finished(request);
        Ok(EmitReadyResult::SentPacket)
    }

    fn defer_for_pressure_cooldown(&mut self, request: ChunkRequest) -> bool {
        let coord = (request.chunk_x, request.chunk_z);
        let Some(turns) = self.pressure_cooldowns.get_mut(&coord) else {
            return false;
        };
        if *turns == 0 {
            self.pressure_cooldowns.remove(&coord);
            return false;
        }
        *turns -= 1;
        if *turns == 0 {
            self.pressure_cooldowns.remove(&coord);
        }
        if !self.scheduler.defer(request) {
            self.pressure_cooldowns.remove(&coord);
            self.pressure_retries.remove(&coord);
            self.clear_pressure_staged(coord);
            return false;
        }
        true
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
        self.pressure_cooldowns.remove(&coord);
        self.clear_pressure_staged(coord);
    }

    fn reset_pressure_tracking(&mut self) {
        self.pressure_retries.clear();
        self.pressure_cooldowns.clear();
        self.pressure_staged_by_chunk.clear();
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
        build_timing: ChunkBuildTiming,
        write_timing: ChunkWriteTiming,
    ) {
        self.max_chunk_data_ms = self.max_chunk_data_ms.max(build_timing.chunk_data_ms);
        self.max_heightmap_ms = self.max_heightmap_ms.max(build_timing.heightmap_ms);
        self.max_light_compute_ms = self.max_light_compute_ms.max(build_timing.light_compute_ms);
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
        self.first_chunk_ms
            .get_or_insert_with(|| self.started.elapsed().as_millis() as u64);
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
            degraded_delivery = self.pressure_abandoned > 0,
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
            in_flight = self.scheduler.in_flight_len(),
            max_in_flight = self.max_in_flight,
            ready = self.ready.len(),
            max_ready = self.max_ready,
            stop_reason = ?self.last_stop_reason,
            first_chunk_ms = self.first_chunk_ms,
            ring1_complete_ms = self.ring1_complete_ms,
            ring2_complete_ms = self.ring2_complete_ms,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "view-distance window flushed",
        );
    }
}

impl Drop for ChunkStreamState {
    fn drop(&mut self) {
        self.active_generation.store(0, Ordering::Release);
    }
}

/// Iterate chunk positions around `(center_x, center_z)` outwards
/// to `view_distance` in chebyshev-ring order. The first cell is the
/// centre; subsequent yields are every cell on ring `r = 1`, then
/// every cell on ring `r = 2`, etc. Within a ring the order is
/// row-major over the bounding square — perceptually this still
/// "spreads" because each ring fills before the next starts.
/// Coverage is identical to a row-major scan: `(2*view_distance +
/// 1)²` cells total, each yielded exactly once.
pub(super) fn spiral_chunks(
    center_x: i32,
    center_z: i32,
    view_distance: i32,
) -> impl Iterator<Item = (i32, i32)> {
    let vd = view_distance.max(0);
    let mut out = Vec::with_capacity(((2 * vd + 1).pow(2)) as usize);
    out.push((center_x, center_z));
    for r in 1..=vd {
        for dz in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dz.abs()) == r {
                    out.push((center_x + dx, center_z + dz));
                }
            }
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
        return stale_chunk_result(request);
    }
    let loaded = match load_chunk_neighbourhood(
        Arc::clone(&world),
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
                fetch_ms: 0,
                pressure_flush: PressureFlushTiming::default(),
                staged: Vec::new(),
                outcome: ChunkPrepareOutcome::Failed(err),
            };
        }
    };

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
                fetch_ms,
                pressure_flush,
                staged,
                outcome: ChunkPrepareOutcome::Backpressured,
            };
        }
        return ChunkPrepareResult {
            request,
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
            fetch_ms,
            pressure_flush,
            staged,
            outcome: ChunkPrepareOutcome::Backpressured,
        };
    }

    if !is_active_request(request, &active_generation) {
        return stale_chunk_result(request);
    }

    let cpu_permit = match resources.acquire_cpu().await {
        Ok(permit) => permit,
        Err(_) => {
            return ChunkPrepareResult {
                request,
                fetch_ms,
                pressure_flush: PressureFlushTiming::default(),
                staged,
                outcome: ChunkPrepareOutcome::Failed("CPU worker pool closed".into()),
            };
        }
    };

    if !is_active_request(request, &active_generation) {
        return stale_chunk_result(request);
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

    ChunkPrepareResult {
        request,
        fetch_ms,
        pressure_flush: PressureFlushTiming::default(),
        staged,
        outcome,
    }
}

fn is_active_request(request: ChunkRequest, active_generation: &AtomicU64) -> bool {
    active_generation.load(Ordering::Acquire) == request.generation.0
}

fn stale_chunk_result(request: ChunkRequest) -> ChunkPrepareResult {
    ChunkPrepareResult {
        request,
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
    let plan_started = Instant::now();
    let plan = {
        let storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::ChunkPrepare,
            "chunk pressure flush plan",
            Instant::now(),
            world.lock().await,
        );
        if storage.world_root().is_none() || !storage.dirty_chunk_cache_saturated() {
            return Ok(PressureFlushTiming::default());
        }
        storage
            .plan_dirty_flush_at_tick(current_tick)
            .map_err(|err| err.to_string())?
    };
    let plan_ms = plan_started.elapsed().as_millis() as u64;
    if plan.is_empty() {
        return Ok(PressureFlushTiming::default());
    }
    let planned_chunks = plan.chunk_count();
    let write_started = Instant::now();
    let commit = crate::dirty_flush::write_dirty_flush_blocking(plan).await?;
    let write_ms = write_started.elapsed().as_millis() as u64;
    let commit_started = Instant::now();
    let flushed = {
        let mut storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::ChunkPrepare,
            "chunk pressure flush commit",
            Instant::now(),
            world.lock().await,
        );
        storage
            .commit_dirty_flush(commit)
            .map_err(|err| err.to_string())?
    };
    let commit_ms = commit_started.elapsed().as_millis() as u64;
    info!(
        cx = request.chunk_x,
        cz = request.chunk_z,
        planned_chunks,
        flushed,
        plan_ms,
        write_ms,
        commit_ms,
        "dirty pressure flush completed"
    );
    Ok(PressureFlushTiming {
        runs: 1,
        planned_chunks,
        flushed_chunks: flushed,
        plan_ms,
        write_ms,
        commit_ms,
    })
}

struct LoadedNeighbourhood {
    centre: Option<Arc<Chunk>>,
    neighbourhood: [[Option<Arc<Chunk>>; 3]; 3],
    staged: Vec<(i32, i32)>,
    fetch_ms: u64,
    backpressured: bool,
}

async fn load_chunk_neighbourhood(
    world: WorldHandle,
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
                    Err(err) => warn!(cx, cz, error = %err, "chunk commit failed; skipping"),
                }
            }
            Ok(None) => {}
            Err(err) => warn!(cx, cz, error = %err, "chunk read failed; skipping"),
        }
    }

    if centre.is_none()
        && !backpressured
        && let Some(generator) = generator.as_ref()
    {
        let can_cache = {
            let storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::ChunkPrepare,
                "chunk prepare generation pressure check",
                Instant::now(),
                world.lock().await,
            );
            storage.can_cache_new_chunk(ChunkPos { x: cx, z: cz })
        };
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
                warn!(cx, cz, error = %err, "chunk generation failed; skipping");
                return Ok(LoadedNeighbourhood {
                    centre: None,
                    neighbourhood,
                    staged,
                    fetch_ms: fetch_started.elapsed().as_millis() as u64,
                    backpressured: false,
                });
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
                Err(err) => warn!(cx, cz, error = %err, "generated chunk insert failed; skipping"),
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
        for dz in 0..3 {
            for dx in 0..3 {
                if neighbourhood[dz][dx].is_some() {
                    continue;
                }
                let ncx = cx + (dx as i32 - 1);
                let ncz = cz + (dz as i32 - 1);
                let pos = ChunkPos { x: ncx, z: ncz };
                let disk_plan = {
                    let storage = crate::lock_metrics::timed_guard(
                        crate::lock_metrics::LockMetricKind::ChunkPrepare,
                        "chunk prepare neighbour snapshot",
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
                    match storage.plan_chunk_snapshot_without_generation(pos) {
                        mc_world::ChunkSnapshotPlan::Cached(chunk) => {
                            neighbourhood[dz][dx] = Some(chunk);
                            staged.push((ncx, ncz));
                            continue;
                        }
                        mc_world::ChunkSnapshotPlan::Load(plan) => plan,
                    }
                };
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
                        warn!(cx = ncx, cz = ncz, error = %err, "neighbour chunk read failed; trying generator fallback");
                        None
                    }
                };
                if chunk.is_none() {
                    let can_generate = {
                        let storage = crate::lock_metrics::timed_guard(
                            crate::lock_metrics::LockMetricKind::ChunkPrepare,
                            "chunk prepare neighbour generation pressure check",
                            Instant::now(),
                            world.lock().await,
                        );
                        storage.can_cache_new_chunk(pos)
                    };
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
                        pos,
                        resources.clone(),
                        request,
                        Arc::clone(&active_generation),
                    )
                    .await
                    {
                        Ok(chunk) => chunk,
                        Err(err) => {
                            warn!(cx = ncx, cz = ncz, error = %err, "neighbour chunk generation failed; lighting may be partial");
                            None
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
                    storage.try_commit_chunk_snapshot(pos, chunk)
                };
                match committed {
                    Ok(Some(chunk)) => {
                        neighbourhood[dz][dx] = Some(chunk);
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
                        warn!(cx = ncx, cz = ncz, error = %err, "neighbour chunk commit failed; lighting may be partial")
                    }
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
        if let Some(baked) = ChunkLight::from_section_lights(&centre.section_lights) {
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
    use std::collections::BTreeMap;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::Mutex;

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

    fn test_biome_registry() -> Registry {
        Registry {
            id: Identifier::parse("minecraft:worldgen/biome").unwrap(),
            entries: vec![Identifier::parse("minecraft:plains").unwrap()],
        }
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
            &mc_data::entity_types::EntityTypeRegistry::default(),
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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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

        for _ in 0..CHUNK_BACKPRESSURE_MAX_RETRIES {
            let request = stream.scheduler.poll_next().expect("deferred request");
            stream.accept_result(ChunkPrepareResult {
                request,
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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
        assert!(stream.pressure_retries.is_empty());
        assert!(stream.pressure_staged_is_empty());
    }

    #[tokio::test]
    async fn pressure_cooldown_does_not_block_later_dispatches() {
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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
        stream.pressure_cooldowns.insert((0, 0), 1);

        stream.dispatch_available().await;

        assert!(stream.scheduler.in_flight_len() >= 1);
        assert!(!stream.pressure_cooldowns.contains_key(&(0, 0)));
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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
    async fn runtime_limits_reduce_ready_batch_send_budget() {
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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
        for _ in 0..3 {
            let request = stream.scheduler.poll_next().expect("queued chunk");
            stream.accept_result(ChunkPrepareResult {
                request,
                fetch_ms: 0,
                pressure_flush: PressureFlushTiming::default(),
                staged: Vec::new(),
                outcome: ChunkPrepareOutcome::Absent,
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

        assert_eq!(stream.absent, 1);
        assert_eq!(stream.ready.len(), 2);
        assert_eq!(stream.last_stop_reason, ChunkPipelineStopReason::SendBudget);
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
                queue_pressure_percent: 1,
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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
        for _ in 0..3 {
            let request = stream.scheduler.poll_next().expect("queued chunk");
            stream.accept_result(ChunkPrepareResult {
                request,
                fetch_ms: 0,
                pressure_flush: PressureFlushTiming::default(),
                staged: Vec::new(),
                outcome: ChunkPrepareOutcome::Absent,
            });
        }
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
        assert_eq!(stream.absent, 2);
        assert_eq!(stream.ready.len(), 1);
        assert_eq!(stream.last_stop_reason, ChunkPipelineStopReason::SendBudget);
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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
            Compression::Disabled,
            Arc::new(SessionRegistry::new()),
            1,
            0,
            0,
            0.0,
            2,
            ChunkPipelineResources::with_limits(1, 1),
            ChunkPipelinePolicy::default(),
        )
        .with_runtime_control(Some(control.clone()));

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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
                fetch_ms: 0,
                pressure_flush: PressureFlushTiming::default(),
                staged: Vec::new(),
                outcome: ChunkPrepareOutcome::Absent,
            });
        }
        assert_eq!(stream.ready.len(), 3);
        assert_eq!(stream.scheduler.in_flight_len(), 3);

        stream.observe_runtime_control();

        let snapshot = control.snapshot();
        assert_eq!(
            snapshot.last_decision.pressure,
            Some(crate::AutoscalePressure::Memory)
        );
        assert_eq!(stream.ready.len(), 0);
        assert_eq!(stream.scheduler.in_flight_len(), 0);
        assert!(stream.scheduler.queued_len() >= 3);
        assert_eq!(stream.memory_pressure_shed_runs, 1);
        assert_eq!(stream.memory_pressure_shed_ready, 3);
        assert_eq!(stream.memory_pressure_shed_in_flight, 3);
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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
        let request = stream.scheduler.poll_next().expect("queued chunk");
        stream.accept_result(ChunkPrepareResult {
            request,
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming::default(),
            staged: Vec::new(),
            outcome: ChunkPrepareOutcome::Absent,
        });
        stream.loaded.insert((3, 0));
        sessions.mark_loaded(session_id, (3, 0));

        let (mut client, mut server) = tokio::io::duplex(256);
        let mut light_cache = LightCache::new();

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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
        for _ in 0..3 {
            let request = stream.scheduler.poll_next().expect("queued chunk");
            stream.accept_result(ChunkPrepareResult {
                request,
                fetch_ms: 0,
                pressure_flush: PressureFlushTiming::default(),
                staged: Vec::new(),
                outcome: ChunkPrepareOutcome::Absent,
            });
        }
        let mut writer = tokio::io::sink();
        let mut light_cache = LightCache::new();

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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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

        for _ in 0..1024 {
            if stream.step(&mut writer, &mut light_cache).await.unwrap()
                == ChunkStreamStep::Complete
            {
                break;
            }
            tokio::task::yield_now().await;
        }

        assert!(stream.is_complete());
        assert_eq!(stream.emitted, 1);
        assert_eq!(stream.absent, 0);
        assert_eq!(stream.pressure_abandoned, 0);
        assert!(stream.pressure_staged_is_empty());
        assert!(stream.pressure_staged_by_chunk.is_empty());
        assert_eq!(calls.load(Ordering::Acquire), 1);
    }
}
