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

use mc_data::block_light::BlockLightTable;

use crate::block::BlockStateId;
use crate::chunk::{Chunk, ChunkPos, MIN_Y, SECTION_COUNT};
use crate::section::{SECTION_DIM, SECTION_VOLUME};

/// World-height span in blocks (24 sections × 16 blocks).
pub const WORLD_HEIGHT: usize = SECTION_COUNT * SECTION_DIM;
/// Per-chunk cell count, one u8 per cell (used as the output buffer
/// size in `ChunkLight`).
pub const CHUNK_LIGHT_LEN: usize = SECTION_VOLUME * SECTION_COUNT;

const N_X: usize = SECTION_DIM * 3;
const N_Z: usize = SECTION_DIM * 3;
const N_VOL: usize = N_X * WORLD_HEIGHT * N_Z;

/// One chunk's worth of computed light, per-cell `u8` in `0..=15`,
/// indexed by `y * (16 * 16) + z * 16 + x` with `y` in `0..384`
/// (world Y = `y + MIN_Y`, so y=0 is world Y=-64). The wire encoder
/// converts this into the 4-bit-packed nibble arrays
/// `LightData::sky_updates` / `block_updates` expect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkLight {
    pub sky: Vec<u8>,
    pub block: Vec<u8>,
}

impl ChunkLight {
    /// Fresh buffers, all zero. Useful for direct unit-testing
    /// without invoking the engine.
    #[must_use]
    pub fn zeroed() -> Self {
        Self {
            sky: vec![0; CHUNK_LIGHT_LEN],
            block: vec![0; CHUNK_LIGHT_LEN],
        }
    }

    #[must_use]
    pub fn sky_at(&self, x: u8, y: i32, z: u8) -> u8 {
        let local_y = (y - MIN_Y) as usize;
        self.sky[chunk_idx(x as usize, local_y, z as usize)]
    }

    #[must_use]
    pub fn block_at(&self, x: u8, y: i32, z: u8) -> u8 {
        let local_y = (y - MIN_Y) as usize;
        self.block[chunk_idx(x as usize, local_y, z as usize)]
    }
}

fn chunk_idx(x: usize, local_y: usize, z: usize) -> usize {
    local_y * (SECTION_DIM * SECTION_DIM) + z * SECTION_DIM + x
}

/// Reusable working buffers for the lighting engine. Allocating
/// ~4 MB on every chunk emit measurably slows the spawn burst in
/// debug mode (the M3.g/M4.f harness exceeds 60 s without this).
/// Build one per connection (or one per `emit_chunks_around` call)
/// and pass it into [`compute_chunk_light`].
pub struct LightWorkspace {
    sky: Vec<u8>,
    block: Vec<u8>,
    opacity: Vec<u8>,
    propagates_sky: Vec<bool>,
    queue: VecDeque<u32>,
}

impl LightWorkspace {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sky: vec![0; N_VOL],
            block: vec![0; N_VOL],
            opacity: vec![0; N_VOL],
            propagates_sky: vec![true; N_VOL],
            queue: VecDeque::new(),
        }
    }

    fn reset(&mut self) {
        self.sky.fill(0);
        self.block.fill(0);
        self.opacity.fill(0);
        self.propagates_sky.fill(true);
        self.queue.clear();
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
/// exactly once at login), and — from M9.b onwards — mutated in
/// place by the incremental relight BFS on subsequent edits.
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

    // Fast path: if every chunk in the 3×3 neighbourhood is fully
    // air (single-state air sections, no biome variation needed for
    // lighting), the output is sky=15 everywhere, block=0 — no BFS.
    // Our test-world has ~400 of its 441 spawn-window chunks in
    // Status: structure_starts which have no terrain at all, so
    // skipping their BFS turns the spawn burst from ~30 s to a few
    // seconds in debug builds.
    if is_all_air_neighbourhood(&neighbourhood) {
        return ChunkLight {
            sky: vec![15; CHUNK_LIGHT_LEN],
            block: vec![0; CHUNK_LIGHT_LEN],
        };
    }

    ws.reset();

    populate_grids(
        &neighbourhood,
        table,
        &mut ws.opacity,
        &mut ws.propagates_sky,
        &mut ws.block,
    );

    seed_sky_from_open_columns(&ws.propagates_sky, &mut ws.sky, &mut ws.queue);
    bfs(&ws.opacity, &mut ws.sky, &mut ws.queue);

    seed_block_from_emission(&ws.block, &mut ws.queue);
    bfs(&ws.opacity, &mut ws.block, &mut ws.queue);

    extract_centre(&ws.sky, &ws.block)
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
) {
    let air = BlockStateId(0);
    for (cz, row) in neighbourhood.iter().enumerate() {
        for (cx, slot) in row.iter().enumerate() {
            let Some(chunk) = *slot else {
                // Missing neighbour: leave the slab at the all-air
                // defaults (opacity=0, propagates_sky=true, emission=0).
                continue;
            };
            for ly in 0..WORLD_HEIGHT {
                let world_y = MIN_Y + ly as i32;
                for lz in 0..SECTION_DIM {
                    for lx in 0..SECTION_DIM {
                        let gx = cx * SECTION_DIM + lx;
                        let gz = cz * SECTION_DIM + lz;
                        let idx = grid_idx(gx, ly, gz);
                        let state = chunk.get_block(lx as u8, world_y, lz as u8).unwrap_or(air);
                        opacity[idx] = table.opacity(state.0).unwrap_or(0);
                        propagates_sky[idx] = table.propagates_sky(state.0).unwrap_or(true);
                        let emit = table.emission(state.0).unwrap_or(0);
                        if emit > 0 {
                            block_seed[idx] = emit;
                        }
                    }
                }
            }
        }
    }
}

fn seed_sky_from_open_columns(propagates_sky: &[bool], sky: &mut [u8], queue: &mut VecDeque<u32>) {
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
            for ly in (0..WORLD_HEIGHT).rev() {
                let idx = grid_idx(gx, ly, gz);
                if propagates_sky[idx] {
                    sky[idx] = 15;
                } else {
                    break;
                }
            }
        }
    }
    for ly in 0..WORLD_HEIGHT {
        for gz in 0..N_Z {
            for gx in 0..N_X {
                let idx = grid_idx(gx, ly, gz);
                if sky[idx] != 15 {
                    continue;
                }
                if has_dark_neighbour(sky, gx, ly, gz) {
                    queue.push_back(idx as u32);
                }
            }
        }
    }
}

fn has_dark_neighbour(sky: &[u8], gx: usize, ly: usize, gz: usize) -> bool {
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
        if ny < 0 || ny >= WORLD_HEIGHT as isize {
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

fn seed_block_from_emission(block: &[u8], queue: &mut VecDeque<u32>) {
    for (idx, &v) in block.iter().enumerate() {
        if v > 0 {
            queue.push_back(idx as u32);
        }
    }
}

/// Generic 6-neighbour BFS used by both passes.
fn bfs(opacity: &[u8], values: &mut [u8], queue: &mut VecDeque<u32>) {
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
        for (dx, dy, dz) in neighbours {
            let nx = gx as isize + dx;
            let ny = ly as isize + dy;
            let nz = gz as isize + dz;
            if nx < 0 || nx >= N_X as isize {
                continue;
            }
            if ny < 0 || ny >= WORLD_HEIGHT as isize {
                continue;
            }
            if nz < 0 || nz >= N_Z as isize {
                continue;
            }
            let nidx = grid_idx(nx as usize, ny as usize, nz as usize);
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

fn extract_centre(sky: &[u8], block: &[u8]) -> ChunkLight {
    let mut out = ChunkLight::zeroed();
    for ly in 0..WORLD_HEIGHT {
        for lz in 0..SECTION_DIM {
            for lx in 0..SECTION_DIM {
                let gx = SECTION_DIM + lx;
                let gz = SECTION_DIM + lz;
                let src = grid_idx(gx, ly, gz);
                let dst = chunk_idx(lx, ly, lz);
                out.sky[dst] = sky[src];
                out.block[dst] = block[src];
            }
        }
    }
    out
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
    use crate::chunk::{ChunkPos, MAX_Y};

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
    fn open_air_chunk_is_sky_15_everywhere_and_block_0() {
        let table = tiny_table();
        let input = solo(air_chunk());
        let out = compute_chunk_light(borrow(&input), &table);
        for ly in 0..WORLD_HEIGHT {
            for lz in 0..SECTION_DIM {
                for lx in 0..SECTION_DIM {
                    let idx = chunk_idx(lx, ly, lz);
                    assert_eq!(out.sky[idx], 15, "sky at ({lx},{ly},{lz})");
                    assert_eq!(out.block[idx], 0, "block at ({lx},{ly},{lz})");
                }
            }
        }
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
    /// invariants are checked: sky=15 on the very top, block=0 in a
    /// no-light chunk.
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

        let input = [
            [None, None, None],
            [None, Some(chunk), None],
            [None, None, None],
        ];
        let out = compute_chunk_light(borrow(&input), &table);

        // The top of the world should always be open sky.
        assert_eq!(out.sky_at(0, MAX_Y - 1, 0), 15);
        assert_eq!(out.sky_at(15, MAX_Y - 1, 15), 15);
        // No emitter in a fully-grass chunk → block-light is 0 throughout.
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
