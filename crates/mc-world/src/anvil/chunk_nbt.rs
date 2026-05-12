//! Chunk ↔ NBT translation (Java-standard named-root NBT, as Anvil
//! stores it).
//!
//! Filled out below the region binary layer in a follow-on commit.

use thiserror::Error;

use crate::chunk::Chunk;

#[derive(Debug, Error)]
pub enum ChunkNbtError {
    #[error("not yet implemented")]
    Unimplemented,
}

/// Build a [`Chunk`] from a parsed NBT compound. **Stub.**
pub fn chunk_from_nbt(_nbt: &mc_nbt::Tag) -> Result<Chunk, ChunkNbtError> {
    Err(ChunkNbtError::Unimplemented)
}

/// Encode a [`Chunk`] to an NBT compound. **Stub.**
pub fn chunk_to_nbt(_chunk: &Chunk) -> Result<mc_nbt::Tag, ChunkNbtError> {
    Err(ChunkNbtError::Unimplemented)
}
