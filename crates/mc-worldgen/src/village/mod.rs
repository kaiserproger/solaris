//! Vanilla village generation (checkpoint B), engine side.
//!
//! This module assembles a village the way vanilla does: one
//! `minecraft:random_spread` structure set picks a chunk, the weighted
//! structure whose biome tag matches the start column is chosen, `JigsawStructure`
//! places a start piece at the `WORLD_SURFACE_WG` height, and
//! `JigsawPlacement` grows the village by connecting jigsaw blocks across
//! template pools. Piece blocks carry their rotation, projection and processor
//! list, and the terrain around rigid pieces is blended with the column-height
//! analogue described in [`beard`].
//!
//! ## What is not placed yet
//!
//! `feature_pool_element` entries — the 13 village decor features the A1/A2
//! executor in [`crate::vanilla_features`] implements — are **not** placed: the
//! growth loop accepts piece elements only, [`solver::feature_elements`] has no
//! non-test caller, and nothing in this module runs the executor. A village
//! therefore generates its buildings, streets and terrain without its decorative
//! features; wiring that lane is the next village item, not an approximation
//! already in place.
//!
//! ## One authority for the data
//!
//! The vanilla specs are `mc-data`'s (`mc_data::village_data`): structure sets,
//! structures, template pools and processor lists. Nothing here re-declares
//! those fields. [`closure`] is the single conversion boundary: it walks the
//! closure from a structure set, loads pieces through
//! [`crate::structures::StructureTemplate`] (jigsaw metadata is engine-side) and
//! carries the `mc-data` specs verbatim. [`placement`] is engine logic over the
//! placement fields the data side owns, and [`beard`] implements the
//! column-height terrain analogue.
//!
//! ## Terrain adaptation
//!
//! `terrain_adaptation = beard_thin` is applied as a **column-height analogue**,
//! not as vanilla's density term; [`beard`] documents the divergence and the
//! upgrade path.

pub mod beard;
pub mod closure;
pub mod piece;
pub mod placement;
pub mod plan_source;
pub mod processors;
pub mod shapes;
pub mod solver;

pub use beard::{BeardColumn, BeardContribution};
pub use closure::{
    ClosureElement, ClosureError, ClosurePool, ClosureStructure, VillageClosure,
    load_village_closure,
};
pub use placement::{
    RandomSpreadPlacement, WeightedStructure, select_structure, set_large_feature_with_salt,
    start_origin,
};
pub use solver::{Junction, PlacedPiece, Rotation, VillageAssembly, assemble_village};
