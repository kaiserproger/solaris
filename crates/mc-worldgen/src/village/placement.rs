//! Structure placement: which chunk a structure set can start in, and the
//! `setLargeFeatureSeed` derivation the solver and this placement share.
//!
//! `RandomSpreadStructurePlacement.getPotentialStructureChunk` (26.1.2) is:
//! `gridX = floorDiv(sourceX, spacing)`, the same for Z, a
//! `WorldgenRandom(LegacyRandomSource(0))` re-seeded through
//! `setLargeFeatureWithSalt(seed, gridX, gridZ, salt)` —
//! `seed + gridX * 341873128712 + gridZ * 132897987541 + salt` — then one
//! `spreadType.evaluate(random, spacing - separation)` draw per axis
//! (`LINEAR` is `nextInt`, `TRIANGULAR` is `(nextInt + nextInt) / 2`), and the
//! candidate chunk is `grid * spacing + spread`. A chunk generates the set when
//! that candidate is the chunk itself.
//!
//! Which entry of the set actually starts there, and how the village is grown,
//! is [`super::solver`]'s: both run on `setLargeFeatureSeed`, a different
//! derivation from the placement's own salt-seeded source.

use mc_world::BlockPos;

use crate::vanilla_features::{LegacyRandom, RandomSource};

/// `minecraft:random_spread`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RandomSpreadPlacement {
    pub spacing: i32,
    pub separation: i32,
    pub salt: i64,
    pub triangular: bool,
}

impl RandomSpreadPlacement {
    /// `StructurePlacement.getPotentialStructureChunk`.
    #[must_use]
    pub fn potential_chunk(&self, seed: i64, source_x: i32, source_z: i32) -> (i32, i32) {
        let grid_x = source_x.div_euclid(self.spacing);
        let grid_z = source_z.div_euclid(self.spacing);
        let mut random =
            LegacyRandom::new(set_large_feature_with_salt(seed, grid_x, grid_z, self.salt));
        let limit = self.spacing - self.separation;
        let spread_x = self.evaluate(&mut random, limit);
        let spread_z = self.evaluate(&mut random, limit);
        (
            grid_x * self.spacing + spread_x,
            grid_z * self.spacing + spread_z,
        )
    }

    /// `StructurePlacement.isPlacementChunk`.
    #[must_use]
    pub fn is_placement_chunk(&self, seed: i64, chunk_x: i32, chunk_z: i32) -> bool {
        self.potential_chunk(seed, chunk_x, chunk_z) == (chunk_x, chunk_z)
    }

    /// `RandomSpreadType.evaluate`.
    fn evaluate(&self, random: &mut LegacyRandom, limit: i32) -> i32 {
        if self.triangular {
            (random.next_int_bounded(limit) + random.next_int_bounded(limit)) / 2
        } else {
            random.next_int_bounded(limit)
        }
    }
}

/// `WorldgenRandom.setLargeFeatureWithSalt`:
/// `result = x * 341873128712 + z * 132897987541 + seed + blend`, then re-seed.
#[must_use]
pub fn set_large_feature_with_salt(seed: i64, x: i32, z: i32, salt: i64) -> i64 {
    i64::from(x)
        .wrapping_mul(341_873_128_712)
        .wrapping_add(i64::from(z).wrapping_mul(132_897_987_541))
        .wrapping_add(seed)
        .wrapping_add(salt)
}

/// `WorldgenRandom.setLargeFeatureSeed`:
/// `seed` seeds a legacy source, two `nextLong`s scale the chunk coordinates,
/// and `result = chunkX * xScale ^ chunkZ * zScale ^ seed` re-seeds it.
///
/// This — not [`set_large_feature_with_salt`] — is the seed
/// `ChunkGenerator.createStructures` selects a structure set entry with and
/// `Structure.GenerationContext` grows the structure on.
#[must_use]
pub fn set_large_feature_seed(seed: i64, chunk_x: i32, chunk_z: i32) -> i64 {
    let mut random = LegacyRandom::new(seed);
    let x_scale = random.next_long();
    let z_scale = random.next_long();
    i64::from(chunk_x).wrapping_mul(x_scale) ^ i64::from(chunk_z).wrapping_mul(z_scale) ^ seed
}

/// `WorldGenerationContext`-free start column: vanilla's
/// `project_start_to_heightmap = WORLD_SURFACE_WG` puts the start piece's Y at
/// the first free height of the start chunk's centre column.
#[must_use]
pub fn start_origin(chunk_x: i32, chunk_z: i32, start_height: i32, free_height: i32) -> BlockPos {
    BlockPos {
        x: chunk_x * 16,
        y: if start_height == 0 {
            free_height
        } else {
            start_height
        },
        z: chunk_z * 16,
    }
}
