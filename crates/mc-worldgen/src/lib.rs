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
///
/// 23: the village assembly is rebuilt on the 26.1.2 bodies end to end — the
/// structure selection re-draws from the set the way
/// `ChunkGenerator.createStructures` does and the growth runs on
/// `setLargeFeatureSeed`, source jigsaws are rotated before they attach, the
/// target pool's and fallback's candidate lists are shuffled, the
/// `use_expansion_hack` box and the shared free-space shape are applied, the
/// queue is priority-ordered, the biome gate is decided at the stub position —
/// and the closure's `feature_pool_element` entries are placed as decor through
/// the data-driven feature executor on the `FEATURES` step's own random
/// ([`village::decor`]). Every change moves village geometry or adds blocks, so
/// revision-22 worlds are refused with the fresh-`world_dir` message.
///
/// 24: a village is populated — every piece template's `minecraft:villager` is
/// placed as a chunk inhabitant marker (`StructureTemplate.placeEntities`'s
/// transform of the template's entity list), which is what the runtime spawns
/// villagers from. Revision-23 chunks carry no markers, so a village would stay
/// empty when they are reloaded; the mismatch is refused with the
/// fresh-`world_dir` message instead.
pub const WORLDGEN_REVISION: u32 = 24;

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
