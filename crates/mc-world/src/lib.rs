//! # mc-world
//!
//! Block states, chunk format, world storage.
//!
//! Part of the Solaris engine.

pub mod anvil;
pub mod block;
pub mod chunk;
pub mod light;
pub mod section;
pub mod storage;
pub mod wire;

pub use block::{Block, BlockRegistry, BlockState, BlockStateId, RegistryError};
pub use chunk::{
    BIOME_DIM, BIOME_VOLUME, BiomeSection, BlockPos, ChestBlockEntity, Chunk, ChunkGenerator,
    ChunkPos, FurnaceBlockEntity, FurnaceSlot, HEIGHTMAP_BITS, HEIGHTMAP_LEN, Heightmap,
    HopperBlockEntity, LIGHT_LAYER_BYTES, MAX_Y, MIN_SECTION_Y, MIN_Y, SECTION_COUNT,
    ScheduledBlockTick, ScheduledFluidTick, SectionLight,
};
pub use section::{ChunkSection, PackedBitArray, SECTION_DIM, SECTION_VOLUME};
pub use storage::{ChunkDiskLoadPlan, ChunkSnapshot, ChunkSnapshotPlan, WorldError, WorldStorage};

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
