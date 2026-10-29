//! # mc-worldgen
//!
//! Generation pipeline, biomes, structures.
//!
//! Solaris-owned density-routed terrain with data-fed biomes, caves, aquifers,
//! ores, decorations, and optional structures. The overworld applies aquifers
//! on the CPU after both CPU and GPU solid-grid generation. Generation is
//! deterministic from seed and coordinates, independent of chunk order.
//! Aquifer boundary fluids are not yet scheduled for vanilla postprocessing;
//! Solaris has no generation-time hook that consumes those marks.

pub mod blueprint;
pub mod earth;
pub mod end;
pub mod mosaic;
pub mod nether;
pub mod noise;
mod overworld_structures;
pub mod structures;
pub mod terrain;
pub mod vanilla_climate;
pub mod vanilla_density;
pub mod vanilla_features;
pub mod village;
pub mod world_contract;

#[cfg(test)]
mod blueprint_tests;
#[cfg(test)]
mod mosaic_tests;
#[cfg(test)]
mod vanilla_features_tests;
#[cfg(test)]
mod vanilla_overworld_tests;
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

pub use blueprint::{
    BlockEntitySeed, BlockEntitySeedKind, Blueprint, BlueprintBlock, BlueprintCatalog,
    BlueprintError, BlueprintInstance, BlueprintStage, Cardinal, CatalogError,
    MAX_BLOCK_ENTITIES_PER_BLUEPRINT, MAX_BLOCKS_PER_BLUEPRINT, MAX_BLUEPRINTS,
    MAX_CATALOG_DECODED_BYTES, MAX_FOOTPRINT_AXIS, MAX_PALETTE_ENTRIES, MAX_STAGES_PER_BLUEPRINT,
    MAX_STREET_CONNECTIONS_PER_BLUEPRINT, PaletteEntry, PlacedBlock, PlacedBlockEntity,
    QuarterTurn, StreetConnection, rotate_block_state,
};
pub use end::{
    END_HEIGHT, END_ISLAND_AMPLITUDE, END_ISLAND_BASE_Y, END_ISLAND_FALLOFF, END_ISLAND_RADIUS,
    END_MIN_Y, END_NOISE_SCALE, END_PILLAR_COUNT, END_PILLAR_HALF_WIDTH, END_PILLAR_HEIGHT_SPAN,
    END_PILLAR_MIN_HEIGHT, END_PILLAR_RADIUS, EndGenerator, EndGeneratorError, end_geometry,
};
pub use mosaic::{
    MosaicConfig, MosaicError, MosaicImages, render_earth_mosaic, render_mosaic, write_mosaic,
};
pub use nether::{
    NETHER_CEILING_Y, NETHER_HEIGHT, NETHER_LAVA_LEVEL, NETHER_MIN_Y, NetherGenerator,
    NetherGeneratorError, nether_geometry,
};
pub use structures::{StructureError, StructureTemplate, TemplateBlock, TemplateChest};
pub use terrain::{
    BiomeRules, BiomeScope, ChunkSolids, ClayRule, OreRule, OreRules, OreRulesError, OreSpacing,
    SolidGrid, SpawnLocation, TerrainDiagnosticSample, TerrainGenerator, TerrainGeneratorError,
    TreeRule, WorldgenMode, YRange,
};
#[cfg(feature = "worldgen-counters")]
pub use terrain::{HeightContextStats, height_context_stats, reset_height_context_stats};

/// Changes whenever Solaris intentionally changes newly generated terrain.
///
/// 22: core generates vanilla jigsaw villages —
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
///
/// 25: vanilla overworld uses the local 26.1.2 noise router and expanded
/// multi-noise biome preset; village gates sample the actual 3D start stub.
/// Existing analogue worlds must not grow newly routed vanilla chunks.
///
/// 26: ore feature size controls ellipsoid geometry rather than a guaranteed
/// block count; revision-25 worlds must not mix the inflated ore distribution
/// with newly generated vanilla-like veins.
///
/// 27: village playability on slopes — rigid pieces clear the terrain inside
/// their template-air cells, `terrain_matching` streets are written before the
/// rigid pieces, and a piece's water source open to the side or below is
/// replaced by farmland. Revision-26 villages keep hillside soil in their houses
/// and farm springs over their streets.
///
/// 28: native vanilla density and noise evaluation use fp32 after precise
/// integer-cell/fraction splitting. Old worlds require the existing explicit
/// offline worldgen upgrade; that preserves their chunks and accepts boundary
/// seams. The opaque terrain epoch is not a numeric-version selector.
/// SLOD v1 remains the approximate fp64 distant-terrain program.
///
/// 29: persist native structure starts and generate Overworld structures,
/// modern jigsaw families, Nether landscapes/resources, and End outer cities.
///
/// 30: correct the deepslate vertical gradient to Y 0..8 with linear probability.
/// Existing chunks are retained by the explicit offline worldgen upgrade.
///
/// 31: populate overworld density caves with noise-based aquifers, including
/// water, lava and solid pressure barriers on both CPU and GPU density paths;
/// place shipwrecks and ocean ruins on the seabed, cap the monument below sea
/// level, and generate mineshaft loot as chest minecarts.
pub const WORLDGEN_REVISION: u32 = 31;

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
