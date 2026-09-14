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
pub mod settlement_catalog;
pub mod settlement_sites;
pub mod structures;
pub mod terrain;
pub mod vanilla_features;
pub mod village;

#[cfg(test)]
mod mosaic_tests;
#[cfg(test)]
mod settlement_catalog_tests;
#[cfg(test)]
mod settlement_sites_tests;
#[cfg(test)]
mod vanilla_features_tests;
#[cfg(test)]
mod village_piece_tests;
#[cfg(test)]
mod village_plan_source_tests;
#[cfg(test)]
mod village_processors_tests;
#[cfg(test)]
mod village_shapes_tests;
#[cfg(test)]
mod village_solver_tests;
#[cfg(test)]
mod village_tests;

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
pub use settlement_catalog::{
    BlockEntitySeed, BlockEntitySeedKind, Blueprint, BlueprintBlock, BlueprintCatalog,
    BlueprintError, BlueprintInstance, BlueprintPoi, BlueprintStage, Cardinal, CatalogError,
    MAX_BLOCK_ENTITIES_PER_BLUEPRINT, MAX_BLOCKS_PER_BLUEPRINT, MAX_BLUEPRINTS,
    MAX_CATALOG_DECODED_BYTES, MAX_FOOTPRINT_AXIS, MAX_PALETTE_ENTRIES,
    MAX_PLACEMENTS_PER_SETTLEMENT, MAX_POI_PER_BLUEPRINT, MAX_SETTLEMENT_VARIANTS,
    MAX_STAGES_PER_BLUEPRINT, MAX_STREET_CONNECTIONS_PER_BLUEPRINT, PaletteEntry, PlacedBlock,
    PlacedBlockEntity, PlacedPoi, PoiKind, QuarterTurn, StreetConnection, rotate_block_state,
};
pub use settlement_sites::{
    MAX_ROAD_RADIUS_BLOCKS, MAX_ROAD_WAYPOINTS, MAX_SITES_PER_PAGE, ROAD_NEIGHBOUR_CELLS, RoadEdge,
    SITE_CELL_BLOCKS, SettlementSelector, SiteCandidate, SiteError, SiteLayout, SitePlacement,
    SitePoi, SiteVariant,
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
///
/// 22: core generates vanilla villages (`settlement_profile = "vanilla"`) —
/// jigsaw-assembled pieces with their processors, and their
/// `terrain_adaptation = beard_thin` applied as the column-height analogue in
/// [`village::beard`]. A world written by revision 21 has no villages and keeps
/// generating without them; the mismatch is what startup reports.
pub const WORLDGEN_REVISION: u32 = 22;

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
