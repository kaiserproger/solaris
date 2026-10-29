//! Which blocks of a chunk the terrain density makes solid (`density > 0`).
//! Generation reads density only through that sign, so the GPU worldgen
//! experiment returns these bits instead of the densities themselves.
use std::collections::HashMap;
use std::sync::Arc;

use mc_world::ChunkPos;

/// Solid bits of one chunk, column-major: bit `offset` of column
/// `z * 16 + x` (offset from the density's min y) is word
/// `column * words_per_column + offset / 32`, bit `offset % 32`.
#[derive(Clone)]
pub struct ChunkSolids {
    words: Box<[u32]>,
    height: usize,
}

impl ChunkSolids {
    /// Words per column for a density `height`.
    #[must_use]
    pub const fn words_per_column(height: usize) -> usize {
        height.div_ceil(32)
    }

    /// From [`crate::vanilla_density::VanillaDensity::fill_density_chunk`]
    /// output (`(y - min_y) * 256 + z * 16 + x`).
    #[must_use]
    pub fn from_densities(values: &[f32], height: usize) -> Self {
        assert_eq!(values.len(), height * 256);
        let stride = Self::words_per_column(height);
        let mut words = vec![0u32; 256 * stride].into_boxed_slice();
        for (offset, layer) in values.as_chunks::<256>().0.iter().enumerate() {
            let (word, bit) = (offset / 32, offset % 32);
            for (column, value) in layer.iter().enumerate() {
                words[column * stride + word] |= u32::from(*value > 0.0) << bit;
            }
        }
        Self { words, height }
    }

    /// From words in this type's layout (the GPU's output).
    #[must_use]
    pub fn from_words(words: Box<[u32]>, height: usize) -> Self {
        assert_eq!(words.len(), 256 * Self::words_per_column(height));
        Self { words, height }
    }

    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }

    #[must_use]
    pub fn solid(&self, column: usize, offset: usize) -> bool {
        debug_assert!(offset < self.height);
        let word = self.words[column * Self::words_per_column(self.height) + offset / 32];
        word >> (offset % 32) & 1 != 0
    }

    /// Highest solid offset of `column` below `end` (exclusive).
    #[must_use]
    pub fn top_below(&self, column: usize, end: usize) -> Option<usize> {
        let end = end.min(self.height);
        let stride = Self::words_per_column(self.height);
        let column = &self.words[column * stride..][..stride];
        let mut word = end / 32;
        let mut mask = if end.is_multiple_of(32) {
            word = word.checked_sub(1)?;
            u32::MAX
        } else {
            (1u32 << (end % 32)) - 1
        };
        loop {
            let bits = column[word] & mask;
            if bits != 0 {
                return Some(word * 32 + 31 - bits.leading_zeros() as usize);
            }
            word = word.checked_sub(1)?;
            mask = u32::MAX;
        }
    }
}

/// [`ChunkSolids`] by chunk, computed elsewhere (the GPU worldgen
/// experiment). Generation reads its own chunk and every held neighbour its
/// ore halo and surfaces reach instead of evaluating density.
#[derive(Default)]
pub struct SolidGrid {
    chunks: HashMap<(i32, i32), Arc<ChunkSolids>>,
}

impl SolidGrid {
    pub fn insert(&mut self, pos: ChunkPos, solids: Arc<ChunkSolids>) {
        self.chunks.insert((pos.x, pos.z), solids);
    }

    #[must_use]
    pub fn chunk(&self, pos: ChunkPos) -> Option<&ChunkSolids> {
        self.chunks.get(&(pos.x, pos.z)).map(Arc::as_ref)
    }

    fn column(&self, world_x: i32, world_z: i32) -> Option<(&ChunkSolids, usize)> {
        let solids = self.chunk(ChunkPos {
            x: world_x.div_euclid(16),
            z: world_z.div_euclid(16),
        })?;
        Some((
            solids,
            (world_z.rem_euclid(16) * 16 + world_x.rem_euclid(16)) as usize,
        ))
    }

    /// Whether `(x, y, z)` is solid; `None` outside held chunks or the
    /// density's height range starting at `min_y`.
    pub(super) fn solid(&self, world_x: i32, y: i32, world_z: i32, min_y: i32) -> Option<bool> {
        let (solids, column) = self.column(world_x, world_z)?;
        let offset = usize::try_from(y.checked_sub(min_y)?).ok()?;
        (offset < solids.height).then(|| solids.solid(column, offset))
    }

    /// Top solid y, as `SurfaceHeightSampler::surface_height` answers it.
    pub(super) fn surface_height(&self, world_x: i32, world_z: i32, min_y: i32) -> Option<i32> {
        let (solids, column) = self.column(world_x, world_z)?;
        Some(
            solids
                .top_below(column, solids.height)
                .map_or(min_y - 1, |offset| min_y + offset as i32),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_follow_density_signs_and_top_finds_the_highest_below_a_limit() {
        let height = 70;
        let mut values = vec![-1.0f32; height * 256];
        let set = |values: &mut Vec<f32>, column: usize, offset: usize, v: f32| {
            values[offset * 256 + column] = v;
        };
        for offset in [0, 31, 32, 63, 64, 69] {
            set(&mut values, 5, offset, 0.5);
        }
        set(&mut values, 7, 40, 0.0);
        set(&mut values, 7, 41, f32::MIN_POSITIVE);
        let solids = ChunkSolids::from_densities(&values, height);
        for offset in 0..height {
            assert_eq!(
                solids.solid(5, offset),
                [0, 31, 32, 63, 64, 69].contains(&offset),
                "{offset}"
            );
        }
        assert!(!solids.solid(7, 40), "zero density is not solid");
        assert!(solids.solid(7, 41));
        assert_eq!(solids.top_below(5, height), Some(69));
        assert_eq!(solids.top_below(5, 69), Some(64));
        assert_eq!(solids.top_below(5, 64), Some(63));
        assert_eq!(solids.top_below(5, 32), Some(31));
        assert_eq!(solids.top_below(5, 31), Some(0));
        assert_eq!(solids.top_below(5, 0), None);
        assert_eq!(solids.top_below(6, height), None);
    }
}
