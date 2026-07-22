//! Deterministic overworld routing.
//!
//! The router is the only source of terrain height, climate, rivers, and cave
//! membership. Later stages may choose blocks, but may not reshape terrain.

mod router;

pub(in crate::terrain) use router::{OverworldRouter, TerrainSample};
