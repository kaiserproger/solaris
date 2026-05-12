//! Chunk section: a 16×16×16 cube of block states with a small,
//! growable palette.
//!
//! Storage mirrors vanilla / Anvil:
//!
//! - **Single** — the whole section is one state. No index array, no
//!   palette overhead. Common for the void above the build height and
//!   for the stone-only mid-overworld layers.
//! - **Indirect** — a `Vec<BlockStateId>` palette and a packed
//!   bit-array of indices. `bits_per_entry` grows naturally with
//!   palette size, minimum 4 (vanilla's pad-to-4 convention for
//!   the on-wire LinearPalette format). M5.c.3 removed the
//!   256-entry cap; sections with more than 256 distinct states
//!   stay indirect on disk but the wire encoder converts to the
//!   direct format described below at emit time.
//!
//! On the wire (`mc_world::wire::encode_chunk_data`), sections with
//! `bits_per_entry >= 9` switch to the **GlobalPalette / Direct**
//! shape: no palette section is emitted, and the per-cell indices
//! are widened to raw global-state ids at
//! `ceil(log2(num_global_states))` bits. The disk codec keeps the
//! palette+indices shape vanilla writes regardless of palette size.
//!
//! Packing follows vanilla's "entries do not cross i64 boundaries"
//! convention: `entries_per_word = 64 / bits_per_entry` and the high
//! bits in each word are padding. This matches what real `.mca`
//! files contain.

use crate::block::BlockStateId;

pub const SECTION_DIM: usize = 16;
pub const SECTION_VOLUME: usize = SECTION_DIM * SECTION_DIM * SECTION_DIM;

const MIN_INDIRECT_BITS: u8 = 4;

/// A 16×16×16 cube of block states.
///
/// `air` is the state treated as "empty" for the `non_air_count`
/// fast-skip path. Callers that need to count cave-air / void-air
/// separately can store multiple sections with distinct `air`
/// values; the typical caller passes `BlockRegistry::block(air).default`.
#[derive(Debug, Clone)]
pub struct ChunkSection {
    storage: BlockStorage,
    non_air_count: u16,
    air: BlockStateId,
}

#[derive(Debug, Clone)]
enum BlockStorage {
    Single(BlockStateId),
    Indirect {
        palette: Vec<BlockStateId>,
        indices: PackedBitArray,
    },
}

impl ChunkSection {
    /// A section pre-filled with `state`. `air` declares what counts
    /// as empty for `non_air_count`.
    #[must_use]
    pub fn filled(state: BlockStateId, air: BlockStateId) -> Self {
        let non_air_count = if state == air {
            0
        } else {
            SECTION_VOLUME as u16
        };
        Self {
            storage: BlockStorage::Single(state),
            non_air_count,
            air,
        }
    }

    #[must_use]
    pub fn get(&self, x: u8, y: u8, z: u8) -> BlockStateId {
        let idx = cell_index(x, y, z);
        match &self.storage {
            BlockStorage::Single(s) => *s,
            BlockStorage::Indirect { palette, indices } => {
                let p = indices.get(idx) as usize;
                palette[p]
            }
        }
    }

    /// Set a cell and return the previous state.
    pub fn set(&mut self, x: u8, y: u8, z: u8, state: BlockStateId) -> BlockStateId {
        let idx = cell_index(x, y, z);
        let prev = match &mut self.storage {
            BlockStorage::Single(current) => {
                let current = *current;
                if current == state {
                    return current;
                }
                self.storage = promote_from_single(current, state, idx);
                current
            }
            BlockStorage::Indirect { palette, indices } => {
                let prev_p = indices.get(idx) as usize;
                let prev_state = palette[prev_p];
                if prev_state == state {
                    return prev_state;
                }
                let new_p = match palette.iter().position(|&s| s == state) {
                    Some(p) => p,
                    None => {
                        palette.push(state);
                        let needed_bits = bits_for_palette(palette.len());
                        if needed_bits > indices.bits_per_entry() {
                            indices.rebit(needed_bits);
                        }
                        palette.len() - 1
                    }
                };
                indices.set(idx, new_p as u32);
                prev_state
            }
        };

        if prev == self.air && state != self.air {
            self.non_air_count += 1;
        } else if prev != self.air && state == self.air {
            self.non_air_count -= 1;
        }
        prev
    }

    /// Build a section directly from a palette + index array. The
    /// codec uses this when loading vanilla `.mca` files. Caller must
    /// supply a `PackedBitArray` of length `SECTION_VOLUME` whose
    /// every entry is < `palette.len()`. `non_air_count` is computed
    /// by walking the indices once.
    #[must_use]
    pub fn from_indirect(
        palette: Vec<BlockStateId>,
        indices: PackedBitArray,
        air: BlockStateId,
    ) -> Self {
        assert_eq!(indices.len(), SECTION_VOLUME);
        let non_air_count = (0..SECTION_VOLUME)
            .filter(|&i| {
                let p = indices.get(i) as usize;
                palette[p] != air
            })
            .count() as u16;
        Self {
            storage: BlockStorage::Indirect { palette, indices },
            non_air_count,
            air,
        }
    }

    /// Access the raw packed indices, for the Anvil writer.
    #[must_use]
    pub fn indices(&self) -> Option<&PackedBitArray> {
        match &self.storage {
            BlockStorage::Single(_) => None,
            BlockStorage::Indirect { indices, .. } => Some(indices),
        }
    }

    #[must_use]
    pub fn non_air_count(&self) -> u16 {
        self.non_air_count
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.non_air_count == 0
    }

    /// Returns the palette in its current order, or `None` if the
    /// section is still in Single mode.
    #[must_use]
    pub fn palette(&self) -> Option<&[BlockStateId]> {
        match &self.storage {
            BlockStorage::Single(_) => None,
            BlockStorage::Indirect { palette, .. } => Some(palette),
        }
    }
}

/// Promote a previously single-valued section to indirect mode after
/// one cell got overwritten.
fn promote_from_single(current: BlockStateId, new: BlockStateId, idx: usize) -> BlockStorage {
    let palette = vec![current, new];
    let mut indices = PackedBitArray::zeroed(MIN_INDIRECT_BITS, SECTION_VOLUME);
    // Every cell currently maps to palette index 0 (zeroed); flip the
    // one we're setting to palette index 1.
    indices.set(idx, 1);
    BlockStorage::Indirect { palette, indices }
}

fn cell_index(x: u8, y: u8, z: u8) -> usize {
    debug_assert!((x as usize) < SECTION_DIM);
    debug_assert!((y as usize) < SECTION_DIM);
    debug_assert!((z as usize) < SECTION_DIM);
    (y as usize * SECTION_DIM + z as usize) * SECTION_DIM + x as usize
}

fn bits_for_palette(palette_len: usize) -> u8 {
    if palette_len <= 1 {
        return MIN_INDIRECT_BITS;
    }
    let bits = (palette_len - 1).ilog2() as u8 + 1;
    bits.max(MIN_INDIRECT_BITS)
}

// ---------------------------------------------------------------------
// PackedBitArray
// ---------------------------------------------------------------------

/// `len` entries of `bits_per_entry` bits, packed into `u64` words
/// without crossing word boundaries (the vanilla / Anvil layout).
///
/// Padding bits at the top of each word are unused; this wastes a
/// few bits per section relative to a fully contiguous packing but
/// matches what `.mca` files write on disk, so the codec in M2.e
/// can move bytes through verbatim.
#[derive(Debug, Clone)]
pub struct PackedBitArray {
    bits_per_entry: u8,
    len: usize,
    data: Vec<u64>,
}

impl PackedBitArray {
    /// All-zero array with the given bit-width and length.
    #[must_use]
    pub fn zeroed(bits_per_entry: u8, len: usize) -> Self {
        assert!((1..=32).contains(&bits_per_entry));
        let epw = entries_per_word(bits_per_entry);
        let words = len.div_ceil(epw);
        Self {
            bits_per_entry,
            len,
            data: vec![0; words],
        }
    }

    /// Build an array from a pre-packed word slice. Used when loading
    /// data from disk: the words are already in the vanilla layout.
    #[must_use]
    pub fn from_words(bits_per_entry: u8, len: usize, words: Vec<u64>) -> Self {
        assert!((1..=32).contains(&bits_per_entry));
        let epw = entries_per_word(bits_per_entry);
        assert_eq!(words.len(), len.div_ceil(epw));
        Self {
            bits_per_entry,
            len,
            data: words,
        }
    }

    #[must_use]
    pub fn bits_per_entry(&self) -> u8 {
        self.bits_per_entry
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub fn get(&self, idx: usize) -> u32 {
        assert!(idx < self.len);
        let bits = self.bits_per_entry as u32;
        let epw = entries_per_word(self.bits_per_entry);
        let (w, o) = (idx / epw, (idx % epw) as u32 * bits);
        let mask = if bits == 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        };
        ((self.data[w] >> o) & mask) as u32
    }

    pub fn set(&mut self, idx: usize, value: u32) {
        assert!(idx < self.len);
        let bits = self.bits_per_entry as u32;
        let max_value = if bits == 32 {
            u32::MAX
        } else {
            (1u32 << bits) - 1
        };
        assert!(value <= max_value, "value {value} exceeds {bits}-bit field");
        let epw = entries_per_word(self.bits_per_entry);
        let (w, o) = (idx / epw, (idx % epw) as u32 * bits);
        let mask = if bits == 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        };
        self.data[w] = (self.data[w] & !(mask << o)) | ((value as u64 & mask) << o);
    }

    /// Re-pack the array at a wider bit-width, preserving every
    /// entry's value. No-op if `new_bits == self.bits_per_entry`.
    pub fn rebit(&mut self, new_bits: u8) {
        assert!(new_bits >= self.bits_per_entry, "rebit only widens");
        if new_bits == self.bits_per_entry {
            return;
        }
        let mut next = PackedBitArray::zeroed(new_bits, self.len);
        for i in 0..self.len {
            next.set(i, self.get(i));
        }
        *self = next;
    }

    /// Raw word view, for callers writing the Anvil layout.
    #[must_use]
    pub fn words(&self) -> &[u64] {
        &self.data
    }
}

fn entries_per_word(bits_per_entry: u8) -> usize {
    (64 / bits_per_entry as usize).max(1)
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const AIR: BlockStateId = BlockStateId(0);
    const STONE: BlockStateId = BlockStateId(1);

    // ----- PackedBitArray -----

    #[test]
    fn packed_round_trip_4bit() {
        let mut a = PackedBitArray::zeroed(4, 100);
        for i in 0..100 {
            a.set(i, (i % 16) as u32);
        }
        for i in 0..100 {
            assert_eq!(a.get(i), (i % 16) as u32);
        }
    }

    #[test]
    fn packed_word_boundary() {
        // 5 bits per entry, 12 entries per word — entry 12 starts a
        // fresh word. Confirm the boundary is respected.
        let mut a = PackedBitArray::zeroed(5, 30);
        for i in 0..30 {
            a.set(i, (i as u32) & 0x1F);
        }
        for i in 0..30 {
            assert_eq!(a.get(i), (i as u32) & 0x1F);
        }
        // Top 4 bits of each word are padding for 5-bit packing.
        for w in a.words() {
            assert_eq!(w >> 60, 0);
        }
    }

    #[test]
    fn packed_rebit_preserves_values() {
        let mut a = PackedBitArray::zeroed(4, 50);
        for i in 0..50 {
            a.set(i, (i % 16) as u32);
        }
        a.rebit(8);
        assert_eq!(a.bits_per_entry(), 8);
        for i in 0..50 {
            assert_eq!(a.get(i), (i % 16) as u32);
        }
    }

    // ----- ChunkSection -----

    #[test]
    fn filled_air_is_empty() {
        let s = ChunkSection::filled(AIR, AIR);
        assert_eq!(s.non_air_count(), 0);
        assert!(s.is_empty());
        assert_eq!(s.get(0, 0, 0), AIR);
        assert_eq!(s.get(15, 15, 15), AIR);
        assert!(s.palette().is_none());
    }

    #[test]
    fn filled_stone_counts_all_cells() {
        let s = ChunkSection::filled(STONE, AIR);
        assert_eq!(s.non_air_count(), SECTION_VOLUME as u16);
        assert_eq!(s.get(7, 3, 12), STONE);
    }

    #[test]
    fn first_set_promotes_to_indirect_and_tracks_count() {
        let mut s = ChunkSection::filled(AIR, AIR);
        assert_eq!(s.set(1, 2, 3, STONE), AIR);
        assert_eq!(s.get(1, 2, 3), STONE);
        assert_eq!(s.get(0, 0, 0), AIR);
        assert_eq!(s.non_air_count(), 1);
        let palette = s.palette().unwrap();
        assert_eq!(palette, &[AIR, STONE]);
    }

    #[test]
    fn set_back_to_air_decrements_count() {
        let mut s = ChunkSection::filled(AIR, AIR);
        s.set(1, 2, 3, STONE);
        s.set(1, 2, 3, AIR);
        assert_eq!(s.non_air_count(), 0);
        assert_eq!(s.get(1, 2, 3), AIR);
    }

    #[test]
    fn growing_palette_widens_bits() {
        let mut s = ChunkSection::filled(AIR, AIR);
        // 17 distinct non-air states forces the 4-bit packed array
        // to widen to 5 bits (palette grows past 16 entries).
        for i in 1..=17u32 {
            s.set(i as u8 % 16, (i / 16) as u8, 0, BlockStateId(i));
        }
        let palette_len = s.palette().unwrap().len();
        assert!(palette_len >= 17);
        // Spot-check: cells we set are still readable.
        for i in 1..=17u32 {
            let got = s.get(i as u8 % 16, (i / 16) as u8, 0);
            assert_eq!(got, BlockStateId(i));
        }
    }

    #[test]
    fn coordinate_corners_are_addressable() {
        let mut s = ChunkSection::filled(AIR, AIR);
        s.set(0, 0, 0, BlockStateId(10));
        s.set(15, 15, 15, BlockStateId(20));
        s.set(15, 0, 0, BlockStateId(30));
        s.set(0, 15, 0, BlockStateId(40));
        s.set(0, 0, 15, BlockStateId(50));
        assert_eq!(s.get(0, 0, 0), BlockStateId(10));
        assert_eq!(s.get(15, 15, 15), BlockStateId(20));
        assert_eq!(s.get(15, 0, 0), BlockStateId(30));
        assert_eq!(s.get(0, 15, 0), BlockStateId(40));
        assert_eq!(s.get(0, 0, 15), BlockStateId(50));
        assert_eq!(s.non_air_count(), 5);
    }

    #[test]
    fn palette_grows_past_256_entries_without_panicking() {
        // M5.c.3: removed the MAX_INDIRECT_BITS = 8 cap. Inserting
        // 260 distinct synthetic state ids into one section must
        // produce a palette of length ≥ 260 and a packed bit-width
        // wide enough to address it (≥ 9 bits).
        let mut s = ChunkSection::filled(AIR, AIR);
        for i in 1..=260u32 {
            // Walk (x, y) so every set hits a distinct cell.
            let cell = i - 1;
            let x = (cell & 0x0F) as u8;
            let y = ((cell >> 4) & 0x0F) as u8;
            let z = ((cell >> 8) & 0x0F) as u8;
            s.set(x, y, z, BlockStateId(i));
        }
        let palette = s.palette().unwrap();
        assert!(
            palette.len() >= 260,
            "expected ≥ 260 palette entries, got {}",
            palette.len(),
        );
        let bits = s.indices().unwrap().bits_per_entry();
        assert!(
            bits >= 9,
            "expected ≥ 9 bits per entry past the 256-state threshold, got {bits}",
        );
        // Spot-check that all 260 distinct cells round-trip.
        for i in 1..=260u32 {
            let cell = i - 1;
            let x = (cell & 0x0F) as u8;
            let y = ((cell >> 4) & 0x0F) as u8;
            let z = ((cell >> 8) & 0x0F) as u8;
            assert_eq!(s.get(x, y, z), BlockStateId(i));
        }
    }

    /// Random sequence of sets — `non_air_count` must match a linear
    /// scan. Catches off-by-one in the +/- bookkeeping in `set`.
    #[test]
    fn non_air_count_matches_linear_scan() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut s = ChunkSection::filled(AIR, AIR);
        // Deterministic pseudo-random walk: 200 ops.
        let mut h = DefaultHasher::new();
        for op in 0..200u64 {
            op.hash(&mut h);
            let r = h.finish();
            let x = (r & 0x0F) as u8;
            let y = ((r >> 4) & 0x0F) as u8;
            let z = ((r >> 8) & 0x0F) as u8;
            let state = if (r >> 12) & 0x07 == 0 {
                AIR
            } else {
                BlockStateId((r >> 16) as u32 & 0xFF)
            };
            s.set(x, y, z, state);
        }
        let mut linear = 0u16;
        for y in 0..16u8 {
            for z in 0..16u8 {
                for x in 0..16u8 {
                    if s.get(x, y, z) != AIR {
                        linear += 1;
                    }
                }
            }
        }
        assert_eq!(s.non_air_count(), linear);
    }
}
