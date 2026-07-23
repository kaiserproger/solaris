//! # mc-worldgen
//!
//! Generation pipeline, biomes, structures.
//!
//! Solaris-owned density-routed terrain with data-fed biomes, caves, ores,
//! decorations, and optional structures. Generation is deterministic from
//! seed and world coordinates and does not depend on chunk generation order.

pub mod noise;
pub mod structures;
pub mod terrain;

pub use structures::{
    StructureError, StructureRules, StructureTemplate, TemplateBlock, TemplateChest,
};
pub use terrain::{
    BiomeRules, BiomeScope, OreRule, OreRules, OreRulesError, OreSpacing, TellusWorldgenSettings,
    TerrainGenerator, TerrainGeneratorError, WorldgenMode, YRange,
};

/// Changes whenever Solaris intentionally changes newly generated terrain.
pub const WORLDGEN_REVISION: u32 = 9;

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
