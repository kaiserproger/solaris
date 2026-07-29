//! Deterministic overworld routing.
//!
//! Landforms and caves are separate coordinate-derived stages. Later stages
//! may choose blocks and decorations, but may not reshape terrain.

use mc_world::chunk::ChunkGeometry;

use super::{SEA_LEVEL, TellusWorldgenSettings, WorldgenMode};

mod caves;
mod drainage;
mod landforms;

const CAVE_SURFACE_CLEARANCE: i32 = 32;

#[derive(Clone, Copy, Debug)]
pub(in crate::terrain) struct TerrainSample {
    pub(in crate::terrain) surface_y: i32,
    pub(in crate::terrain) continentalness: f64,
    pub(in crate::terrain) ridges: f64,
    /// Distance from the nearest river centre line.
    pub(in crate::terrain) river: f64,
    pub(in crate::terrain) temperature: f64,
    pub(in crate::terrain) moisture: f64,
}

/// Stateless world-coordinate authority for newly generated overworld chunks.
#[derive(Clone, Copy, Debug)]
pub(in crate::terrain) struct OverworldRouter {
    seed: i64,
    geometry: ChunkGeometry,
    mode: WorldgenMode,
}

impl OverworldRouter {
    pub(in crate::terrain) const fn new(
        seed: i64,
        geometry: ChunkGeometry,
        mode: WorldgenMode,
    ) -> Self {
        Self {
            seed,
            geometry,
            mode,
        }
    }

    pub(in crate::terrain) fn sample(self, block_x: i32, block_z: i32) -> TerrainSample {
        landforms::sample(self, block_x, block_z)
    }

    pub(in crate::terrain) fn is_cave(self, x: i32, y: i32, z: i32, surface_y: i32) -> bool {
        caves::contains(self, x, y, z, surface_y)
    }

    pub(in crate::terrain) fn raw_cave(self, x: i32, y: i32, z: i32) -> bool {
        caves::raw(self, x, y, z)
    }

    #[cfg(test)]
    fn temperature(
        self,
        x: f64,
        height: f64,
        z: f64,
        _settings: Option<TellusWorldgenSettings>,
    ) -> f64 {
        landforms::temperature(self, x, height, z)
    }

    fn settings(self) -> Option<TellusWorldgenSettings> {
        match self.mode {
            WorldgenMode::VanillaLike => None,
            WorldgenMode::TellusLike(settings) => Some(settings),
        }
    }

    fn sea_level(self) -> i32 {
        self.settings()
            .map_or(SEA_LEVEL, |settings| settings.sea_level)
    }

    fn clamp_height(self, height: f64) -> i32 {
        let min = self.geometry.min_y().saturating_add(2);
        let max = self.geometry.max_y().saturating_sub(2).min(300).max(min);
        height.round().clamp(f64::from(min), f64::from(max)) as i32
    }
}

#[cfg(test)]
#[path = "router_tests.rs"]
mod tests;
