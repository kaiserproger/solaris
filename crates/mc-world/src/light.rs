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

use std::collections::VecDeque;

#[cfg(target_endian = "little")]
use bytemuck::cast;
use mc_data::block_light::BlockLightTable;
use rustc_hash::FxHashMap;
#[cfg(target_endian = "little")]
use wide::{i16x8, u8x16, u16x8};

use crate::block::BlockStateId;
use crate::chunk::{
    Chunk, ChunkGeometry, ChunkPos, LIGHT_LAYER_BYTES, LightSection, MIN_Y, OVERWORLD_GEOMETRY,
    SECTION_COUNT, SectionLight, top_opaque_column,
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

/// One computed light channel. Every section is known, including zero;
/// uniform sections allocate nothing and mixed sections use copy-on-write storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightLayer {
    sections: Vec<LightSection>,
}

impl LightLayer {
    #[must_use]
    pub fn zeroed() -> Self {
        Self::zeroed_for_sections(SECTION_COUNT)
    }

    fn zeroed_for_sections(section_count: usize) -> Self {
        Self {
            sections: vec![LightSection::uniform(0); section_count],
        }
    }

    #[must_use]
    pub fn filled(value: u8) -> Self {
        Self::filled_for_sections(value, SECTION_COUNT)
    }

    fn filled_for_sections(value: u8, section_count: usize) -> Self {
        assert!(value <= 15);
        let packed = value | (value << 4);
        Self {
            sections: vec![LightSection::uniform(packed); section_count],
        }
    }

    #[inline]
    #[must_use]
    pub fn get(&self, x: usize, local_y: usize, z: usize) -> u8 {
        debug_assert!(x < SECTION_DIM);
        debug_assert!(local_y < self.sections.len() * SECTION_DIM);
        debug_assert!(z < SECTION_DIM);
        let section_idx = local_y / SECTION_DIM;
        self.sections[section_idx].get(section_cell_idx(x, local_y % SECTION_DIM, z))
    }

    pub fn set(&mut self, x: usize, local_y: usize, z: usize, value: u8) {
        debug_assert!(x < SECTION_DIM);
        debug_assert!(local_y < self.sections.len() * SECTION_DIM);
        debug_assert!(z < SECTION_DIM);
        debug_assert!(value <= 15);
        let section_idx = local_y / SECTION_DIM;
        self.sections[section_idx].set(section_cell_idx(x, local_y % SECTION_DIM, z), value);
    }

    #[must_use]
    pub fn section(&self, section_idx: usize) -> Option<&LightSection> {
        self.sections.get(section_idx)
    }

    pub(crate) fn section_count(&self) -> usize {
        self.sections.len()
    }

    fn set_section(&mut self, section_idx: usize, layer: &LightSection) {
        self.sections[section_idx] = layer.clone();
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

    /// Fill both channels for the owning dimension's vertical geometry.
    #[must_use]
    pub fn filled_for_geometry(sky: u8, block: u8, geometry: ChunkGeometry) -> Self {
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
                out.sky.set_section(section_idx, sky);
                any = true;
            }
            if let Some(block) = &section.block {
                out.block.set_section(section_idx, block);
                any = true;
            }
        }
        any.then_some(out)
    }

    pub(crate) fn write_section_lights(&self, section_lights: &mut [SectionLight]) -> bool {
        let mut changed = false;
        for (section_idx, section) in section_lights.iter_mut().enumerate() {
            let sky = self.sky.section(section_idx);
            if section.sky.as_ref() != sky {
                section.sky = sky.cloned();
                changed = true;
            }
            let block = self.block.section(section_idx);
            if section.block.as_ref() != block {
                section.block = block.cloned();
                changed = true;
            }
        }
        changed
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

    /// Retained scratch heap storage, including queue spare capacity.
    #[must_use]
    pub fn allocated_bytes(&self) -> usize {
        self.sky.capacity()
            + self.block.capacity()
            + self.opacity.capacity()
            + self.propagates_sky.capacity() * size_of::<bool>()
            + self.queue.capacity() * size_of::<u32>()
            + self.emitters.capacity() * size_of::<u32>()
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

/// Cache of computed [`ChunkLight`] keyed by [`ChunkPos`], maintained by the
/// lighting owner and updated by [`apply_block_change_to_light`] on edits.
///
/// Uniform sections have no payload allocation; mixed sections share their
/// packed storage with baked chunk snapshots until a light write forks them.
/// Positions are internal integer keys, so the measured tick hot path uses
/// the workspace's non-cryptographic map rather than SipHash.
#[derive(Debug, Default, Clone)]
pub struct LightCache {
    chunks: FxHashMap<ChunkPos, ChunkLight>,
}

impl LightCache {
    #[must_use]
    pub fn new() -> Self {
        Self {
            chunks: FxHashMap::default(),
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

    let shape_changed = table.faces(prev_state.0) != table.faces(new_state.0);
    if prev_emit == new_emit && prev_op == new_op && prev_sky_pass == new_sky_pass && !shape_changed
    {
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

    if prev_op != new_op || prev_sky_pass != new_sky_pass || shape_changed {
        let reseed_column = shape_changed
            || highest_opaque_changed(
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

fn faces_at_coord(
    chunks: &[[Option<&Chunk>; 3]; 3],
    table: &BlockLightTable,
    coord: WCoord,
    geometry: ChunkGeometry,
) -> u32 {
    let (cx, cz, lx, ly, lz) = coord_parts(coord);
    chunks[cz][cx]
        .and_then(|chunk| chunk.get_block(lx as u8, geometry.min_y() + ly as i32, lz as u8))
        .map_or(0, |state| table.faces(state.0))
}

fn face_blocked(
    chunks: &[[Option<&Chunk>; 3]; 3],
    table: &BlockLightTable,
    from: WCoord,
    to: WCoord,
    geometry: ChunkGeometry,
) -> bool {
    let (axis, positive) = if from.0 != to.0 {
        (0, to.0 > from.0)
    } else if from.1 != to.1 {
        (1, to.1 > from.1)
    } else {
        (2, to.2 > from.2)
    };
    let side = axis * 2 + usize::from(positive);
    let a = faces_at_coord(chunks, table, from, geometry);
    let b = faces_at_coord(chunks, table, to, geometry);
    ((a >> (side * 4)) | (b >> ((side ^ 1) * 4))) & 15 == 15
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
            if face_blocked(chunks, table, coord, ncoord, geometry) {
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
        for ly in edit_ly..geometry.height() as usize {
            let coord = (gx, ly, gz);
            if !propagates_sky_at_coord(chunks, table, coord, geometry)
                || face_blocked(chunks, table, (gx, ly + 1, gz), coord, geometry)
            {
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
        let mut column_open_above_self = new_self == 15;
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
            column_open_above_self &=
                !face_blocked(chunks, table, (gx, ly + 1, gz), coord, geometry);
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
            if face_blocked(chunks, table, coord, ncoord, geometry) {
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

    seed_sky_from_open_columns(
        &ws.propagates_sky,
        &mut ws.sky,
        &mut ws.queue,
        world_height,
        neighbourhood.map(|row| row.map(|slot| slot.is_none())),
        |from, to| {
            let target = grid_idx(to.0, to.1, to.2);
            let marked = ws.opacity[target] & 128 != 0
                || (from.1 < world_height
                    && ws.opacity[grid_idx(from.0, from.1, from.2)] & 128 != 0);
            marked && face_blocked(&neighbourhood, table, from, to, geometry)
        },
    );
    bfs(
        &ws.opacity,
        &mut ws.sky,
        &mut ws.queue,
        world_height,
        |from, to| face_blocked(&neighbourhood, table, from, to, geometry),
    );

    if !ws.emitters.is_empty() {
        ws.queue.extend(ws.emitters.iter().copied());
        bfs(
            &ws.opacity,
            &mut ws.block,
            &mut ws.queue,
            world_height,
            |from, to| face_blocked(&neighbourhood, table, from, to, geometry),
        );
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
            // Scalar height hints cannot certify directional shape occlusion.
            if chunk.sections.iter().any(|section| {
                section.palette().map_or_else(
                    || table.faces(section.get(0, 0, 0).0) != 0,
                    |palette| palette.iter().any(|state| table.faces(state.0) != 0),
                )
            }) {
                return false;
            }
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
                    let op = table.opacity(state.0).unwrap_or(0)
                        | if table.faces(state.0) != 0 { 128 } else { 0 };
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
    let palette = section.palette().expect("indirect section palette");
    let indices = section.indices().expect("indirect section indices");
    let bits = usize::from(indices.bits_per_entry());
    let entries = 64 / bits;
    let mask = (1_u64 << bits) - 1;
    // Decode each packed word once. Calling section.get for all 4096 cells
    // repeats a runtime quotient/remainder for the same word.
    for (word_index, &word) in indices.words().iter().enumerate() {
        let start = word_index * entries;
        for offset in 0..entries.min(SECTION_VOLUME - start) {
            let cell = start + offset;
            let lx = cell % SECTION_DIM;
            let lz = (cell / SECTION_DIM) % SECTION_DIM;
            let ly = base_ly + cell / (SECTION_DIM * SECTION_DIM);
            let idx = grid_idx(cx * SECTION_DIM + lx, ly, cz * SECTION_DIM + lz);
            let state = palette[((word >> (offset * bits)) & mask) as usize];
            opacity[idx] = table.opacity(state.0).unwrap_or(0)
                | if table.faces(state.0) != 0 { 128 } else { 0 };
            propagates_sky[idx] = table.propagates_sky(state.0).unwrap_or(true);
            let emit = table.emission(state.0).unwrap_or(0);
            if emit > 0 {
                block_seed[idx] = emit;
                emitters.push(idx as u32);
            }
        }
    }
}

/// `missing` marks absent neighbour slabs (`[z][x]`): all air, so every one of
/// their columns is open sky down to the floor.
fn seed_sky_from_open_columns(
    propagates_sky: &[bool],
    sky: &mut [u8],
    queue: &mut VecDeque<u32>,
    world_height: usize,
    missing: [[bool; 3]; 3],
    occludes: impl Fn(WCoord, WCoord) -> bool,
) {
    for (cz, row) in missing.iter().enumerate() {
        for (cx, _) in row.iter().enumerate().filter(|(_, missing)| **missing) {
            for ly in 0..world_height {
                for lz in 0..SECTION_DIM {
                    let start = grid_idx(cx * SECTION_DIM, ly, cz * SECTION_DIM + lz);
                    sky[start..start + SECTION_DIM].fill(15);
                }
            }
        }
    }
    // Each column's direct sky is one interval [bottom, world_height).
    // Its boundary consists of the bottom cell and the portions below
    // neighbouring columns' bottoms; no full-volume neighbour scan is needed.
    let mut bottoms = [0; N_X * N_Z];
    for gx in 0..N_X {
        for gz in 0..N_Z {
            if missing[gz / SECTION_DIM][gx / SECTION_DIM] {
                continue;
            }
            for ly in (0..world_height).rev() {
                let idx = grid_idx(gx, ly, gz);
                if propagates_sky[idx] && !occludes((gx, ly + 1, gz), (gx, ly, gz)) {
                    sky[idx] = 15;
                } else {
                    bottoms[gz * N_X + gx] = ly + 1;
                    break;
                }
            }
        }
    }
    for gz in 0..N_Z {
        for gx in 0..N_X {
            let column = gz * N_X + gx;
            let bottom = bottoms[column];
            if bottom == world_height {
                continue;
            }
            let mut end = if bottom > 0 { bottom + 1 } else { 0 };
            if gx > 0 {
                end = end.max(bottoms[column - 1]);
            }
            if gx + 1 < N_X {
                end = end.max(bottoms[column + 1]);
            }
            if gz > 0 {
                end = end.max(bottoms[column - N_X]);
            }
            if gz + 1 < N_Z {
                end = end.max(bottoms[column + N_X]);
            }
            for ly in bottom..end {
                queue.push_back(grid_idx(gx, ly, gz) as u32);
            }
        }
    }
}

/// Generic 6-neighbour BFS used by both passes.
fn bfs(
    opacity: &[u8],
    values: &mut [u8],
    queue: &mut VecDeque<u32>,
    world_height: usize,
    occludes: impl Fn(WCoord, WCoord) -> bool,
) {
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
            // The high bit is a cold-path marker, not additional opacity.
            if (opacity[idx] | opacity[nidx]) & 128 != 0
                && occludes((gx, ly, gz), (nx as usize, ny as usize, nz as usize))
            {
                continue;
            }
            let cost = (opacity[nidx] & 15).max(1);
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
        for (pair, output) in row.as_chunks::<2>().0.iter().zip(output) {
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
    let mut sections = Vec::with_capacity(section_count);
    for section_idx in 0..section_count {
        let mut packed = [0_u8; LIGHT_LAYER_BYTES];
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
        sections.push(LightSection::from_bytes_with_nonzero_count(packed, nonzero));
    }
    LightLayer { sections }
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
    let mut sections = Vec::with_capacity(section_count);
    for section_idx in 0..section_count {
        let mut packed = [0_u8; LIGHT_LAYER_BYTES];
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
        sections.push(LightSection::from_bytes_with_nonzero_count(packed, nonzero));
    }
    LightLayer { sections }
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
#[path = "light_boundary_tests.rs"]
mod boundary_tests;

#[cfg(test)]
#[path = "light_tests.rs"]
mod tests;
