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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChunkStreamStep {
    Progress,
    Complete,
}

const INITIAL_CHUNK_MIN_RING: i32 = 2;
const CHUNK_STAGE_SLOW_MS: u64 = 50;

pub(super) struct ChunkStreamState {
    world: WorldHandle,
    biomes: Arc<Registry>,
    block_light: Option<Arc<BlockLightTable>>,
    passive_herd_surface: Option<mc_world::BlockStateId>,
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
    policy: ChunkPipelinePolicy,
    result_queue_size: usize,
    center_cx: i32,
    center_cz: i32,
    direction_yaw: f32,
    view_distance: i32,
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
    pub(super) packet_data_len: usize,
    pub(super) build_timing: ChunkBuildTiming,
    pub(super) write_timing: ChunkWriteTiming,
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
    Failed(String),
}

struct ChunkPrepareResult {
    request: crate::ChunkRequest,
    fetch_ms: u64,
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
        plan_group_spawns(
            chunk,
            surface,
            passable,
            "creature",
            rules,
            entity_types,
            &mut spawns,
        );
        plan_hostile_spawns(chunk, surface, passable, rules, entity_types, &mut spawns);
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
    surface: mc_world::BlockStateId,
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
    let Some((lx, y, lz)) = herd_spawn_surface(chunk, surface, passable, h) else {
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
    surface: mc_world::BlockStateId,
    passable: &[BlockStateId],
    group: &str,
    rules: &mc_data::biomes::BiomeSpawnRules,
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
    out: &mut Vec<HerdSpawn>,
) {
    let chunk_pos = (chunk.pos.x, chunk.pos.z);
    let slot_base = out.len() as u8;
    let h = herd_hash(chunk_pos, slot_base, 0x5350_4157_4E00_0000);
    let Some((lx, y, lz)) = herd_spawn_surface(chunk, surface, passable, h) else {
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

fn herd_surface_y(chunk: &Chunk, lx: u8, lz: u8, surface: mc_world::BlockStateId) -> Option<i32> {
    if let Some(y) = chunk.highest_opaque_y(lx, lz)
        && chunk.get_block(lx, y, lz) == Some(surface)
    {
        return Some(y);
    }
    (mc_world::MIN_Y..mc_world::MAX_Y)
        .rev()
        .find(|&y| chunk.get_block(lx, y, lz) == Some(surface))
}

fn herd_spawn_surface(
    chunk: &Chunk,
    surface: BlockStateId,
    passable: &[BlockStateId],
    h: u64,
) -> Option<(u8, i32, u8)> {
    for attempt in 0..100u64 {
        let candidate = h.wrapping_add(attempt.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let lx = 3 + (candidate as u8 % 10);
        let lz = 3 + ((candidate >> 8) as u8 % 10);
        let Some(y) = herd_surface_y(chunk, lx, lz, surface) else {
            continue;
        };
        if herd_spawn_clearance(chunk, lx, y + 1, lz, surface, passable) {
            return Some((lx, y, lz));
        }
    }
    None
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

pub(crate) fn passive_entity_passable_blocks(blocks: &BlockRegistry) -> Vec<BlockStateId> {
    blocks
        .states()
        .filter(|state| passable_block_name(state.block.id.as_str()))
        .map(|state| state.id)
        .collect()
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
        block_light: Option<Arc<BlockLightTable>>,
        passive_herd_surface: Option<mc_world::BlockStateId>,
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
            block_light,
            passive_herd_surface,
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
            policy,
            result_queue_size: policy.chunk_result_queue_size,
            center_cx,
            center_cz,
            direction_yaw,
            view_distance,
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
        let mut visibility =
            self.sessions
                .replace_view(self.session_id, (center_cx, center_cz), desired);
        visibility.extend(self.sessions.mark_unloaded(self.session_id, &unloads));
        dispatch_visibility_commands(visibility);
        self.center_cx = center_cx;
        self.center_cz = center_cz;
        self.direction_yaw = direction_yaw;
        self.ready.clear();
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
        let initial_target = initial_window_target(self.view_distance);
        self.dispatch_available();
        self.drain_ready();

        let made_send_progress = self.emit_ready_batch(writer, light_cache).await?;
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
        let mut emitted = 0usize;
        while emitted < limit && self.emit_next_ready(writer, light_cache).await? {
            emitted += 1;
        }
        if emitted == limit && !self.ready.is_empty() {
            self.last_stop_reason = ChunkPipelineStopReason::SendBudget;
        }
        Ok(emitted > 0)
    }

    fn dispatch_available(&mut self) {
        self.dispatch_turns += 1;
        let started = Instant::now();
        let mut dispatched_this_turn = 0usize;
        loop {
            if self.scheduler.in_flight_len() >= self.result_queue_size {
                self.last_stop_reason = ChunkPipelineStopReason::QueueFull;
                break;
            }
            if dispatched_this_turn >= self.policy.chunk_prepare_batch_size {
                self.last_stop_reason = ChunkPipelineStopReason::BatchLimit;
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
            if let Some(prepared) = self
                .sessions
                .prepared_chunk((request.chunk_x, request.chunk_z))
            {
                self.accept_result(ChunkPrepareResult {
                    request,
                    fetch_ms: 0,
                    staged: Vec::new(),
                    outcome: ChunkPrepareOutcome::Ready(Box::new(prepared.prepared_cache_hit())),
                });
                dispatched_this_turn += 1;
                self.dispatched += 1;
                continue;
            }
            let world = Arc::clone(&self.world);
            let biomes = Arc::clone(&self.biomes);
            let block_light = self.block_light.as_ref().map(Arc::clone);
            let passive_herd_surface = self.passive_herd_surface;
            let passive_herd_water = Arc::clone(&self.passive_herd_water);
            let passive_herd_passable = Arc::clone(&self.passive_herd_passable);
            let passive_spawn_rules = Arc::clone(&self.passive_spawn_rules);
            let entity_types = Arc::clone(&self.entity_types);
            let resources = self.resources.clone();
            let active_generation = Arc::clone(&self.active_generation);
            let compression = self.compression;
            let tx = self.result_tx.clone();
            tokio::spawn(async move {
                let result = prepare_chunk_request(
                    request,
                    world,
                    biomes,
                    block_light,
                    passive_herd_surface,
                    passive_herd_water,
                    passive_herd_passable,
                    passive_spawn_rules,
                    entity_types,
                    compression,
                    resources,
                    active_generation,
                )
                .await;
                let _ = tx.send(result).await;
            });
            dispatched_this_turn += 1;
            self.dispatched += 1;
        }
        self.max_in_flight = self.max_in_flight.max(self.scheduler.in_flight_len());
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

    async fn emit_next_ready<W>(
        &mut self,
        writer: &mut W,
        light_cache: &mut LightCache,
    ) -> Result<bool, ConnectionError>
    where
        W: AsyncWriteExt + Unpin,
    {
        let Some((_, result)) = self.ready.pop_first() else {
            return Ok(false);
        };
        let request = result.request;
        let cx = request.chunk_x;
        let cz = request.chunk_z;
        self.fetch_ms += result.fetch_ms;
        self.max_fetch_ms = self.max_fetch_ms.max(result.fetch_ms);
        if result.fetch_ms >= CHUNK_STAGE_SLOW_MS {
            self.slow_fetch_chunks += 1;
        }
        self.staged.extend(result.staged);

        match result.outcome {
            ChunkPrepareOutcome::Ready(prepared) => {
                if let Some(light) = prepared.light.clone() {
                    light_cache.insert(ChunkPos { x: cx, z: cz }, light);
                }
                let mut write_timing = prepared.write_timing;
                write_timing.socket_write_ms = write_framed_chunk(writer, &prepared.frame).await?;
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
                self.absent += 1;
                info!(cx, cz, "no chunk in storage");
            }
            ChunkPrepareOutcome::Failed(err) => {
                warn!(cx, cz, error = %err, "chunk encode failed; skipping");
            }
        }

        self.scheduler.mark_finished(request);
        Ok(true)
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
    block_light: Option<Arc<BlockLightTable>>,
    passive_herd_surface: Option<mc_world::BlockStateId>,
    passive_herd_water: Arc<Vec<mc_world::BlockStateId>>,
    passive_herd_passable: Arc<Vec<BlockStateId>>,
    passive_spawn_rules: Arc<mc_data::biomes::BiomeSpawnRules>,
    entity_types: Arc<mc_data::entity_types::EntityTypeRegistry>,
    compression: Compression,
    resources: ChunkPipelineResources,
    active_generation: Arc<AtomicU64>,
) -> ChunkPrepareResult {
    if !is_active_request(request, &active_generation) {
        return stale_chunk_result(request);
    }
    let (centre, neighbourhood, staged, fetch_ms) = match load_chunk_neighbourhood(
        Arc::clone(&world),
        request.chunk_x,
        request.chunk_z,
        resources.clone(),
        request,
        Arc::clone(&active_generation),
    )
    .await
    {
        Ok(loaded) => loaded,
        Err(err) => {
            return ChunkPrepareResult {
                request,
                fetch_ms: 0,
                staged: Vec::new(),
                outcome: ChunkPrepareOutcome::Failed(err),
            };
        }
    };

    let Some(centre) = centre else {
        return ChunkPrepareResult {
            request,
            fetch_ms,
            staged,
            outcome: ChunkPrepareOutcome::Absent,
        };
    };

    if !is_active_request(request, &active_generation) {
        return stale_chunk_result(request);
    }

    let cpu_permit = match resources.acquire_cpu().await {
        Ok(permit) => permit,
        Err(_) => {
            return ChunkPrepareResult {
                request,
                fetch_ms,
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
                    Some(table),
                    passive_herd_surface,
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
                None,
                passive_herd_surface,
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
        staged: Vec::new(),
        outcome: ChunkPrepareOutcome::Absent,
    }
}

type LoadedNeighbourhood = (
    Option<Arc<Chunk>>,
    [[Option<Arc<Chunk>>; 3]; 3],
    Vec<(i32, i32)>,
    u64,
);

async fn load_chunk_neighbourhood(
    world: WorldHandle,
    cx: i32,
    cz: i32,
    resources: ChunkPipelineResources,
    request: ChunkRequest,
    active_generation: Arc<AtomicU64>,
) -> Result<LoadedNeighbourhood, String> {
    if !is_active_request(request, &active_generation) {
        return Ok((
            None,
            std::array::from_fn(|_| std::array::from_fn(|_| None)),
            Vec::new(),
            0,
        ));
    }
    let fetch_started = Instant::now();
    let mut neighbourhood: [[Option<Arc<Chunk>>; 3]; 3] =
        std::array::from_fn(|_| std::array::from_fn(|_| None));
    let mut centre = None;
    let mut staged = Vec::new();
    let mut disk_plan = None;

    let generator = {
        let storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::ChunkPrepare,
            "chunk prepare snapshot",
            Instant::now(),
            world.lock().await,
        );
        if !is_active_request(request, &active_generation) {
            return Ok((
                None,
                neighbourhood,
                staged,
                fetch_started.elapsed().as_millis() as u64,
            ));
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
                        return Ok((
                            None,
                            neighbourhood,
                            staged,
                            fetch_started.elapsed().as_millis() as u64,
                        ));
                    }
                    storage.commit_chunk_snapshot(ChunkPos { x: cx, z: cz }, chunk)
                };
                match chunk {
                    Ok(chunk) => {
                        centre = Some(Arc::clone(&chunk));
                        neighbourhood[1][1] = Some(chunk);
                        staged.push((cx, cz));
                    }
                    Err(err) => warn!(cx, cz, error = %err, "chunk commit failed; skipping"),
                }
            }
            Ok(None) => {}
            Err(err) => warn!(cx, cz, error = %err, "chunk read failed; skipping"),
        }
    }

    if centre.is_none()
        && let Some(generator) = generator
    {
        let chunk = match generate_fresh_chunk(
            generator,
            ChunkPos { x: cx, z: cz },
            resources,
            request,
            Arc::clone(&active_generation),
        )
        .await
        {
            Ok(Some(chunk)) => chunk,
            Ok(None) => {
                return Ok((
                    None,
                    neighbourhood,
                    staged,
                    fetch_started.elapsed().as_millis() as u64,
                ));
            }
            Err(err) => {
                warn!(cx, cz, error = %err, "chunk generation failed; skipping");
                return Ok((
                    None,
                    neighbourhood,
                    staged,
                    fetch_started.elapsed().as_millis() as u64,
                ));
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
                return Ok((
                    None,
                    neighbourhood,
                    staged,
                    fetch_started.elapsed().as_millis() as u64,
                ));
            }
            if let Err(err) = storage.insert_generated_chunk(ChunkPos { x: cx, z: cz }, chunk) {
                warn!(cx, cz, error = %err, "generated chunk insert failed; skipping");
            }
            storage.cached_chunk_snapshot(ChunkPos { x: cx, z: cz })
        };
        if let Some(chunk) = chunk {
            centre = Some(Arc::clone(&chunk));
            neighbourhood[1][1] = Some(chunk);
            staged.push((cx, cz));
        }
    }

    Ok((
        centre,
        neighbourhood,
        staged,
        fetch_started.elapsed().as_millis() as u64,
    ))
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
        chunk.dirty = true;
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
    timing: ChunkBuildTiming,
}

#[allow(clippy::too_many_arguments)]
fn build_chunk_packet(
    centre: &Chunk,
    neighbourhood: &[[Option<Arc<Chunk>>; 3]; 3],
    biomes: &Registry,
    block_light: Option<&BlockLightTable>,
    passive_herd_surface: Option<mc_world::BlockStateId>,
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
    let light = match (block_light, workspace) {
        (Some(table), Some(ws)) => {
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
        }
        _ => LightData::empty(),
    };
    let herd_spawns = plan_passive_herd(
        centre,
        passive_herd_surface,
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
            block_entities: Vec::new(),
            light,
        },
        light: computed_light,
        herd_spawns,
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
        packet_data_len,
        build_timing: built.timing,
        write_timing: timing,
    })
}

async fn write_framed_chunk<W>(writer: &mut W, framed: &[u8]) -> Result<u64, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let socket_write_started = Instant::now();
    writer.write_all(framed).await?;
    Ok(socket_write_started.elapsed().as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_cache_hit_drops_historical_cpu_and_encode_timings() {
        let prepared = PreparedChunkFrame {
            frame: Bytes::from_static(b"chunk-frame"),
            light: None,
            herd_spawns: Vec::new(),
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
}
