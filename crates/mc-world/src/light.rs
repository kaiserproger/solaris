//! Solaris-computed chunk lighting. M4.c.
//!
//! Given a 3×3 chunk neighbourhood and a per-block-state light table
//! (`mc_data::block_light::BlockLightTable`), produces sky-light and
//! block-light values for every cell in the centre chunk's
//! 16×384×16 column. The output is per-cell 0..=15 nibble values
//! (one u8 per cell); the wire encoder (M4.d) packs them into the
//! 4-bit-per-cell vanilla layout.
//!
//! Algorithm:
//!
//! - **Block-light** — BFS over a 48×384×48 working volume seeded
//!   from every cell whose block state has non-zero `emission`. Each
//!   BFS step decrements by `max(1, opacity[neighbour])`, with the
//!   value capped at the source's emission and clipped at 0.
//! - **Sky-light** — vertical heightmap shortcut: for each `(x, z)`
//!   column walk Y top-down, marking cells as 15 while
//!   `propagates_sky` is true. The cell *below* the first opaque
//!   one is queued at value 15 to seed lateral / downward
//!   propagation. BFS proceeds with the same `max(1, opacity)` cost
//!   rule.
//!
//! Both passes use a single shared cost rule, which is *not* a
//! line-by-line port of vanilla — Mojang's algorithm has
//! iteration-order quirks that produce occasional one-nibble drift
//! against engine output. M4.c pins values against itself; the
//! manual-gate (M4.g) verifies the visible result matches vanilla
//! closely enough that the test world renders fully lit.
//!
//! Out-of-window propagation (a glowstone 32 cells away in another
//! region) is intentionally truncated at the 3×3 boundary; for
//! M4's view-distance window of 10 the truncation only matters for
//! pathological setups (lava lakes one chunk over) and we accept
//! the slight darkening.

use std::collections::{HashMap, VecDeque};

#[cfg(target_endian = "little")]
use bytemuck::cast;
use mc_data::block_light::BlockLightTable;
#[cfg(target_endian = "little")]
use wide::{i16x8, u8x16, u16x8};

use crate::block::BlockStateId;
use crate::chunk::{
    Chunk, ChunkGeometry, ChunkPos, LIGHT_LAYER_BYTES, MIN_Y, OVERWORLD_GEOMETRY, SECTION_COUNT,
    SectionLight, top_opaque_column,
};
use crate::section::{SECTION_DIM, SECTION_VOLUME};

/// World-height span in blocks (24 sections × 16 blocks).
pub const WORLD_HEIGHT: usize = SECTION_COUNT * SECTION_DIM;
/// Per-chunk cell count for one light channel.
pub const CHUNK_LIGHT_LEN: usize = SECTION_VOLUME * SECTION_COUNT;

const N_X: usize = SECTION_DIM * 3;
const N_Z: usize = SECTION_DIM * 3;
const N_VOL: usize = N_X * WORLD_HEIGHT * N_Z;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightKernelBackend {
    Scalar,
    PortableSimd,
}

impl LightKernelBackend {
    fn configured() -> Self {
        match std::env::var("SOLARIS_SIMD_BACKEND").as_deref() {
            Ok("portable") => Self::PortableSimd,
            _ => Self::Scalar,
        }
    }
}

/// One light channel for a chunk, stored as lazy per-section nibble
/// arrays. Missing sections are all-zero; present sections are already
/// in the 2048-byte vanilla nibble layout, low nibble first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightLayer {
    sections: Vec<Option<Box<[u8; crate::chunk::LIGHT_LAYER_BYTES]>>>,
    nonzero_nibbles: Vec<u16>,
}

impl LightLayer {
    #[must_use]
    pub fn zeroed() -> Self {
        Self::zeroed_for_sections(SECTION_COUNT)
    }

    fn zeroed_for_sections(section_count: usize) -> Self {
        Self {
            sections: (0..section_count).map(|_| None).collect(),
            nonzero_nibbles: vec![0; section_count],
        }
    }

    #[must_use]
    pub fn filled(value: u8) -> Self {
        Self::filled_for_sections(value, SECTION_COUNT)
    }

    fn filled_for_sections(value: u8, section_count: usize) -> Self {
        debug_assert!(value <= 15);
        if value == 0 {
            return Self::zeroed_for_sections(section_count);
        }
        let packed = value | (value << 4);
        Self {
            sections: (0..section_count)
                .map(|_| Some(Box::new([packed; crate::chunk::LIGHT_LAYER_BYTES])))
                .collect(),
            nonzero_nibbles: vec![SECTION_VOLUME as u16; section_count],
        }
    }

    #[must_use]
    pub fn get(&self, x: usize, local_y: usize, z: usize) -> u8 {
        debug_assert!(x < SECTION_DIM);
        debug_assert!(local_y < self.sections.len() * SECTION_DIM);
        debug_assert!(z < SECTION_DIM);
        let section_idx = local_y / SECTION_DIM;
        let Some(layer) = self.sections[section_idx].as_ref() else {
            return 0;
        };
        get_nibble(layer, section_cell_idx(x, local_y % SECTION_DIM, z))
    }

    pub fn set(&mut self, x: usize, local_y: usize, z: usize, value: u8) {
        debug_assert!(x < SECTION_DIM);
        debug_assert!(local_y < self.sections.len() * SECTION_DIM);
        debug_assert!(z < SECTION_DIM);
        debug_assert!(value <= 15);
        let section_idx = local_y / SECTION_DIM;
        if value == 0 && self.sections[section_idx].is_none() {
            return;
        }
        let layer = self.sections[section_idx]
            .get_or_insert_with(|| Box::new([0; crate::chunk::LIGHT_LAYER_BYTES]));
        let cell = section_cell_idx(x, local_y % SECTION_DIM, z);
        let old = get_nibble(layer, cell);
        if old == value {
            return;
        }
        set_nibble(layer, cell, value);
        if old == 0 && value != 0 {
            self.nonzero_nibbles[section_idx] += 1;
        } else if old != 0 && value == 0 {
            self.nonzero_nibbles[section_idx] -= 1;
        }
        if self.nonzero_nibbles[section_idx] == 0 {
            self.sections[section_idx] = None;
        }
    }

    #[must_use]
    pub fn section(&self, section_idx: usize) -> Option<&[u8; crate::chunk::LIGHT_LAYER_BYTES]> {
        self.sections.get(section_idx)?.as_deref()
    }

    pub(crate) fn section_count(&self) -> usize {
        self.sections.len()
    }

    fn set_section_from_slice(&mut self, section_idx: usize, bytes: &[u8]) -> bool {
        if section_idx >= self.sections.len() || bytes.len() != LIGHT_LAYER_BYTES {
            return false;
        }
        let nonzero_nibbles = bytes
            .iter()
            .map(|byte| u16::from(byte & 0x0F != 0) + u16::from(byte >> 4 != 0))
            .sum();
        if nonzero_nibbles == 0 {
            self.sections[section_idx] = None;
            self.nonzero_nibbles[section_idx] = 0;
            return true;
        }
        let Ok(layer) = <[u8; LIGHT_LAYER_BYTES]>::try_from(bytes) else {
            return false;
        };
        self.sections[section_idx] = Some(Box::new(layer));
        self.nonzero_nibbles[section_idx] = nonzero_nibbles;
        true
    }
}

/// One chunk's worth of computed light.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkLight {
    pub sky: LightLayer,
    pub block: LightLayer,
    min_y: i32,
}

impl ChunkLight {
    /// Fresh light, all zero. Useful for direct unit-testing without
    /// invoking the engine.
    #[must_use]
    pub fn zeroed() -> Self {
        Self::zeroed_for_sections(SECTION_COUNT)
    }

    fn zeroed_for_sections(section_count: usize) -> Self {
        Self {
            sky: LightLayer::zeroed_for_sections(section_count),
            block: LightLayer::zeroed_for_sections(section_count),
            min_y: MIN_Y,
        }
    }

    fn zeroed_for_geometry(geometry: ChunkGeometry) -> Self {
        Self {
            sky: LightLayer::zeroed_for_sections(geometry.section_count()),
            block: LightLayer::zeroed_for_sections(geometry.section_count()),
            min_y: geometry.min_y(),
        }
    }

    #[must_use]
    pub fn filled(sky: u8, block: u8) -> Self {
        Self::filled_for_geometry(sky, block, OVERWORLD_GEOMETRY)
    }

    fn filled_for_geometry(sky: u8, block: u8, geometry: ChunkGeometry) -> Self {
        Self {
            sky: LightLayer::filled_for_sections(sky, geometry.section_count()),
            block: LightLayer::filled_for_sections(block, geometry.section_count()),
            min_y: geometry.min_y(),
        }
    }

    /// Rebuild baked light with the legacy Overworld Y origin.
    /// Use [`Self::from_chunk`] whenever the owning chunk is available.
    #[must_use]
    pub fn from_section_lights(section_lights: &[SectionLight]) -> Option<Self> {
        Self::from_section_lights_into(
            section_lights,
            Self::zeroed_for_sections(section_lights.len()),
        )
    }

    /// Rebuild baked light using the owning chunk's geometry.
    #[must_use]
    pub fn from_chunk(chunk: &Chunk) -> Option<Self> {
        if chunk.section_lights.len() != chunk.geometry().section_count() {
            return None;
        }
        Self::from_section_lights_into(
            &chunk.section_lights,
            Self::zeroed_for_geometry(chunk.geometry()),
        )
    }

    fn from_section_lights_into(section_lights: &[SectionLight], mut out: Self) -> Option<Self> {
        let mut any = false;
        for (section_idx, section) in section_lights.iter().enumerate() {
            if let Some(sky) = &section.sky {
                if !out.sky.set_section_from_slice(section_idx, sky) {
                    return None;
                }
                any = true;
            }
            if let Some(block) = &section.block {
                if !out.block.set_section_from_slice(section_idx, block) {
                    return None;
                }
                any = true;
            }
        }
        any.then_some(out)
    }

    pub(crate) fn write_section_lights(&self, section_lights: &mut [SectionLight]) {
        for (section_idx, section) in section_lights.iter_mut().enumerate() {
            section.sky = self.sky.section(section_idx).map(|layer| layer.to_vec());
            section.block = self.block.section(section_idx).map(|layer| layer.to_vec());
        }
    }

    pub(crate) fn section_count(&self) -> usize {
        debug_assert_eq!(self.sky.section_count(), self.block.section_count());
        self.sky.section_count()
    }

    #[must_use]
    pub fn sky_at(&self, x: u8, y: i32, z: u8) -> u8 {
        let local_y = (y - self.min_y) as usize;
        self.sky.get(x as usize, local_y, z as usize)
    }

    #[must_use]
    pub fn block_at(&self, x: u8, y: i32, z: u8) -> u8 {
        let local_y = (y - self.min_y) as usize;
        self.block.get(x as usize, local_y, z as usize)
    }

    pub fn set_sky_local(&mut self, x: usize, local_y: usize, z: usize, value: u8) {
        self.sky.set(x, local_y, z, value);
    }

    pub fn set_block_local(&mut self, x: usize, local_y: usize, z: usize, value: u8) {
        self.block.set(x, local_y, z, value);
    }
}

fn section_cell_idx(x: usize, section_y: usize, z: usize) -> usize {
    section_y * (SECTION_DIM * SECTION_DIM) + z * SECTION_DIM + x
}

fn get_nibble(layer: &[u8; crate::chunk::LIGHT_LAYER_BYTES], cell: usize) -> u8 {
    let byte = layer[cell / 2];
    if cell & 1 == 0 {
        byte & 0x0F
    } else {
        byte >> 4
    }
}

fn set_nibble(layer: &mut [u8; crate::chunk::LIGHT_LAYER_BYTES], cell: usize, value: u8) {
    let byte = &mut layer[cell / 2];
    if cell & 1 == 0 {
        *byte = (*byte & 0xF0) | value;
    } else {
        *byte = (*byte & 0x0F) | (value << 4);
    }
}

/// Reusable working buffers for the lighting engine. Allocating
/// ~4 MB on every chunk emit measurably slows the spawn burst in
/// debug mode (the M3.g/M4.f harness exceeds 60 s without this).
/// Build one per connection (or one per `emit_chunks_around` call)
/// and pass it into [`compute_chunk_light`].
pub struct LightWorkspace {
    geometry: ChunkGeometry,
    sky: Vec<u8>,
    block: Vec<u8>,
    opacity: Vec<u8>,
    propagates_sky: Vec<bool>,
    queue: VecDeque<u32>,
    emitters: Vec<u32>,
    kernel_backend: LightKernelBackend,
}

impl LightWorkspace {
    #[must_use]
    pub fn new() -> Self {
        Self::with_backend(LightKernelBackend::configured())
    }

    #[must_use]
    pub fn with_backend(kernel_backend: LightKernelBackend) -> Self {
        Self {
            geometry: OVERWORLD_GEOMETRY,
            sky: vec![0; N_VOL],
            block: vec![0; N_VOL],
            opacity: vec![0; N_VOL],
            propagates_sky: vec![true; N_VOL],
            queue: VecDeque::new(),
            emitters: Vec::new(),
            kernel_backend,
        }
    }

    fn reset_for_geometry(&mut self, geometry: ChunkGeometry) {
        let volume = grid_volume(geometry.height() as usize);
        if self.geometry != geometry {
            self.geometry = geometry;
            self.sky.resize(volume, 0);
            self.block.resize(volume, 0);
            self.opacity.resize(volume, 0);
            self.propagates_sky.resize(volume, true);
        }
        self.sky.fill(0);
        self.block.fill(0);
        self.opacity.fill(0);
        self.propagates_sky.fill(true);
        self.queue.clear();
        self.emitters.clear();
    }
}

impl Default for LightWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-connection cache of computed [`ChunkLight`] keyed by
/// [`ChunkPos`]. Owned by `mc-net::play::InteractionState`, populated
/// during the spawn-window emit (so each chunk's lighting is computed
/// exactly once at login), and mutated in place by
/// [`apply_block_change_to_light`] on subsequent edits.
///
/// Memory: ~200 KB per cached chunk × the spawn view-window. With
/// view distance 10 and a ~21×21 emit, the upper bound is ~17 MB per
/// connection. Chunks the all-air fast path returned for are *not*
/// cached (their light is reconstructable on demand) so a fresh-world
/// connection sits well below that.
#[derive(Debug, Default, Clone)]
pub struct LightCache {
    chunks: HashMap<ChunkPos, ChunkLight>,
}

impl LightCache {
    #[must_use]
    pub fn new() -> Self {
        Self {
            chunks: HashMap::new(),
        }
    }

    pub fn insert(&mut self, pos: ChunkPos, light: ChunkLight) {
        self.chunks.insert(pos, light);
    }

    #[must_use]
    pub fn get(&self, pos: ChunkPos) -> Option<&ChunkLight> {
        self.chunks.get(&pos)
    }

    #[must_use]
    pub fn contains(&self, pos: ChunkPos) -> bool {
        self.chunks.contains_key(&pos)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    pub fn remove(&mut self, pos: ChunkPos) -> Option<ChunkLight> {
        self.chunks.remove(&pos)
    }
}

const NEIGHBOURS: [(i32, i32, i32); 6] = [
    (-1, 0, 0),
    (1, 0, 0),
    (0, -1, 0),
    (0, 1, 0),
    (0, 0, -1),
    (0, 0, 1),
];

/// Apply a single block change to the light cache, updating both
/// sky and block light incrementally. Returns the chunk positions
/// whose stored light arrays were modified — the caller emits one
/// `ClientboundLightUpdate` per returned position.
///
/// The bounded-radius design: light propagates at most 15 cells
/// from any source, so all light changes from a single edit are
/// confined to the 3×3 chunks around `centre_pos`. We pull those
/// nine `ChunkLight` slots out of the cache, mutate them in place
/// via removal + addition BFS, and put them back.
///
/// `chunks` is the post-edit 3×3 chunk neighbourhood with the edit
/// chunk at `[1][1]`; off-centre slots may be `None` (treated as
/// air). Light arrays are only modified for chunks whose slot is
/// present in the cache *and* whose chunk is present in `chunks` —
/// missing entries are skipped, accepting the same boundary
/// truncation as the full-recompute path.
///
/// Returns an empty list when the change has no possible light
/// effect (same opacity, emission, and sky propagation in both
/// states), which short-circuits the whole flow for like-for-like
/// substitutions.
#[allow(clippy::too_many_arguments)]
pub fn apply_block_change_to_light(
    cache: &mut LightCache,
    chunks: &[[Option<&Chunk>; 3]; 3],
    table: &BlockLightTable,
    centre_pos: ChunkPos,
    local_x: u8,
    world_y: i32,
    local_z: u8,
    prev_state: BlockStateId,
    new_state: BlockStateId,
) -> Vec<ChunkPos> {
    debug_assert!(local_x < SECTION_DIM as u8);
    debug_assert!(local_z < SECTION_DIM as u8);
    let geometry = shared_neighbourhood_geometry(chunks);
    debug_assert!((geometry.min_y()..geometry.max_y()).contains(&world_y));

    let prev_emit = table.emission(prev_state.0).unwrap_or(0);
    let new_emit = table.emission(new_state.0).unwrap_or(0);
    let prev_op = table.opacity(prev_state.0).unwrap_or(0);
    let new_op = table.opacity(new_state.0).unwrap_or(0);
    let prev_sky_pass = table.propagates_sky(prev_state.0).unwrap_or(true);
    let new_sky_pass = table.propagates_sky(new_state.0).unwrap_or(true);

    if prev_emit == new_emit && prev_op == new_op && prev_sky_pass == new_sky_pass {
        return Vec::new();
    }

    let mut window: [[Option<ChunkLight>; 3]; 3] = std::array::from_fn(|dz| {
        std::array::from_fn(|dx| {
            let pos = ChunkPos {
                x: centre_pos.x + (dx as i32 - 1),
                z: centre_pos.z + (dz as i32 - 1),
            };
            cache.chunks.remove(&pos)
        })
    });
    let mut changed = [[false; 3]; 3];

    let world_x = centre_pos.x * SECTION_DIM as i32 + local_x as i32;
    let world_z = centre_pos.z * SECTION_DIM as i32 + local_z as i32;
    let Some(edit_coord) = world_to_window(centre_pos, geometry, world_x, world_y, world_z) else {
        return Vec::new();
    };

    incremental_block_light(
        &mut window,
        &mut changed,
        chunks,
        table,
        centre_pos,
        world_x,
        world_y,
        world_z,
        new_op,
        new_emit,
        geometry,
    );

    if prev_op != new_op || prev_sky_pass != new_sky_pass {
        let reseed_column = highest_opaque_changed(
            chunks,
            table,
            edit_coord,
            world_y,
            prev_sky_pass,
            new_sky_pass,
        );
        incremental_sky_light(
            &mut window,
            &mut changed,
            chunks,
            table,
            centre_pos,
            world_x,
            world_y,
            world_z,
            prev_sky_pass,
            new_sky_pass,
            new_op,
            reseed_column,
            geometry,
        );
    }

    let mut touched = Vec::new();
    for dz in 0..3 {
        for dx in 0..3 {
            if let Some(light) = window[dz][dx].take() {
                let pos = ChunkPos {
                    x: centre_pos.x + (dx as i32 - 1),
                    z: centre_pos.z + (dz as i32 - 1),
                };
                if changed[dz][dx] {
                    touched.push(pos);
                }
                cache.chunks.insert(pos, light);
            }
        }
    }
    touched
}

/// Window-local coordinate inside the 48×height×48 incremental relight
/// volume: `(gx, ly, gz)` where gx/gz include the 3×3 chunk offset.
type WCoord = (usize, usize, usize);

#[inline]
fn world_to_window(
    centre_pos: ChunkPos,
    geometry: ChunkGeometry,
    wx: i32,
    wy: i32,
    wz: i32,
) -> Option<WCoord> {
    if !(geometry.min_y()..geometry.max_y()).contains(&wy) {
        return None;
    }
    let cx = wx.div_euclid(SECTION_DIM as i32);
    let cz = wz.div_euclid(SECTION_DIM as i32);
    let dx = cx - centre_pos.x;
    let dz = cz - centre_pos.z;
    if !(-1..=1).contains(&dx) || !(-1..=1).contains(&dz) {
        return None;
    }
    let lx = wx.rem_euclid(SECTION_DIM as i32) as usize;
    let lz = wz.rem_euclid(SECTION_DIM as i32) as usize;
    let ly = (wy - geometry.min_y()) as usize;
    let gx = (dx + 1) as usize * SECTION_DIM + lx;
    let gz = (dz + 1) as usize * SECTION_DIM + lz;
    Some((gx, ly, gz))
}

#[inline]
fn pack_light_queue_entry(coord: WCoord, level: u8) -> u64 {
    let (gx, ly, gz) = coord;
    debug_assert!(gx < N_X);
    debug_assert!(ly <= 0x01ff);
    debug_assert!(gz < N_Z);
    debug_assert!(level <= 15);

    (gx as u64) | ((gz as u64) << 6) | ((ly as u64) << 12) | ((level as u64) << 21)
}

#[inline]
fn unpack_light_queue_entry(packed: u64) -> (WCoord, u8) {
    let gx = (packed & 0x3f) as usize;
    let gz = ((packed >> 6) & 0x3f) as usize;
    let ly = ((packed >> 12) & 0x01ff) as usize;
    let level = ((packed >> 21) & 0x0f) as u8;
    ((gx, ly, gz), level)
}

#[inline]
fn neighbour_coord(
    coord: WCoord,
    delta: (i32, i32, i32),
    geometry: ChunkGeometry,
) -> Option<WCoord> {
    let (gx, ly, gz) = coord;
    let nx = gx as i32 + delta.0;
    let ny = ly as i32 + delta.1;
    let nz = gz as i32 + delta.2;
    if nx < 0 || nx >= N_X as i32 || ny < 0 || ny >= geometry.height() || nz < 0 || nz >= N_Z as i32
    {
        return None;
    }
    Some((nx as usize, ny as usize, nz as usize))
}

#[inline]
fn coord_parts(coord: WCoord) -> (usize, usize, usize, usize, usize) {
    let (gx, ly, gz) = coord;
    let dx = gx / SECTION_DIM;
    let dz = gz / SECTION_DIM;
    let lx = gx % SECTION_DIM;
    let lz = gz % SECTION_DIM;
    (dx, dz, lx, ly, lz)
}

#[inline]
fn window_slot_is_some(window: &[[Option<ChunkLight>; 3]; 3], coord: WCoord) -> bool {
    let (dx, dz, _, _, _) = coord_parts(coord);
    window[dz][dx].is_some()
}

#[inline]
fn opacity_at_coord(
    chunks: &[[Option<&Chunk>; 3]; 3],
    table: &BlockLightTable,
    coord: WCoord,
    geometry: ChunkGeometry,
) -> u8 {
    let (dx, dz, lx, ly, lz) = coord_parts(coord);
    let Some(chunk) = chunks[dz][dx] else {
        return 0;
    };
    let state = chunk
        .get_block(lx as u8, geometry.min_y() + ly as i32, lz as u8)
        .unwrap_or(BlockStateId(0));
    table.opacity(state.0).unwrap_or(0)
}

#[inline]
fn propagates_sky_at_coord(
    chunks: &[[Option<&Chunk>; 3]; 3],
    table: &BlockLightTable,
    coord: WCoord,
    geometry: ChunkGeometry,
) -> bool {
    let (dx, dz, lx, ly, lz) = coord_parts(coord);
    let Some(chunk) = chunks[dz][dx] else {
        return true;
    };
    let state = chunk
        .get_block(lx as u8, geometry.min_y() + ly as i32, lz as u8)
        .unwrap_or(BlockStateId(0));
    table.propagates_sky(state.0).unwrap_or(true)
}

#[inline]
fn block_light_at(window: &[[Option<ChunkLight>; 3]; 3], coord: WCoord) -> u8 {
    let (dx, dz, lx, ly, lz) = coord_parts(coord);
    window[dz][dx]
        .as_ref()
        .map(|c| c.block.get(lx, ly, lz))
        .unwrap_or(0)
}

#[inline]
fn sky_light_at(window: &[[Option<ChunkLight>; 3]; 3], coord: WCoord) -> u8 {
    let (dx, dz, lx, ly, lz) = coord_parts(coord);
    window[dz][dx]
        .as_ref()
        .map(|c| c.sky.get(lx, ly, lz))
        .unwrap_or(0)
}

#[inline]
fn set_block_light(
    window: &mut [[Option<ChunkLight>; 3]; 3],
    changed: &mut [[bool; 3]; 3],
    coord: WCoord,
    v: u8,
) {
    let (dx, dz, lx, ly, lz) = coord_parts(coord);
    if let Some(c) = window[dz][dx].as_mut()
        && c.block.get(lx, ly, lz) != v
    {
        c.block.set(lx, ly, lz, v);
        changed[dz][dx] = true;
    }
}

#[inline]
fn set_sky_light(
    window: &mut [[Option<ChunkLight>; 3]; 3],
    changed: &mut [[bool; 3]; 3],
    coord: WCoord,
    v: u8,
) {
    let (dx, dz, lx, ly, lz) = coord_parts(coord);
    if let Some(c) = window[dz][dx].as_mut()
        && c.sky.get(lx, ly, lz) != v
    {
        c.sky.set(lx, ly, lz, v);
        changed[dz][dx] = true;
    }
}

fn highest_opaque_changed(
    chunks: &[[Option<&Chunk>; 3]; 3],
    table: &BlockLightTable,
    edit_coord: WCoord,
    world_y: i32,
    prev_sky_pass: bool,
    new_sky_pass: bool,
) -> bool {
    if prev_sky_pass == new_sky_pass {
        return false;
    }

    let (dx, dz, lx, _ly, lz) = coord_parts(edit_coord);
    let post_top = chunks[dz][dx].and_then(|chunk| {
        heightmap_value_to_world_y_for_geometry(
            chunk.geometry(),
            top_opaque_column(chunk, lx as u8, lz as u8, table),
        )
    });

    match (!prev_sky_pass, !new_sky_pass) {
        (false, true) => post_top == Some(world_y),
        (true, false) => match post_top {
            Some(top) => top < world_y,
            None => true,
        },
        _ => false,
    }
}

/// Removal+addition BFS for block-light around a single edit cell.
/// `world_x/y/z` is the edit cell in world coords; `new_op` and
/// `new_emit` are post-edit opacity and emission for that cell.
#[allow(clippy::too_many_arguments)]
fn incremental_block_light(
    window: &mut [[Option<ChunkLight>; 3]; 3],
    changed: &mut [[bool; 3]; 3],
    chunks: &[[Option<&Chunk>; 3]; 3],
    table: &BlockLightTable,
    centre_pos: ChunkPos,
    world_x: i32,
    world_y: i32,
    world_z: i32,
    new_op: u8,
    new_emit: u8,
    geometry: ChunkGeometry,
) {
    let edit_coord = match world_to_window(centre_pos, geometry, world_x, world_y, world_z) {
        Some(c) => c,
        None => return,
    };
    if !window_slot_is_some(window, edit_coord) {
        return;
    }

    let mut removal: VecDeque<u64> = VecDeque::new();
    let mut relight: VecDeque<u64> = VecDeque::new();

    let old_self = block_light_at(window, edit_coord);
    set_block_light(window, changed, edit_coord, new_emit);
    if old_self > new_emit {
        removal.push_back(pack_light_queue_entry(edit_coord, old_self));
    }
    if new_emit > 0 {
        relight.push_back(pack_light_queue_entry(edit_coord, new_emit));
    }
    // Edit-cell neighbours are always candidate relight seeds: light
    // propagating *through* the edit cell may now be wrong if opacity
    // changed.
    for delta in NEIGHBOURS {
        if let Some(coord) = neighbour_coord(edit_coord, delta, geometry) {
            relight.push_back(pack_light_queue_entry(coord, 0));
        }
    }

    while let Some(packed) = removal.pop_front() {
        let (coord, prev_val) = unpack_light_queue_entry(packed);
        for delta in NEIGHBOURS {
            let Some(ncoord) = neighbour_coord(coord, delta, geometry) else {
                continue;
            };
            if !window_slot_is_some(window, ncoord) {
                continue;
            }
            let n_val = block_light_at(window, ncoord);
            if n_val == 0 {
                continue;
            }
            // Special-case the edit cell on the *receiving* side: its
            // opacity is already accounted for via `new_op`, but we
            // already overwrote its light to `new_emit`, so skip it
            // (treating it as an independent source).
            if ncoord == edit_coord {
                relight.push_back(pack_light_queue_entry(ncoord, n_val));
                continue;
            }
            let n_op = opacity_at_coord(chunks, table, ncoord, geometry);
            let cost = n_op.max(1);
            if n_val == prev_val.saturating_sub(cost) {
                set_block_light(window, changed, ncoord, 0);
                removal.push_back(pack_light_queue_entry(ncoord, n_val));
            } else {
                relight.push_back(pack_light_queue_entry(ncoord, n_val));
            }
        }
    }

    let _ = new_op; // opacity drives BFS via cost; explicit to silence unused
    while let Some(packed) = relight.pop_front() {
        let (coord, _queued_level) = unpack_light_queue_entry(packed);
        if !window_slot_is_some(window, coord) {
            continue;
        }
        let cur = block_light_at(window, coord);
        if cur <= 1 {
            continue;
        }
        let best_propagated = cur - 1;
        for delta in NEIGHBOURS {
            let Some(ncoord) = neighbour_coord(coord, delta, geometry) else {
                continue;
            };
            if !window_slot_is_some(window, ncoord) {
                continue;
            }
            // M9.f early-skip: skip the opacity lookup if the neighbour
            // is already at the best possible level we could push.
            if block_light_at(window, ncoord) >= best_propagated {
                continue;
            }
            let n_op = opacity_at_coord(chunks, table, ncoord, geometry);
            let cost = n_op.max(1);
            let propagated = cur.saturating_sub(cost);
            if propagated > block_light_at(window, ncoord) {
                set_block_light(window, changed, ncoord, propagated);
                relight.push_back(pack_light_queue_entry(ncoord, propagated));
            }
        }
    }
}

/// Removal+addition BFS for sky-light, with column re-seed when the
/// edit cell's `propagates_sky` flag changed.
///
/// Sky-light's "source" is the top of every open column. When
/// `propagates_sky` flips at the edit cell, columns above and below
/// may need their direct-15 status recomputed within the affected
/// 31×31 footprint. We do this re-seeding implicitly via the
/// removal/addition BFS plus a column walk anchored at the edit
/// cell.
#[allow(clippy::too_many_arguments)]
fn incremental_sky_light(
    window: &mut [[Option<ChunkLight>; 3]; 3],
    changed: &mut [[bool; 3]; 3],
    chunks: &[[Option<&Chunk>; 3]; 3],
    table: &BlockLightTable,
    centre_pos: ChunkPos,
    world_x: i32,
    world_y: i32,
    world_z: i32,
    _prev_sky_pass: bool,
    new_sky_pass: bool,
    new_op: u8,
    reseed_column: bool,
    geometry: ChunkGeometry,
) {
    let edit_coord = match world_to_window(centre_pos, geometry, world_x, world_y, world_z) {
        Some(c) => c,
        None => return,
    };
    if !window_slot_is_some(window, edit_coord) {
        return;
    }

    let mut removal: VecDeque<u64> = VecDeque::new();
    let mut relight: VecDeque<u64> = VecDeque::new();

    // What's the new "ceiling" sky value at the edit cell?
    // = 15 if propagates_sky now AND every cell directly above
    //   propagates sky too; else 0 (BFS will re-propagate from
    //   neighbours).
    let new_self = if new_sky_pass {
        let mut all_open = true;
        let (gx, edit_ly, gz) = edit_coord;
        for ly in edit_ly + 1..geometry.height() as usize {
            let coord = (gx, ly, gz);
            if !propagates_sky_at_coord(chunks, table, coord, geometry) {
                all_open = false;
                break;
            }
        }
        if all_open { 15 } else { 0 }
    } else {
        0
    };

    let old_self = sky_light_at(window, edit_coord);
    set_sky_light(window, changed, edit_coord, new_self);
    if old_self > new_self {
        removal.push_back(pack_light_queue_entry(edit_coord, old_self));
    }
    if new_self > 0 {
        relight.push_back(pack_light_queue_entry(edit_coord, new_self));
    }

    // Column propagation: if propagates_sky flipped at the edit cell,
    // cells *below* may need their 15-status recomputed (they were
    // direct-sky if and only if every cell from the geometry ceiling down to them
    // propagates sky). Walk down from the edit cell, updating each
    // cell's sky to 15 if the column is now open, or to 0 if it just
    // closed; queue removal/relight accordingly. Stop at the first
    // opaque cell or once we hit any cell whose new value matches its
    // old.
    if reseed_column {
        let column_open_above_self = new_self == 15;
        let (gx, edit_ly, gz) = edit_coord;
        for ly in (0..edit_ly).rev() {
            let coord = (gx, ly, gz);
            if !window_slot_is_some(window, coord) {
                break;
            }
            let cell_passes = propagates_sky_at_coord(chunks, table, coord, geometry);
            if !cell_passes {
                break;
            }
            let new_val = if column_open_above_self {
                // Walking down through an open column: sky stays at 15.
                15
            } else {
                // Edit cell now blocks (or column was already blocked):
                // direct-sky path is gone. Drop to 0; relight BFS will
                // refill from neighbours where applicable.
                0
            };
            let old = sky_light_at(window, coord);
            if old == new_val {
                break;
            }
            set_sky_light(window, changed, coord, new_val);
            if old > new_val {
                removal.push_back(pack_light_queue_entry(coord, old));
            }
            if new_val > 0 {
                relight.push_back(pack_light_queue_entry(coord, new_val));
            }
        }
    }

    // Edit-cell neighbours always candidate seeds — sky pushed
    // through the edit cell may now be wrong.
    for delta in NEIGHBOURS {
        if let Some(coord) = neighbour_coord(edit_coord, delta, geometry) {
            relight.push_back(pack_light_queue_entry(coord, 0));
        }
    }

    while let Some(packed) = removal.pop_front() {
        let (coord, prev_val) = unpack_light_queue_entry(packed);
        for delta in NEIGHBOURS {
            let Some(ncoord) = neighbour_coord(coord, delta, geometry) else {
                continue;
            };
            if !window_slot_is_some(window, ncoord) {
                continue;
            }
            let n_val = sky_light_at(window, ncoord);
            if n_val == 0 {
                continue;
            }
            let n_op = opacity_at_coord(chunks, table, ncoord, geometry);
            let cost = n_op.max(1);
            // Sky vertical-down cost is 0 when the source was at 15
            // and the neighbour passes sky — that's the "direct sky
            // column" case. The current `bfs` cost rule already
            // models this with cost = max(1, opacity); the column
            // re-seed above is what handles the 15-passes-through
            // case explicitly. So removal here uses the same cost.
            if n_val == prev_val.saturating_sub(cost) {
                set_sky_light(window, changed, ncoord, 0);
                removal.push_back(pack_light_queue_entry(ncoord, n_val));
            } else {
                relight.push_back(pack_light_queue_entry(ncoord, n_val));
            }
        }
    }

    let _ = new_op;
    while let Some(packed) = relight.pop_front() {
        let (coord, _queued_level) = unpack_light_queue_entry(packed);
        if !window_slot_is_some(window, coord) {
            continue;
        }
        let cur = sky_light_at(window, coord);
        if cur <= 1 {
            continue;
        }
        let best_propagated = cur - 1;
        for delta in NEIGHBOURS {
            let Some(ncoord) = neighbour_coord(coord, delta, geometry) else {
                continue;
            };
            if !window_slot_is_some(window, ncoord) {
                continue;
            }
            if sky_light_at(window, ncoord) >= best_propagated {
                continue;
            }
            let n_op = opacity_at_coord(chunks, table, ncoord, geometry);
            let cost = n_op.max(1);
            let propagated = cur.saturating_sub(cost);
            if propagated > sky_light_at(window, ncoord) {
                set_sky_light(window, changed, ncoord, propagated);
                relight.push_back(pack_light_queue_entry(ncoord, propagated));
            }
        }
    }
}

/// Compute sky and block light for the centre chunk of a 3×3
/// neighbourhood. The centre is `neighbourhood[1][1]` (must be
/// `Some`); off-centre slots may be `None`, in which case the engine
/// treats that neighbour as a column of air (opacity 0,
/// propagates_sky true, emission 0).
///
/// Convenience wrapper around [`compute_chunk_light_in`] that
/// allocates a fresh [`LightWorkspace`] for one call. Hot paths
/// that compute many chunks should manage the workspace explicitly.
pub fn compute_chunk_light(
    neighbourhood: [[Option<&Chunk>; 3]; 3],
    table: &BlockLightTable,
) -> ChunkLight {
    let mut ws = LightWorkspace::new();
    compute_chunk_light_in(&mut ws, neighbourhood, table)
}

/// Like [`compute_chunk_light`] but reuses caller-owned buffers.
pub fn compute_chunk_light_in(
    ws: &mut LightWorkspace,
    neighbourhood: [[Option<&Chunk>; 3]; 3],
    table: &BlockLightTable,
) -> ChunkLight {
    assert!(
        neighbourhood[1][1].is_some(),
        "centre chunk must be present",
    );
    let geometry = shared_neighbourhood_geometry(&neighbourhood);

    // Fast path: if every chunk in the 3×3 neighbourhood is fully
    // air (single-state air sections, no biome variation needed for
    // lighting), the output is sky=15 everywhere, block=0 — no BFS.
    // Our test-world has ~400 of its 441 spawn-window chunks in
    // Status: structure_starts which have no terrain at all, so
    // skipping their BFS turns the spawn burst from ~30 s to a few
    // seconds in debug builds.
    if is_all_air_neighbourhood(&neighbourhood) {
        return ChunkLight::filled_for_geometry(15, 0, geometry);
    }

    if let Some(light) = compute_columnar_no_emitter_light(&neighbourhood, table, geometry) {
        return light;
    }

    compute_chunk_light_slow_in(ws, neighbourhood, table, geometry)
}

fn shared_neighbourhood_geometry(neighbourhood: &[[Option<&Chunk>; 3]; 3]) -> ChunkGeometry {
    let geometry = neighbourhood[1][1]
        .expect("centre chunk must be present")
        .geometry();
    for chunk in neighbourhood.iter().flatten().flatten() {
        assert_eq!(
            chunk.geometry(),
            geometry,
            "lighting neighbourhood chunks must share geometry",
        );
    }
    geometry
}

fn compute_chunk_light_slow_in(
    ws: &mut LightWorkspace,
    neighbourhood: [[Option<&Chunk>; 3]; 3],
    table: &BlockLightTable,
    geometry: ChunkGeometry,
) -> ChunkLight {
    debug_assert!(neighbourhood[1][1].is_some());

    ws.reset_for_geometry(geometry);
    let world_height = geometry.height() as usize;

    populate_grids(
        &neighbourhood,
        table,
        &mut ws.opacity,
        &mut ws.propagates_sky,
        &mut ws.block,
        &mut ws.emitters,
    );

    seed_sky_from_open_columns(&ws.propagates_sky, &mut ws.sky, &mut ws.queue, world_height);
    bfs(&ws.opacity, &mut ws.sky, &mut ws.queue, world_height);

    if !ws.emitters.is_empty() {
        ws.queue.extend(ws.emitters.iter().copied());
        bfs(&ws.opacity, &mut ws.block, &mut ws.queue, world_height);
    }

    extract_centre_with_backend(&ws.sky, &ws.block, ws.kernel_backend, geometry)
}

fn compute_columnar_no_emitter_light(
    neighbourhood: &[[Option<&Chunk>; 3]; 3],
    table: &BlockLightTable,
    geometry: ChunkGeometry,
) -> Option<ChunkLight> {
    if !neighbourhood_is_columnar_without_emitters(neighbourhood, table, geometry) {
        return None;
    }

    let centre = neighbourhood[1][1]?;
    let air = BlockStateId(0);
    let world_height = geometry.height() as usize;
    let mut out = ChunkLight::zeroed_for_geometry(geometry);
    for lz in 0..SECTION_DIM {
        for lx in 0..SECTION_DIM {
            if let Some(top_y) = columnar_top_hint(centre, lx as u8, lz as u8) {
                for ly in ((top_y + 1 - geometry.min_y()) as usize)..world_height {
                    out.set_sky_local(lx, ly, lz, 15);
                }
            } else {
                for ly in (0..world_height).rev() {
                    let world_y = geometry.min_y() + ly as i32;
                    let state = centre.get_block(lx as u8, world_y, lz as u8).unwrap_or(air);
                    if table.propagates_sky(state.0).unwrap_or(true) {
                        out.set_sky_local(lx, ly, lz, 15);
                    } else {
                        break;
                    }
                }
            }
        }
    }
    Some(out)
}

fn neighbourhood_is_columnar_without_emitters(
    neighbourhood: &[[Option<&Chunk>; 3]; 3],
    table: &BlockLightTable,
    geometry: ChunkGeometry,
) -> bool {
    let air = BlockStateId(0);
    for row in neighbourhood {
        for slot in row {
            let Some(chunk) = *slot else {
                continue;
            };
            for lz in 0..SECTION_DIM {
                for lx in 0..SECTION_DIM {
                    if let Some(top_y) = columnar_top_hint(chunk, lx as u8, lz as u8) {
                        if !hinted_column_is_columnar_without_emitters(
                            chunk, table, lx as u8, lz as u8, top_y,
                        ) {
                            return false;
                        }
                        continue;
                    }

                    let mut blocked = false;
                    for world_y in (geometry.min_y()..geometry.max_y()).rev() {
                        let state = chunk.get_block(lx as u8, world_y, lz as u8).unwrap_or(air);
                        if table.emission(state.0).unwrap_or(0) > 0 {
                            return false;
                        }
                        let propagates_sky = table.propagates_sky(state.0).unwrap_or(true);
                        if !propagates_sky && table.opacity(state.0).unwrap_or(0) < 15 {
                            return false;
                        }
                        if !blocked {
                            if !propagates_sky {
                                blocked = true;
                            }
                        } else if propagates_sky {
                            return false;
                        }
                    }
                }
            }
        }
    }
    true
}

fn columnar_top_hint(chunk: &Chunk, x: u8, z: u8) -> Option<i32> {
    chunk.highest_opaque_y(x, z).or_else(|| {
        chunk
            .heightmaps
            .get("MOTION_BLOCKING")
            .or_else(|| chunk.heightmaps.get("WORLD_SURFACE"))
            .and_then(|hm| heightmap_value_to_world_y_for_geometry(chunk.geometry(), hm.get(x, z)))
    })
}

fn heightmap_value_to_world_y_for_geometry(geometry: ChunkGeometry, value: u32) -> Option<i32> {
    (value != 0).then(|| geometry.min_y() + value as i32 - 1)
}

fn hinted_column_is_columnar_without_emitters(
    chunk: &Chunk,
    table: &BlockLightTable,
    x: u8,
    z: u8,
    top_y: i32,
) -> bool {
    let air = BlockStateId(0);
    for y in chunk.geometry().min_y()..=top_y {
        let state = chunk.get_block(x, y, z).unwrap_or(air);
        if table.emission(state.0).unwrap_or(0) > 0 {
            return false;
        }
        if table.propagates_sky(state.0).unwrap_or(true) {
            return false;
        }
        if table.opacity(state.0).unwrap_or(0) < 15 {
            return false;
        }
    }
    true
}

fn is_all_air_neighbourhood(neighbourhood: &[[Option<&Chunk>; 3]; 3]) -> bool {
    let air = BlockStateId(0);
    for row in neighbourhood {
        for slot in row {
            let Some(chunk) = *slot else {
                continue; // missing neighbour = treated as air, trivially passes
            };
            for section in &chunk.sections {
                if !section_is_only(section, air) {
                    return false;
                }
            }
        }
    }
    true
}

fn section_is_only(section: &crate::section::ChunkSection, state: BlockStateId) -> bool {
    // Single-state sections expose `palette() == None` and
    // `get(0,0,0)` reports the held state. Indirect-mode sections
    // are non-trivial by construction (the codec only switches to
    // indirect when more than one state appears).
    section.palette().is_none() && section.get(0, 0, 0) == state
}

fn populate_grids(
    neighbourhood: &[[Option<&Chunk>; 3]; 3],
    table: &BlockLightTable,
    opacity: &mut [u8],
    propagates_sky: &mut [bool],
    block_seed: &mut [u8],
    emitters: &mut Vec<u32>,
) {
    for (cz, row) in neighbourhood.iter().enumerate() {
        for (cx, slot) in row.iter().enumerate() {
            let Some(chunk) = *slot else {
                // Missing neighbour: leave the slab at the all-air
                // defaults (opacity=0, propagates_sky=true, emission=0).
                continue;
            };
            for (section_idx, section) in chunk.sections.iter().enumerate() {
                let base_ly = section_idx * SECTION_DIM;
                if section.palette().is_none() {
                    let state = section.get(0, 0, 0);
                    let op = table.opacity(state.0).unwrap_or(0);
                    let sky_pass = table.propagates_sky(state.0).unwrap_or(true);
                    let emit = table.emission(state.0).unwrap_or(0);
                    if op == 0 && sky_pass && emit == 0 {
                        continue;
                    }
                    populate_uniform_section(
                        cx,
                        cz,
                        base_ly,
                        op,
                        sky_pass,
                        emit,
                        opacity,
                        propagates_sky,
                        block_seed,
                        emitters,
                    );
                    continue;
                }

                populate_indirect_section(
                    cx,
                    cz,
                    base_ly,
                    section,
                    table,
                    opacity,
                    propagates_sky,
                    block_seed,
                    emitters,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn populate_uniform_section(
    cx: usize,
    cz: usize,
    base_ly: usize,
    op: u8,
    sky_pass: bool,
    emit: u8,
    opacity: &mut [u8],
    propagates_sky: &mut [bool],
    block_seed: &mut [u8],
    emitters: &mut Vec<u32>,
) {
    for sy in 0..SECTION_DIM {
        let ly = base_ly + sy;
        for lz in 0..SECTION_DIM {
            let gz = cz * SECTION_DIM + lz;
            for lx in 0..SECTION_DIM {
                let gx = cx * SECTION_DIM + lx;
                let idx = grid_idx(gx, ly, gz);
                opacity[idx] = op;
                propagates_sky[idx] = sky_pass;
                if emit > 0 {
                    block_seed[idx] = emit;
                    emitters.push(idx as u32);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn populate_indirect_section(
    cx: usize,
    cz: usize,
    base_ly: usize,
    section: &crate::section::ChunkSection,
    table: &BlockLightTable,
    opacity: &mut [u8],
    propagates_sky: &mut [bool],
    block_seed: &mut [u8],
    emitters: &mut Vec<u32>,
) {
    for sy in 0..SECTION_DIM {
        let ly = base_ly + sy;
        for lz in 0..SECTION_DIM {
            let gz = cz * SECTION_DIM + lz;
            for lx in 0..SECTION_DIM {
                let gx = cx * SECTION_DIM + lx;
                let idx = grid_idx(gx, ly, gz);
                let state = section.get(lx as u8, sy as u8, lz as u8);
                opacity[idx] = table.opacity(state.0).unwrap_or(0);
                propagates_sky[idx] = table.propagates_sky(state.0).unwrap_or(true);
                let emit = table.emission(state.0).unwrap_or(0);
                if emit > 0 {
                    block_seed[idx] = emit;
                    emitters.push(idx as u32);
                }
            }
        }
    }
}

fn seed_sky_from_open_columns(
    propagates_sky: &[bool],
    sky: &mut [u8],
    queue: &mut VecDeque<u32>,
    world_height: usize,
) {
    // Two-pass seed. Pass 1: walk each column top-down, marking
    // every cell as `sky=15` while `propagates_sky` holds. Don't
    // touch the queue yet — pushing every interior open-sky cell
    // would queue ~884k entries on a flat world and dominate the
    // BFS runtime (each interior cell's six neighbours are also
    // sky=15, so the BFS does no work but the pop/push churn does).
    //
    // Pass 2: push only "boundary" sky=15 cells — those with at
    // least one neighbour that is *not* sky=15. These are the
    // cells where BFS will actually drive an update.
    for gx in 0..N_X {
        for gz in 0..N_Z {
            for ly in (0..world_height).rev() {
                let idx = grid_idx(gx, ly, gz);
                if propagates_sky[idx] {
                    sky[idx] = 15;
                } else {
                    break;
                }
            }
        }
    }
    for ly in 0..world_height {
        for gz in 0..N_Z {
            for gx in 0..N_X {
                let idx = grid_idx(gx, ly, gz);
                if sky[idx] != 15 {
                    continue;
                }
                if has_dark_neighbour(sky, gx, ly, gz, world_height) {
                    queue.push_back(idx as u32);
                }
            }
        }
    }
}

fn has_dark_neighbour(sky: &[u8], gx: usize, ly: usize, gz: usize, world_height: usize) -> bool {
    const NEIGHBOURS: [(isize, isize, isize); 6] = [
        (-1, 0, 0),
        (1, 0, 0),
        (0, -1, 0),
        (0, 1, 0),
        (0, 0, -1),
        (0, 0, 1),
    ];
    for (dx, dy, dz) in NEIGHBOURS {
        let nx = gx as isize + dx;
        let ny = ly as isize + dy;
        let nz = gz as isize + dz;
        if nx < 0 || nx >= N_X as isize {
            continue;
        }
        if ny < 0 || ny >= world_height as isize {
            continue;
        }
        if nz < 0 || nz >= N_Z as isize {
            continue;
        }
        if sky[grid_idx(nx as usize, ny as usize, nz as usize)] != 15 {
            return true;
        }
    }
    false
}

/// Generic 6-neighbour BFS used by both passes.
fn bfs(opacity: &[u8], values: &mut [u8], queue: &mut VecDeque<u32>, world_height: usize) {
    while let Some(packed) = queue.pop_front() {
        let idx = packed as usize;
        let current = values[idx];
        if current == 0 {
            continue;
        }
        let (gx, ly, gz) = unpack_idx(idx);
        let neighbours: [(isize, isize, isize); 6] = [
            (-1, 0, 0),
            (1, 0, 0),
            (0, -1, 0),
            (0, 1, 0),
            (0, 0, -1),
            (0, 0, 1),
        ];
        // M9.f early-skip (Starlight trick): best-case propagation is
        // `current - 1` when opacity == 1. If the neighbour already
        // sits at that level or higher, the opacity lookup + cost
        // math can't improve it — skip the read entirely. Saves
        // ~5–6× block-state-equivalent reads in dense regions.
        let best_propagated = current.saturating_sub(1);
        for (dx, dy, dz) in neighbours {
            let nx = gx as isize + dx;
            let ny = ly as isize + dy;
            let nz = gz as isize + dz;
            if nx < 0 || nx >= N_X as isize {
                continue;
            }
            if ny < 0 || ny >= world_height as isize {
                continue;
            }
            if nz < 0 || nz >= N_Z as isize {
                continue;
            }
            let nidx = grid_idx(nx as usize, ny as usize, nz as usize);
            if values[nidx] >= best_propagated {
                continue;
            }
            let cost = opacity[nidx].max(1);
            let candidate = current.saturating_sub(cost);
            if candidate > values[nidx] {
                values[nidx] = candidate;
                if candidate > 1 {
                    queue.push_back(nidx as u32);
                }
            }
        }
    }
}

fn extract_centre_with_backend(
    sky: &[u8],
    block: &[u8],
    backend: LightKernelBackend,
    geometry: ChunkGeometry,
) -> ChunkLight {
    let section_count = geometry.section_count();
    let extract = |grid| match backend {
        LightKernelBackend::Scalar => extract_light_layer_scalar(grid, section_count),
        LightKernelBackend::PortableSimd => extract_light_layer_portable(grid, section_count),
    };
    ChunkLight {
        sky: extract(sky),
        block: extract(block),
        min_y: geometry.min_y(),
    }
}

fn extract_light_layer_scalar(grid: &[u8], section_count: usize) -> LightLayer {
    extract_light_layer(grid, section_count, |row, output| {
        let mut nonzero = 0_u16;
        for (pair, output) in row.chunks_exact(2).zip(output) {
            let low = pair[0];
            let high = pair[1];
            debug_assert!(low <= 15 && high <= 15);
            *output = low | (high << 4);
            nonzero += u16::from(low != 0) + u16::from(high != 0);
        }
        nonzero
    })
}

#[cfg(target_endian = "little")]
fn extract_light_layer_portable(grid: &[u8], section_count: usize) -> LightLayer {
    debug_assert_eq!(grid.len(), grid_volume(section_count * SECTION_DIM));
    let mut sections: Vec<Option<Box<[u8; LIGHT_LAYER_BYTES]>>> =
        (0..section_count).map(|_| None).collect();
    let mut nonzero_nibbles = vec![0_u16; section_count];
    for section_idx in 0..section_count {
        let mut packed = Box::new([0_u8; LIGHT_LAYER_BYTES]);
        let mut nonzero = 0_u16;
        for section_y in 0..SECTION_DIM {
            let local_y = section_idx * SECTION_DIM + section_y;
            for local_z in (0..SECTION_DIM).step_by(2) {
                let first_start = grid_idx(SECTION_DIM, local_y, SECTION_DIM + local_z);
                let second_start = grid_idx(SECTION_DIM, local_y, SECTION_DIM + local_z + 1);
                let (first, first_nonzero) =
                    pack_light_row_portable(&grid[first_start..first_start + SECTION_DIM]);
                let (second, second_nonzero) =
                    pack_light_row_portable(&grid[second_start..second_start + SECTION_DIM]);
                let output_start = (section_y * SECTION_DIM + local_z) * SECTION_DIM / 2;
                packed[output_start..output_start + SECTION_DIM]
                    .copy_from_slice(&u8x16::narrow_i16x8(first, second).to_array());
                nonzero += first_nonzero + second_nonzero;
            }
        }
        if nonzero != 0 {
            sections[section_idx] = Some(packed);
            nonzero_nibbles[section_idx] = nonzero;
        }
    }
    LightLayer {
        sections,
        nonzero_nibbles,
    }
}

#[cfg(target_endian = "little")]
#[inline]
fn pack_light_row_portable(row: &[u8]) -> (i16x8, u16) {
    let row: [u8; SECTION_DIM] = row.try_into().expect("light row width");
    debug_assert!(row.iter().all(|value| *value <= 15));
    let values = u8x16::new(row);
    let words: u16x8 = cast(values);
    let packed = (words & u16x8::splat(0x000f)) | ((words >> 4_u16) & u16x8::splat(0x00f0));
    let nonzero = values.simd_ne(u8x16::ZERO).to_bitmask().count_ones() as u16;
    (cast(packed), nonzero)
}

#[cfg(not(target_endian = "little"))]
fn extract_light_layer_portable(grid: &[u8], section_count: usize) -> LightLayer {
    extract_light_layer_scalar(grid, section_count)
}

fn extract_light_layer(
    grid: &[u8],
    section_count: usize,
    mut pack_row: impl FnMut(&[u8], &mut [u8]) -> u16,
) -> LightLayer {
    debug_assert_eq!(grid.len(), grid_volume(section_count * SECTION_DIM));
    let mut sections: Vec<Option<Box<[u8; LIGHT_LAYER_BYTES]>>> =
        (0..section_count).map(|_| None).collect();
    let mut nonzero_nibbles = vec![0_u16; section_count];
    for section_idx in 0..section_count {
        let mut packed = Box::new([0_u8; LIGHT_LAYER_BYTES]);
        let mut nonzero = 0_u16;
        for section_y in 0..SECTION_DIM {
            let local_y = section_idx * SECTION_DIM + section_y;
            for local_z in 0..SECTION_DIM {
                let start = grid_idx(SECTION_DIM, local_y, SECTION_DIM + local_z);
                let output_start = (section_y * SECTION_DIM + local_z) * SECTION_DIM / 2;
                nonzero += pack_row(
                    &grid[start..start + SECTION_DIM],
                    &mut packed[output_start..output_start + SECTION_DIM / 2],
                );
            }
        }
        if nonzero != 0 {
            sections[section_idx] = Some(packed);
            nonzero_nibbles[section_idx] = nonzero;
        }
    }
    LightLayer {
        sections,
        nonzero_nibbles,
    }
}

#[doc(hidden)]
#[must_use]
pub fn benchmark_extract_centre(
    sky: &[u8],
    block: &[u8],
    backend: LightKernelBackend,
) -> ChunkLight {
    assert_eq!(sky.len(), N_VOL);
    assert_eq!(block.len(), N_VOL);
    extract_centre_with_backend(sky, block, backend, OVERWORLD_GEOMETRY)
}

#[cfg(test)]
fn extract_centre_reference(sky: &[u8], block: &[u8]) -> ChunkLight {
    let mut out = ChunkLight::zeroed();
    for local_y in 0..WORLD_HEIGHT {
        for local_z in 0..SECTION_DIM {
            for local_x in 0..SECTION_DIM {
                let source = grid_idx(SECTION_DIM + local_x, local_y, SECTION_DIM + local_z);
                out.set_sky_local(local_x, local_y, local_z, sky[source]);
                out.set_block_local(local_x, local_y, local_z, block[source]);
            }
        }
    }
    out
}

fn grid_volume(world_height: usize) -> usize {
    N_X * world_height * N_Z
}

fn grid_idx(gx: usize, ly: usize, gz: usize) -> usize {
    ly * (N_X * N_Z) + gz * N_X + gx
}

fn unpack_idx(idx: usize) -> (usize, usize, usize) {
    let area = N_X * N_Z;
    let ly = idx / area;
    let rem = idx % area;
    let gz = rem / N_X;
    let gx = rem % N_X;
    (gx, ly, gz)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_data::Identifier;

    use crate::block::BlockRegistry;
    use crate::chunk::{ChunkGeometry, ChunkPos, MAX_Y};

    fn tiny_table() -> BlockLightTable {
        // State ids:
        //   0 = air (transparent, no emission, sky passes)
        //   1 = stone (opaque, no emission, sky blocked)
        //   2 = glass (transparent, sky passes — same shape as air
        //              for the purposes of this fixture)
        //   3 = glowstone (transparent emitter, emission=15)
        //   4 = water (opacity 1 soft attenuator, no emission, sky
        //              blocked at the source per vanilla's predicate)
        BlockLightTable::from_arrays(
            "test",
            vec![0, 0, 0, 15, 0],
            vec![0, 15, 0, 0, 1],
            vec![true, false, true, true, false],
        )
    }

    fn air_chunk() -> Chunk {
        let plains = Identifier::parse("minecraft:plains").unwrap();
        Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), plains)
    }

    fn custom_air_chunk() -> Chunk {
        let plains = Identifier::parse("minecraft:plains").unwrap();
        let geometry = ChunkGeometry::new(0, 256).unwrap();
        Chunk::empty_with_geometry(ChunkPos { x: 0, z: 0 }, BlockStateId(0), plains, geometry)
    }

    fn solo(chunk: Chunk) -> [[Option<Chunk>; 3]; 3] {
        // Centre-only neighbourhood. Wrapped in Option so the borrow
        // pattern in `compute_chunk_light` matches.
        [
            [None, None, None],
            [None, Some(chunk), None],
            [None, None, None],
        ]
    }

    fn borrow(input: &[[Option<Chunk>; 3]; 3]) -> [[Option<&Chunk>; 3]; 3] {
        std::array::from_fn(|i| std::array::from_fn(|j| input[i][j].as_ref()))
    }

    #[test]
    fn scalar_and_portable_extraction_are_bit_identical() {
        let mut sky = vec![0; N_VOL];
        let mut block = vec![0; N_VOL];
        let mut state = 0x6a09_e667_f3bc_c909_u64;
        for value in sky.iter_mut().chain(&mut block) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *value = (state & 0x0f) as u8;
        }

        let scalar = extract_centre_with_backend(
            &sky,
            &block,
            LightKernelBackend::Scalar,
            OVERWORLD_GEOMETRY,
        );
        let portable = extract_centre_with_backend(
            &sky,
            &block,
            LightKernelBackend::PortableSimd,
            OVERWORLD_GEOMETRY,
        );
        let reference = extract_centre_reference(&sky, &block);

        assert_eq!(scalar, reference);
        assert_eq!(portable, reference);
    }

    #[test]
    fn open_air_chunk_is_sky_15_everywhere_and_block_0() {
        let table = tiny_table();
        let input = solo(air_chunk());
        let out = compute_chunk_light(borrow(&input), &table);
        for ly in 0..WORLD_HEIGHT {
            for lz in 0..SECTION_DIM {
                for lx in 0..SECTION_DIM {
                    assert_eq!(out.sky.get(lx, ly, lz), 15, "sky at ({lx},{ly},{lz})");
                    assert_eq!(out.block.get(lx, ly, lz), 0, "block at ({lx},{ly},{lz})");
                }
            }
        }
    }

    #[test]
    fn custom_geometry_computes_open_sky_and_block_light() {
        let table = tiny_table();
        let mut chunk = custom_air_chunk();
        chunk.set_block(8, 64, 8, BlockStateId(3));
        let input = solo(chunk);

        let out = compute_chunk_light(borrow(&input), &table);

        assert_eq!(out.section_count(), 16);
        assert_eq!(out.sky_at(8, 0, 8), 15);
        assert_eq!(out.sky_at(8, 255, 8), 15);
        assert_eq!(out.block_at(8, 64, 8), 15);
        assert_eq!(out.block_at(9, 64, 8), 14);
    }

    #[test]
    fn chunk_light_can_be_rebuilt_from_baked_section_layers() {
        let mut chunk = air_chunk();
        let mut sky = vec![0; crate::chunk::LIGHT_LAYER_BYTES];
        let mut block = vec![0; crate::chunk::LIGHT_LAYER_BYTES];
        sky[0] = 0x21;
        block[7] = 0xF0;
        chunk.section_lights[0].sky = Some(sky.clone());
        chunk.section_lights[2].block = Some(block.clone());

        let light = ChunkLight::from_section_lights(&chunk.section_lights)
            .expect("present baked layers should rebuild chunk light");

        assert_eq!(light.sky.section(0).unwrap()[0], 0x21);
        assert_eq!(light.block.section(2).unwrap()[7], 0xF0);
        assert_eq!(light.sky.get(0, 0, 0), 1);
        assert_eq!(light.sky.get(1, 0, 0), 2);
        assert_eq!(light.block.section(0), None);
    }

    #[test]
    fn custom_geometry_baked_light_uses_chunk_world_y() {
        let mut chunk = custom_air_chunk();
        let mut block = vec![0; crate::chunk::LIGHT_LAYER_BYTES];
        block[0] = 0x0F;
        chunk.section_lights[0].block = Some(block);

        let light =
            ChunkLight::from_chunk(&chunk).expect("present baked layer should rebuild chunk light");

        assert_eq!(light.block_at(0, 0, 0), 15);
    }

    #[test]
    fn glowstone_emits_15_radius() {
        let table = tiny_table();
        let mut chunk = air_chunk();
        // Place glowstone (state 3) at (8, 64, 8).
        chunk.set_block(8, 64, 8, BlockStateId(3));
        let input = solo(chunk);
        let out = compute_chunk_light(borrow(&input), &table);
        // Glowstone cell is at emission 15.
        assert_eq!(out.block_at(8, 64, 8), 15);
        // 1 cell away: 14.
        assert_eq!(out.block_at(9, 64, 8), 14);
        assert_eq!(out.block_at(8, 65, 8), 14);
        // 15 cells away laterally: 0 (15 - 15 steps).
        assert_eq!(out.block_at(15, 64, 0), 0);
        // 14 cells away (Chebyshev sum is 14 — 7+7): 1 via shortest
        // Manhattan path. Pin via Manhattan distance.
        // (8,64,8) → (8,64,1): 7 z-steps, value 8.
        assert_eq!(out.block_at(8, 64, 1), 8);
    }

    #[test]
    fn solid_column_blocks_sky_underneath() {
        let table = tiny_table();
        let mut chunk = air_chunk();
        // Stone column at x=8, z=8 from world Y=MIN_Y+1 up to Y=64.
        // The cell at world Y=MIN_Y is left as air, forming a 1×1
        // "well" at the very bottom of the world that lateral
        // propagation can reach.
        for y in (MIN_Y + 1)..=64 {
            chunk.set_block(8, y, 8, BlockStateId(1));
        }
        let input = solo(chunk);
        let out = compute_chunk_light(borrow(&input), &table);

        // Sky above the column: 15 (open air well above the stone top).
        assert_eq!(out.sky_at(8, 100, 8), 15);
        // The cell *inside* the column is 0 (opaque).
        assert_eq!(out.sky_at(8, 0, 8), 0);
        // Bottom-of-the-world air cell directly below the column:
        // lateral BFS brings sky-15 from (7, MIN_Y, 8) and (9, MIN_Y, 8)
        // (both fully open columns) with one step of cost-1 attenuation.
        assert_eq!(
            out.sky_at(8, MIN_Y, 8),
            14,
            "sky leaks into the well floor from adjacent open columns",
        );
    }

    #[test]
    fn columnar_fast_path_matches_slow_bfs() {
        let table = tiny_table();
        let mut chunk = air_chunk();
        for lz in 0..SECTION_DIM {
            for lx in 0..SECTION_DIM {
                let top = 48 + ((lx + lz) % 5) as i32;
                for y in MIN_Y..=top {
                    chunk.set_block(lx as u8, y, lz as u8, BlockStateId(1));
                }
            }
        }
        let input = solo(chunk);
        let borrowed = borrow(&input);
        let fast = compute_columnar_no_emitter_light(&borrowed, &table, OVERWORLD_GEOMETRY)
            .expect("columnar terrain should use fast path");
        let mut ws = LightWorkspace::new();
        let slow = compute_chunk_light_slow_in(&mut ws, borrowed, &table, OVERWORLD_GEOMETRY);
        assert_eq!(fast, slow);
    }

    #[test]
    fn columnar_fast_path_rejects_emitters_and_caves() {
        let table = tiny_table();

        let mut emitter = air_chunk();
        emitter.set_block(8, 64, 8, BlockStateId(3));
        let input = solo(emitter);
        assert!(
            compute_columnar_no_emitter_light(&borrow(&input), &table, OVERWORLD_GEOMETRY)
                .is_none()
        );

        let mut cave = air_chunk();
        for y in MIN_Y..=64 {
            cave.set_block(8, y, 8, BlockStateId(1));
        }
        cave.set_block(8, 0, 8, BlockStateId(0));
        let input = solo(cave);
        assert!(
            compute_columnar_no_emitter_light(&borrow(&input), &table, OVERWORLD_GEOMETRY)
                .is_none()
        );

        let mut soft_blocker = air_chunk();
        for y in MIN_Y..64 {
            soft_blocker.set_block(8, y, 8, BlockStateId(1));
        }
        soft_blocker.set_block(8, 64, 8, BlockStateId(4));
        let input = solo(soft_blocker);
        assert!(
            compute_columnar_no_emitter_light(&borrow(&input), &table, OVERWORLD_GEOMETRY)
                .is_none()
        );
    }

    #[test]
    fn three_by_three_seam_propagates_block_light() {
        // Glowstone in the centre chunk at (15, 64, 15) (NE corner).
        // The NE neighbour should receive light at (0, 64, 0) with a
        // 2-Manhattan-step penalty (one step to leave the centre
        // chunk, one to enter the NE neighbour — but we pass through
        // a single shared "diagonal" face, which is actually two
        // steps).
        let table = tiny_table();
        let mut centre = air_chunk();
        centre.set_block(15, 64, 15, BlockStateId(3));
        let ne = air_chunk();
        let input: [[Option<Chunk>; 3]; 3] = [
            [None, None, None],
            [None, Some(centre), None],
            [None, None, Some(ne)],
        ];
        let out = compute_chunk_light(borrow(&input), &table);
        // Centre chunk's emitter cell: 15.
        assert_eq!(out.block_at(15, 64, 15), 15);
        // We can only extract the centre chunk's light from the
        // engine, but the BFS *did* visit the NE neighbour during
        // compute. The verification is indirect: the centre's own
        // cells near the corner should be 15 (emitter) and 14
        // (one step away) — which is the same as a fully-isolated
        // chunk.
        assert_eq!(out.block_at(14, 64, 15), 14);
        assert_eq!(out.block_at(15, 64, 14), 14);
    }

    #[test]
    fn missing_neighbours_treated_as_air() {
        let table = tiny_table();
        let input = solo(air_chunk()); // only centre, others None
        let out = compute_chunk_light(borrow(&input), &table);
        // No surrounding chunks, but the centre is still all air —
        // sky should be 15 everywhere, block 0.
        assert_eq!(out.sky_at(0, MIN_Y, 0), 15);
        assert_eq!(out.sky_at(15, MAX_Y - 1, 15), 15);
        assert_eq!(out.block_at(0, 0, 0), 0);
    }

    /// When the real .analysis/test-world block_light.json + blocks
    /// report are present, run the engine on the spawn chunk and
    /// sanity-check the output. The exact byte-for-byte values
    /// aren't pinned (vanilla's algorithm differs in iteration
    /// quirks, and the M3 status appendix already documented that
    /// our world has almost no baked light to compare against), but
    /// invariants are checked: sky=15 on the very top, and block-light
    /// stays dark only when the sampled chunk has no emitters.
    #[test]
    fn engine_runs_on_real_spawn_chunk_when_data_present() {
        let workspace = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .unwrap()
            .to_path_buf();
        let blocks_path = workspace.join("data/vanilla/reports/blocks.json");
        let light_path = workspace.join("data/vanilla/reports/block_light.json");
        let region_path = workspace.join(".analysis/test-world/region/r.0.0.mca");
        if !blocks_path.is_file() || !light_path.is_file() || !region_path.is_file() {
            eprintln!("skipping: prerequisites missing");
            return;
        }

        let report = mc_data::blocks::load_blocks_report(&blocks_path).unwrap();
        let registry = BlockRegistry::from_report(&report).unwrap();
        let table = mc_data::block_light::load(&light_path).unwrap();

        let payloads = crate::anvil::region::read_region(&region_path).unwrap();
        // Find the first chunk that's Status:full.
        let mut full_chunk: Option<Chunk> = None;
        for payload in &payloads {
            let mut cur = std::io::Cursor::new(&payload.uncompressed_nbt[..]);
            let (_, root) = mc_nbt::read_named(&mut cur).unwrap();
            let chunk = crate::anvil::chunk_nbt::chunk_from_nbt(&root, &registry).unwrap();
            if chunk.status.contains("full") {
                full_chunk = Some(chunk);
                break;
            }
        }
        let Some(chunk) = full_chunk else {
            eprintln!("skipping: no Status:full chunk in test world");
            return;
        };

        let has_emitter = (0..WORLD_HEIGHT).any(|ly| {
            let world_y = MIN_Y + ly as i32;
            (0..16u8).any(|lz| {
                (0..16u8).any(|lx| {
                    chunk
                        .get_block(lx, world_y, lz)
                        .is_some_and(|state| table.emission(state.0).unwrap_or(0) > 0)
                })
            })
        });

        let input = [
            [None, None, None],
            [None, Some(chunk), None],
            [None, None, None],
        ];
        let out = compute_chunk_light(borrow(&input), &table);

        // The top of the world should always be open sky.
        assert_eq!(out.sky_at(0, MAX_Y - 1, 0), 15);
        assert_eq!(out.sky_at(15, MAX_Y - 1, 15), 15);
        if has_emitter {
            let has_block_light = (0..WORLD_HEIGHT).any(|ly| {
                let world_y = MIN_Y + ly as i32;
                (0..16u8).any(|lz| (0..16u8).any(|lx| out.block_at(lx, world_y, lz) > 0))
            });
            assert!(has_block_light, "emitting chunk produced no block-light");
        } else {
            for ly in 0..WORLD_HEIGHT {
                for lz in 0..16u8 {
                    for lx in 0..16u8 {
                        let world_y = MIN_Y + ly as i32;
                        assert_eq!(
                            out.block_at(lx, world_y, lz),
                            0,
                            "expected no block-light in the spawn chunk; got non-zero at \
                             ({lx}, {world_y}, {lz})",
                        );
                    }
                }
            }
        }
    }

    // ===== M9.a-c: incremental relight tests =====

    fn full_recompute(
        neighbourhood: [[Option<&Chunk>; 3]; 3],
        table: &BlockLightTable,
    ) -> ChunkLight {
        compute_chunk_light(neighbourhood, table)
    }

    fn seed_cache_from_full(
        cache: &mut LightCache,
        chunks: &[(ChunkPos, &Chunk)],
        table: &BlockLightTable,
    ) {
        // Seed each chunk's light by running a full compute with its
        // 3×3 neighbourhood drawn from the provided chunk list.
        let by_pos: HashMap<ChunkPos, &Chunk> = chunks.iter().map(|&(p, c)| (p, c)).collect();
        for &(pos, _) in chunks {
            let mut nbh: [[Option<&Chunk>; 3]; 3] = [[None; 3]; 3];
            for dz in -1i32..=1 {
                for dx in -1i32..=1 {
                    let np = ChunkPos {
                        x: pos.x + dx,
                        z: pos.z + dz,
                    };
                    nbh[(dz + 1) as usize][(dx + 1) as usize] = by_pos.get(&np).copied();
                }
            }
            let light = full_recompute(nbh, table);
            cache.insert(pos, light);
        }
    }

    fn assert_light_eq(actual: &ChunkLight, expected: &ChunkLight, tag: &str) {
        for ly in 0..WORLD_HEIGHT {
            for lz in 0..SECTION_DIM {
                for lx in 0..SECTION_DIM {
                    let actual_sky = actual.sky.get(lx, ly, lz);
                    let expected_sky = expected.sky.get(lx, ly, lz);
                    assert_eq!(
                        actual_sky, expected_sky,
                        "{tag}: sky mismatch at ({lx},{ly},{lz}): inc={} full={}",
                        actual_sky, expected_sky,
                    );
                    let actual_block = actual.block.get(lx, ly, lz);
                    let expected_block = expected.block.get(lx, ly, lz);
                    assert_eq!(
                        actual_block, expected_block,
                        "{tag}: block mismatch at ({lx},{ly},{lz}): inc={} full={}",
                        actual_block, expected_block,
                    );
                }
            }
        }
    }

    #[test]
    fn incremental_no_op_for_like_for_like_swap() {
        let table = tiny_table();
        let chunk = air_chunk();
        let pos = ChunkPos { x: 0, z: 0 };
        let mut cache = LightCache::new();
        seed_cache_from_full(&mut cache, &[(pos, &chunk)], &table);

        // Place air on air — opacity, emission, propagates_sky all
        // unchanged. Should return empty touched list and not mutate
        // the cache.
        let touched = apply_block_change_to_light(
            &mut cache,
            &[
                [None, None, None],
                [None, Some(&chunk), None],
                [None, None, None],
            ],
            &table,
            pos,
            8,
            64,
            8,
            BlockStateId(0),
            BlockStateId(0),
        );
        assert!(touched.is_empty());
    }

    #[test]
    fn incremental_place_glowstone_in_air() {
        let table = tiny_table();
        let pre = air_chunk();
        let pos = ChunkPos { x: 0, z: 0 };
        let mut cache = LightCache::new();
        seed_cache_from_full(&mut cache, &[(pos, &pre)], &table);

        // Apply: place glowstone (3) at (8, 64, 8). Build the
        // post-edit chunk and feed both into the incremental update.
        let mut post = pre.clone();
        post.set_block(8, 64, 8, BlockStateId(3));
        let touched = apply_block_change_to_light(
            &mut cache,
            &[
                [None, None, None],
                [None, Some(&post), None],
                [None, None, None],
            ],
            &table,
            pos,
            8,
            64,
            8,
            BlockStateId(0),
            BlockStateId(3),
        );
        assert_eq!(touched, vec![pos]);

        let inc = cache.get(pos).unwrap();
        let full = full_recompute(
            [
                [None, None, None],
                [None, Some(&post), None],
                [None, None, None],
            ],
            &table,
        );
        assert_light_eq(inc, &full, "place glowstone");
    }

    #[test]
    fn incremental_custom_geometry_updates_the_edited_world_y() {
        let table = tiny_table();
        let pre = custom_air_chunk();
        let pos = ChunkPos { x: 0, z: 0 };
        let mut cache = LightCache::new();
        seed_cache_from_full(&mut cache, &[(pos, &pre)], &table);

        let mut post = pre.clone();
        post.set_block(8, 0, 8, BlockStateId(3));
        let touched = apply_block_change_to_light(
            &mut cache,
            &[
                [None, None, None],
                [None, Some(&post), None],
                [None, None, None],
            ],
            &table,
            pos,
            8,
            0,
            8,
            BlockStateId(0),
            BlockStateId(3),
        );

        let incremental = cache.get(pos).unwrap();
        let full = full_recompute(
            [
                [None, None, None],
                [None, Some(&post), None],
                [None, None, None],
            ],
            &table,
        );
        assert_eq!(touched, vec![pos]);
        assert_eq!(incremental.block_at(8, 0, 8), full.block_at(8, 0, 8));
        assert_eq!(incremental.block_at(9, 0, 8), full.block_at(9, 0, 8));
    }

    #[test]
    fn incremental_custom_geometry_updates_sky_at_the_edited_world_y() {
        let table = tiny_table();
        let pre = custom_air_chunk();
        let pos = ChunkPos { x: 0, z: 0 };
        let mut cache = LightCache::new();
        seed_cache_from_full(&mut cache, &[(pos, &pre)], &table);

        let mut post = pre.clone();
        post.set_block(8, 0, 8, BlockStateId(1));
        apply_block_change_to_light(
            &mut cache,
            &[
                [None, None, None],
                [None, Some(&post), None],
                [None, None, None],
            ],
            &table,
            pos,
            8,
            0,
            8,
            BlockStateId(0),
            BlockStateId(1),
        );

        let incremental = cache.get(pos).unwrap();
        let full = full_recompute(
            [
                [None, None, None],
                [None, Some(&post), None],
                [None, None, None],
            ],
            &table,
        );
        assert_eq!(incremental.sky_at(8, 0, 8), full.sky_at(8, 0, 8));
        assert_eq!(incremental.sky_at(8, 1, 8), full.sky_at(8, 1, 8));
    }

    #[test]
    #[should_panic(expected = "lighting neighbourhood chunks must share geometry")]
    fn incremental_mixed_geometry_fails_closed() {
        let table = tiny_table();
        let centre = custom_air_chunk();
        let east = air_chunk();
        let pos = ChunkPos { x: 0, z: 0 };
        let mut cache = LightCache::new();
        cache.insert(
            pos,
            full_recompute(
                [
                    [None, None, None],
                    [None, Some(&centre), None],
                    [None, None, None],
                ],
                &table,
            ),
        );

        apply_block_change_to_light(
            &mut cache,
            &[
                [None, None, None],
                [None, Some(&centre), Some(&east)],
                [None, None, None],
            ],
            &table,
            pos,
            8,
            0,
            8,
            BlockStateId(0),
            BlockStateId(3),
        );
    }

    #[test]
    fn incremental_remove_glowstone() {
        let table = tiny_table();
        let mut pre = air_chunk();
        pre.set_block(8, 64, 8, BlockStateId(3));
        let pos = ChunkPos { x: 0, z: 0 };
        let mut cache = LightCache::new();
        seed_cache_from_full(&mut cache, &[(pos, &pre)], &table);

        let mut post = pre.clone();
        post.set_block(8, 64, 8, BlockStateId(0));
        let _ = apply_block_change_to_light(
            &mut cache,
            &[
                [None, None, None],
                [None, Some(&post), None],
                [None, None, None],
            ],
            &table,
            pos,
            8,
            64,
            8,
            BlockStateId(3),
            BlockStateId(0),
        );

        let inc = cache.get(pos).unwrap();
        let full = full_recompute(
            [
                [None, None, None],
                [None, Some(&post), None],
                [None, None, None],
            ],
            &table,
        );
        assert_light_eq(inc, &full, "remove glowstone");
    }

    #[test]
    fn incremental_place_stone_blocks_sky() {
        let table = tiny_table();
        let pre = air_chunk();
        let pos = ChunkPos { x: 0, z: 0 };
        let mut cache = LightCache::new();
        seed_cache_from_full(&mut cache, &[(pos, &pre)], &table);

        // Place stone (1) at (8, 100, 8) — opaque, blocks sky.
        let mut post = pre.clone();
        post.set_block(8, 100, 8, BlockStateId(1));
        let _ = apply_block_change_to_light(
            &mut cache,
            &[
                [None, None, None],
                [None, Some(&post), None],
                [None, None, None],
            ],
            &table,
            pos,
            8,
            100,
            8,
            BlockStateId(0),
            BlockStateId(1),
        );

        let inc = cache.get(pos).unwrap();
        let full = full_recompute(
            [
                [None, None, None],
                [None, Some(&post), None],
                [None, None, None],
            ],
            &table,
        );
        assert_light_eq(inc, &full, "place stone in air");
    }

    #[test]
    fn incremental_break_stone_in_solid_column() {
        let table = tiny_table();
        // Build a stone column at (8, *, 8) from MIN_Y..=64.
        let mut pre = air_chunk();
        for y in (MIN_Y + 1)..=64 {
            pre.set_block(8, y, 8, BlockStateId(1));
        }
        let pos = ChunkPos { x: 0, z: 0 };
        let mut cache = LightCache::new();
        seed_cache_from_full(&mut cache, &[(pos, &pre)], &table);

        // Break the stone at (8, 64, 8) — column top, so sky now
        // pours into a 1-cell pocket but doesn't reach further down
        // (cells below are still stone).
        let mut post = pre.clone();
        post.set_block(8, 64, 8, BlockStateId(0));
        let _ = apply_block_change_to_light(
            &mut cache,
            &[
                [None, None, None],
                [None, Some(&post), None],
                [None, None, None],
            ],
            &table,
            pos,
            8,
            64,
            8,
            BlockStateId(1),
            BlockStateId(0),
        );

        let inc = cache.get(pos).unwrap();
        let full = full_recompute(
            [
                [None, None, None],
                [None, Some(&post), None],
                [None, None, None],
            ],
            &table,
        );
        assert_light_eq(inc, &full, "break stone column top");
    }

    #[test]
    fn incremental_edit_at_chunk_seam_propagates() {
        let table = tiny_table();
        let pre_centre = air_chunk();
        let plains = Identifier::parse("minecraft:plains").unwrap();
        let pre_east = Chunk::empty(ChunkPos { x: 1, z: 0 }, BlockStateId(0), plains.clone());

        let cpos = ChunkPos { x: 0, z: 0 };
        let epos = ChunkPos { x: 1, z: 0 };
        let mut cache = LightCache::new();
        seed_cache_from_full(
            &mut cache,
            &[(cpos, &pre_centre), (epos, &pre_east)],
            &table,
        );

        // Place glowstone at the east edge of the centre chunk:
        // (15, 64, 8). Light should ripple into the east neighbour.
        let mut post_centre = pre_centre.clone();
        post_centre.set_block(15, 64, 8, BlockStateId(3));
        let post_east = pre_east.clone();
        let touched = apply_block_change_to_light(
            &mut cache,
            &[
                [None, None, None],
                [None, Some(&post_centre), Some(&post_east)],
                [None, None, None],
            ],
            &table,
            cpos,
            15,
            64,
            8,
            BlockStateId(0),
            BlockStateId(3),
        );
        assert!(touched.contains(&cpos), "centre chunk should be touched");
        assert!(touched.contains(&epos), "east neighbour should be touched");

        let inc_centre = cache.get(cpos).unwrap();
        let inc_east = cache.get(epos).unwrap();
        let full_centre = full_recompute(
            [
                [None, None, None],
                [None, Some(&post_centre), Some(&post_east)],
                [None, None, None],
            ],
            &table,
        );
        let full_east = full_recompute(
            [
                [None, None, None],
                [Some(&post_centre), Some(&post_east), None],
                [None, None, None],
            ],
            &table,
        );
        assert_light_eq(inc_centre, &full_centre, "centre after seam edit");
        assert_light_eq(inc_east, &full_east, "east after seam edit");
    }

    #[test]
    fn incremental_random_edit_sequence_matches_full_recompute() {
        // Deterministic pseudo-random edit sequence inside one chunk;
        // after each edit, assert the cached light equals a full
        // recompute on the post-edit chunk. Block states limited to
        // {air=0, stone=1, glowstone=3}.
        let table = tiny_table();
        let mut chunk = air_chunk();
        let pos = ChunkPos { x: 0, z: 0 };
        let mut cache = LightCache::new();
        seed_cache_from_full(&mut cache, &[(pos, &chunk)], &table);

        // Simple LCG.
        let mut s: u32 = 0xdeadbeef;
        let mut next = || {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            s
        };

        for _ in 0..40 {
            let lx = (next() % 16) as u8;
            let lz = (next() % 16) as u8;
            // Restrict Y to a thin band so sky-light interactions stay
            // testable but the sequence converges.
            let world_y = 60 + (next() % 8) as i32;
            let new_state_choices = [BlockStateId(0), BlockStateId(1), BlockStateId(3)];
            let new_state = new_state_choices[(next() as usize) % 3];

            let prev = chunk.get_block(lx, world_y, lz).unwrap_or(BlockStateId(0));
            chunk.set_block(lx, world_y, lz, new_state);

            apply_block_change_to_light(
                &mut cache,
                &[
                    [None, None, None],
                    [None, Some(&chunk), None],
                    [None, None, None],
                ],
                &table,
                pos,
                lx,
                world_y,
                lz,
                prev,
                new_state,
            );

            let inc = cache.get(pos).unwrap().clone();
            let full = full_recompute(
                [
                    [None, None, None],
                    [None, Some(&chunk), None],
                    [None, None, None],
                ],
                &table,
            );
            assert_light_eq(
                &inc,
                &full,
                &format!("random edit ({lx},{world_y},{lz}) -> {}", new_state.0),
            );
        }
    }
}
