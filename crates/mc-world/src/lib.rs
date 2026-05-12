//! # mc-world
//!
//! Block states, chunk format, world storage.
//!
//! Part of the Solaris engine.

pub mod anvil;
pub mod block;
pub mod chunk;
pub mod section;

pub use block::{Block, BlockRegistry, BlockState, BlockStateId, RegistryError};
pub use chunk::{
    BIOME_DIM, BIOME_VOLUME, BiomeSection, BlockPos, Chunk, ChunkPos, HEIGHTMAP_BITS,
    HEIGHTMAP_LEN, Heightmap, MAX_Y, MIN_SECTION_Y, MIN_Y, SECTION_COUNT,
};
pub use section::{ChunkSection, PackedBitArray, SECTION_DIM, SECTION_VOLUME};

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
