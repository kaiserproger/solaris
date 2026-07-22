//! Deterministic overworld density routing.
//!
//! This module owns terrain shape. Features consume its output but may not
//! invent a second height, climate, river, or cave authority.

mod density;

pub(in crate::terrain) use density::{DensityRouter, TerrainSample};
