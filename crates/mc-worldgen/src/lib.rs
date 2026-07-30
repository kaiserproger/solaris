//! # mc-worldgen
//!
//! Generation pipeline, biomes, structures.
//!
//! Solaris-owned density-routed terrain with data-fed biomes, caves, ores,
//! decorations, and optional structures. Generation is deterministic from
//! seed and world coordinates and does not depend on chunk generation order.

pub mod mosaic;
pub mod noise;
pub mod structures;
pub mod terrain;

#[cfg(test)]
mod mosaic_tests;

pub use mosaic::{MosaicConfig, MosaicError, MosaicImages, render_mosaic, write_mosaic};
pub use structures::{
    PlainsVillagePrototypePart, StructureError, StructureInhabitant, StructureRules,
    StructureTemplate, TemplateBlock, TemplateChest,
};
pub use terrain::{
    BiomeRules, BiomeScope, OreRule, OreRules, OreRulesError, OreSpacing, SpawnLocation,
    TellusWorldgenSettings, TerrainDiagnosticSample, TerrainGenerator, TerrainGeneratorError,
    WorldgenMode, YRange,
};

/// Changes whenever Solaris intentionally changes newly generated terrain.
pub const WORLDGEN_REVISION: u32 = 11;

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
