//! Anvil region-file codec: read and write `.mca` files into our
//! [`crate::chunk::Chunk`] type.
//!
//! Submodules:
//!
//! - [`region`] — sector layout, header parsing, compression. Returns
//!   raw uncompressed NBT bytes per chunk slot.
//! - [`chunk_nbt`] — translation between the parsed NBT compound and
//!   our [`Chunk`] struct.
//!
//! The two layers are split because the region binary is independent
//! of the chunk schema (tools or different worldgen targets could
//! reuse it).

pub mod chunk_nbt;
pub mod region;

pub use chunk_nbt::{ChunkNbtError, chunk_from_nbt, chunk_to_nbt};
pub use region::{
    CHUNKS_PER_REGION_AXIS, ChunkPayload, CompressionType, RegionError, read_region, write_region,
};
