//! # mc-world
//!
//! Block states, chunk format, world storage.
//!
//! Part of the Solaris engine.

pub mod anvil;
pub mod block;
pub mod chunk;
pub mod light;
pub mod resident;
pub mod section;
pub mod storage;
pub mod wire;

pub use block::{Block, BlockRegistry, BlockState, BlockStateId, RegistryError};
pub use chunk::{
    BIOME_DIM, BIOME_VOLUME, BiomeSection, BlockMutationToken, BlockPos, ChestBlockEntity, Chunk,
    ChunkGenerator, ChunkGeometry, ChunkLightSourceToken, ChunkPos, FurnaceBlockEntity,
    FurnaceSlot, HEIGHTMAP_BITS, HEIGHTMAP_LEN, Heightmap, HopperBlockEntity, LIGHT_LAYER_BYTES,
    MAX_Y, MIN_SECTION_Y, MIN_Y, OVERWORLD_GEOMETRY, SECTION_COUNT, ScheduledBlockTick,
    ScheduledFluidTick, SectionLight, SettlementInhabitantMarker, SettlementVacantHomeMarker,
};
pub use resident::{
    JournalStampResult, ResidentAppliedBlockEdit, ResidentBlockEdit, ResidentBlockEditBatchResult,
    ResidentBlockEntityChange, ResidentBlockMutation, ResidentBlockPrecondition,
    ResidentChestCommitResult, ResidentFluidTickPlan, ResidentFurnaceCommitResult,
    ResidentFurnaceTickCommitResult, ResidentHopperTransferCommitResult,
    ResidentHopperTransferPlan, ResidentOpaqueBlockEntityCommitResult,
    ResidentScheduledBlockTickPlan, WorldMutationView,
};
pub use section::{ChunkSection, PackedBitArray, SECTION_DIM, SECTION_VOLUME};
pub use storage::{
    ChunkDiskLoadPlan, ChunkPrepareSource, ChunkSnapshot, ChunkSnapshotPlan, ChunkSourceView,
    DirtyFlushPlan, ScheduledTickView, WorldError, WorldReadSnapshot, WorldReadView, WorldSpawn,
    WorldStorage,
};

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod inhabited_time_tests;
