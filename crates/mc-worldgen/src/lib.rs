//! # mc-worldgen
//!
//! Generation pipeline, biomes, structures.
//!
//! M7 baseline: hash-noise terrain, single biome, no caves / ores /
//! structures. The terrain function is Solaris's own — no vanilla
//! algorithm. See `docs/milestones/M7.md` for scope decisions.

pub mod noise;
pub mod terrain;

pub use terrain::{ChunkGenerator, TerrainGenerator};

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
