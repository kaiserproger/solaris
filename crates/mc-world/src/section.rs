//! Chunk section: a 16×16×16 cube of block states with a small,
//! growable palette.
//!
//! RAM storage keeps palette indices compact:
//!
//! - **Single** — the whole section is one state. No index array, no
//!   palette overhead. Common for the void above the build height and
//!   for the stone-only mid-overworld layers.
//! - **Indirect** — a `Vec<BlockStateId>` palette and a packed
//!   bit-array of indices. `bits_per_entry` grows naturally with
//!   palette size, starting at one bit for two states. Disk and wire
//!   encoders pad block indices to vanilla's four-bit minimum.
//!   Sections with more than 256 distinct states stay indirect on
//!   disk but the wire encoder converts to direct format at emit time.
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

use std::sync::Arc;

use crate::block::BlockStateId;

pub const SECTION_DIM: usize = 16;
pub const SECTION_VOLUME: usize = SECTION_DIM * SECTION_DIM * SECTION_DIM;

const MIN_INDIRECT_BITS: u8 = 1;

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
    Indirect(Arc<IndirectStorage>),
}

/// Unchanged sections stay shared when a resident chunk snapshot is cloned.
/// A block edit detaches only this section's palette and packed indices.
#[derive(Debug, Clone)]
struct IndirectStorage {
    palette: Vec<BlockStateId>,
    indices: PackedBitArray,
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
            BlockStorage::Indirect(storage) => {
                let p = storage.indices.get(idx) as usize;
                storage.palette[p]
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
            BlockStorage::Indirect(storage) => {
                let prev_p = storage.indices.get(idx) as usize;
                let prev_state = storage.palette[prev_p];
                if prev_state == state {
                    return prev_state;
                }
                let IndirectStorage { palette, indices } = Arc::make_mut(storage);
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
    /// every entry is < `palette.len()`. Indices are compacted to the
    /// palette's required width on admission; one-entry palettes use
    /// allocation-free Single storage. `non_air_count` scans the indices.
    #[must_use]
    pub fn from_indirect(
        palette: Vec<BlockStateId>,
        mut indices: PackedBitArray,
        air: BlockStateId,
    ) -> Self {
        assert_eq!(indices.len(), SECTION_VOLUME);
        assert!(!palette.is_empty());
        if palette.len() == 1 {
            return Self::filled(palette[0], air);
        }
        let bits = bits_for_palette(palette.len());
        if indices.bits_per_entry() != bits {
            let words = indices.words_at_bits(bits).collect();
            indices = PackedBitArray::from_words(bits, SECTION_VOLUME, words);
        }
        let non_air_count = (0..SECTION_VOLUME)
            .filter(|&i| {
                let p = indices.get(i) as usize;
                palette[p] != air
            })
            .count() as u16;
        Self {
            storage: BlockStorage::Indirect(Arc::new(IndirectStorage { palette, indices })),
            non_air_count,
            air,
        }
    }

    /// Access RAM-packed indices. Block codecs must apply vanilla's minimum width.
    #[must_use]
    pub fn indices(&self) -> Option<&PackedBitArray> {
        match &self.storage {
            BlockStorage::Single(_) => None,
            BlockStorage::Indirect(storage) => Some(&storage.indices),
        }
    }

    /// Identity, owner count and full heap charge for deduplicated snapshot accounting.
    pub(crate) fn shared_heap_allocation(&self) -> Option<(usize, usize, usize)> {
        match &self.storage {
            BlockStorage::Single(_) => None,
            BlockStorage::Indirect(storage) => Some((
                Arc::as_ptr(storage) as usize,
                Arc::strong_count(storage),
                self.estimated_heap_bytes(),
            )),
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
            BlockStorage::Indirect(storage) => Some(&storage.palette),
        }
    }

    #[must_use]
    pub(crate) fn estimated_heap_bytes(&self) -> usize {
        match &self.storage {
            BlockStorage::Single(_) => 0,
            BlockStorage::Indirect(storage) => std::mem::size_of::<IndirectStorage>()
                .saturating_add(2 * std::mem::size_of::<usize>())
                .saturating_add(
                    storage
                        .palette
                        .capacity()
                        .saturating_mul(std::mem::size_of::<BlockStateId>()),
                )
                .saturating_add(storage.indices.estimated_heap_bytes()),
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
    BlockStorage::Indirect(Arc::new(IndirectStorage { palette, indices }))
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
/// Padding bits at the top of each word are unused. Disk and wire
/// codecs may need to widen RAM indices to their minimum entry width.
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

    /// Stream words at a codec's required width without a temporary packed array.
    /// Matching widths reuse the stored words, including their padding.
    pub(crate) fn words_at_bits(&self, bits: u8) -> impl ExactSizeIterator<Item = u64> + '_ {
        assert!((1..=32).contains(&bits));
        let epw = entries_per_word(bits);
        (0..self.len.div_ceil(epw)).map(move |word| {
            if bits == self.bits_per_entry {
                return self.data[word];
            }
            let start = word * epw;
            let end = (start + epw).min(self.len);
            let mut packed = 0;
            for idx in start..end {
                let value = u64::from(self.get(idx));
                assert!(value < (1u64 << bits), "palette index exceeds target width");
                packed |= value << ((idx - start) * bits as usize);
            }
            packed
        })
    }

    #[must_use]
    pub(crate) fn estimated_heap_bytes(&self) -> usize {
        self.data
            .capacity()
            .saturating_mul(std::mem::size_of::<u64>())
    }
}

fn entries_per_word(bits_per_entry: u8) -> usize {
    (64 / bits_per_entry as usize).max(1)
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
#[path = "section_tests.rs"]
mod tests;
