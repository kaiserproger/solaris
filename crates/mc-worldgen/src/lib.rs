//! # mc-worldgen
//!
//! Generation pipeline, biomes, structures.
//!
//! Solaris-owned hash-noise terrain with data-fed biomes, caves, ores,
//! decorations, and optional structure markers. This crate does not
//! implement Mojang's vanilla worldgen algorithms.

pub mod noise;
pub mod structures;
pub mod terrain;

pub use structures::{StructureError, StructureRules, StructureTemplate, TemplateBlock};
pub use terrain::{
    BiomeRules, BiomeScope, OreRule, OreRules, OreSpacing, TellusWorldgenSettings,
    TerrainGenerator, TerrainGeneratorError, WorldgenMode, YRange,
};

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
