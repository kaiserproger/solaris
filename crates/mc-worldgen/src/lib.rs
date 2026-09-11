//! # mc-worldgen
//!
//! Generation pipeline, biomes, structures.
//!
//! Solaris-owned density-routed terrain with data-fed biomes, caves, ores,
//! decorations, and optional structures. Generation is deterministic from
//! seed and world coordinates and does not depend on chunk generation order.

pub mod end;
pub mod mosaic;
pub mod nether;
pub mod noise;
pub mod structures;
pub mod terrain;

#[cfg(test)]
mod mosaic_tests;

pub use end::{
    END_HEIGHT, END_ISLAND_AMPLITUDE, END_ISLAND_BASE_Y, END_ISLAND_FALLOFF, END_ISLAND_RADIUS,
    END_MIN_Y, END_NOISE_SCALE, END_PILLAR_COUNT, END_PILLAR_HALF_WIDTH, END_PILLAR_HEIGHT_SPAN,
    END_PILLAR_MIN_HEIGHT, END_PILLAR_RADIUS, EndGenerator, EndGeneratorError, end_geometry,
};
pub use mosaic::{MosaicConfig, MosaicError, MosaicImages, render_mosaic, write_mosaic};
pub use nether::{
    NETHER_CEILING_Y, NETHER_HEIGHT, NETHER_LAVA_LEVEL, NETHER_MIN_Y, NetherGenerator,
    NetherGeneratorError, nether_geometry,
};
pub use structures::{
    PlainsVillagePrototypePart, StructureError, StructureInhabitant, StructureRules,
    StructureTemplate, TemplateBlock, TemplateChest,
};
pub use terrain::{
    BiomeRules, BiomeScope, ClayRule, OreRule, OreRules, OreRulesError, OreSpacing, SpawnLocation,
    TellusWorldgenSettings, TerrainDiagnosticSample, TerrainGenerator, TerrainGeneratorError,
    TreeRule, WorldgenMode, YRange,
};

/// Changes whenever Solaris intentionally changes newly generated terrain.
pub const WORLDGEN_REVISION: u32 = 20;

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
