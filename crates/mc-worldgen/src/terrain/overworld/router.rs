use mc_world::chunk::ChunkGeometry;

use crate::noise::{fbm_2d, fbm_3d};

use super::super::{SEA_LEVEL, TellusWorldgenSettings, WorldgenMode};

const CONTINENT_SCALE: f64 = 2_800.0;
const CONTINENT_WARP_SCALE: f64 = 1_100.0;
const CONTINENT_WARP_STRENGTH: f64 = 120.0;
const EROSION_SCALE: f64 = 1_400.0;
const UPLAND_SCALE: f64 = 760.0;
const HILL_SCALE: f64 = 310.0;
const DETAIL_SCALE: f64 = 120.0;
const RIDGE_SCALE: f64 = 1_450.0;
const RIVER_SCALE: f64 = 1_650.0;
const RIVER_DETAIL_SCALE: f64 = 520.0;
const SPAWN_LAND_RADIUS: f64 = 384.0;
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
///
/// The router is deliberately layered: continents choose ocean or land,
/// erosion chooses plains or uplands, ridges add mountains only on established
/// land, and rivers carve only low relief. No later layer can turn one column
/// into an unbounded pit.
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
        let settings = self.settings();
        let world_scale = settings
            .map(|value| (value.world_scale_meters_per_block / 30.0).clamp(0.25, 8.0))
            .unwrap_or(1.0);
        let x = f64::from(block_x);
        let z = f64::from(block_z);
        let sea_level = settings.map_or(SEA_LEVEL, |value| value.sea_level);

        let warp_x = fbm_2d(
            x / (CONTINENT_WARP_SCALE * world_scale),
            z / (CONTINENT_WARP_SCALE * world_scale),
            self.seed ^ 0x5752_5058,
            2,
            0.5,
        ) * CONTINENT_WARP_STRENGTH
            * world_scale;
        let warp_z = fbm_2d(
            x / (CONTINENT_WARP_SCALE * world_scale),
            z / (CONTINENT_WARP_SCALE * world_scale),
            self.seed ^ 0x5752_505A,
            2,
            0.5,
        ) * CONTINENT_WARP_STRENGTH
            * world_scale;
        let warped_x = x + warp_x;
        let warped_z = z + warp_z;

        let raw_continent = fbm_2d(
            warped_x / (CONTINENT_SCALE * world_scale),
            warped_z / (CONTINENT_SCALE * world_scale),
            self.seed ^ 0x434F_4E54,
            4,
            0.52,
        );
        let spawn_weight = 1.0 - smootherstep((x.hypot(z) / SPAWN_LAND_RADIUS).clamp(0.0, 1.0));
        let continentalness = raw_continent.max(lerp(raw_continent, 0.30, spawn_weight));
        let land_weight = smootherstep(remap(continentalness, -0.22, 0.08));

        let erosion = normalized(fbm_2d(
            warped_x / (EROSION_SCALE * world_scale),
            warped_z / (EROSION_SCALE * world_scale),
            self.seed ^ 0x4552_4F53,
            4,
            0.52,
        ));
        let upland = fbm_2d(
            warped_x / (UPLAND_SCALE * world_scale),
            warped_z / (UPLAND_SCALE * world_scale),
            self.seed ^ 0x5550_4C44,
            3,
            0.5,
        );
        let hills = fbm_2d(
            warped_x / (HILL_SCALE * world_scale),
            warped_z / (HILL_SCALE * world_scale),
            self.seed ^ 0x4849_4C4C,
            3,
            0.5,
        );
        let detail = fbm_2d(
            x / (DETAIL_SCALE * world_scale),
            z / (DETAIL_SCALE * world_scale),
            self.seed ^ 0x4445_544C,
            2,
            0.45,
        );
        let ridge_noise = fbm_2d(
            warped_x / (RIDGE_SCALE * world_scale),
            warped_z / (RIDGE_SCALE * world_scale),
            self.seed ^ 0x5249_4447,
            4,
            0.5,
        );
        let ridge_shape = (1.0 - ridge_noise.abs()).clamp(0.0, 1.0).powi(3);
        let mountain_mask = smootherstep(remap(continentalness, 0.12, 0.48))
            * smootherstep(remap(1.0 - erosion, 0.34, 0.82))
            * (1.0 - spawn_weight);
        let ridges = ridge_shape * mountain_mask;

        let ocean_scale = settings.map_or(1.0, |value| value.oceanic_height_scale.max(0.0));
        let land_scale = settings.map_or(1.0, |value| value.terrestrial_height_scale.max(0.0));
        let relief_scale = settings.map_or(1.0, |_| 1.15);
        let deep_ocean = smootherstep(remap(-continentalness, 0.08, 0.58));
        let ocean_floor = f64::from(sea_level) - (12.0 + deep_ocean * 34.0) * ocean_scale
            + hills * 2.0 * ocean_scale;
        let rolling_land = f64::from(sea_level)
            + 7.0
            + continentalness.max(0.0) * 18.0 * land_scale
            + upland * (4.0 + (1.0 - erosion) * 7.0) * land_scale * relief_scale
            + hills * 3.0 * land_scale * relief_scale
            + detail * 1.25 * land_scale * relief_scale;
        let mountain_height = settings.map_or(92.0, |_| 132.0) * land_scale;
        let mut height = lerp(
            ocean_floor,
            rolling_land + ridges * mountain_height,
            land_weight,
        );

        let river = (fbm_2d(
            warped_x / (RIVER_SCALE * world_scale),
            warped_z / (RIVER_SCALE * world_scale),
            self.seed ^ 0x5249_5641,
            3,
            0.5,
        ) + fbm_2d(
            x / (RIVER_DETAIL_SCALE * world_scale),
            z / (RIVER_DETAIL_SCALE * world_scale),
            self.seed ^ 0x5249_5642,
            2,
            0.45,
        ) * 0.10)
            .abs()
            .max(spawn_weight * 0.14);
        let river_channel = 1.0 - smootherstep(remap(river, 0.018, 0.070));
        let low_relief = 1.0
            - smootherstep(remap(
                height,
                f64::from(sea_level) + 22.0,
                f64::from(sea_level) + 49.0,
            ));
        let river_weight = river_channel * land_weight * low_relief * (1.0 - spawn_weight);
        let river_floor = f64::from(sea_level) - 3.0 + detail.abs() * 0.5;
        height = lerp(height, river_floor, river_weight);
        let routed_river = river.max((1.0 - river_weight) * 0.10);

        let temperature = self.temperature(x, height, z, settings);
        let moisture = fbm_2d(
            warped_x / (1_650.0 * world_scale),
            warped_z / (1_650.0 * world_scale),
            self.seed ^ 0x4D4F_4953,
            3,
            0.52,
        );

        TerrainSample {
            surface_y: self.clamp_height(height),
            continentalness,
            ridges,
            river: routed_river,
            temperature,
            moisture,
        }
    }

    pub(in crate::terrain) fn is_cave(self, x: i32, y: i32, z: i32, surface_y: i32) -> bool {
        let cave_ceiling = surface_y.saturating_sub(CAVE_SURFACE_CLEARANCE).min(32);
        if y >= cave_ceiling {
            return false;
        }
        self.is_cave_raw(x, y, z)
            && [(-1, 0), (1, 0), (0, -1), (0, 1)]
                .into_iter()
                .any(|(dx, dz)| {
                    x.checked_add(dx)
                        .zip(z.checked_add(dz))
                        .is_some_and(|(x, z)| self.is_cave_raw(x, y, z))
                })
    }

    fn is_cave_raw(self, x: i32, y: i32, z: i32) -> bool {
        if y <= self.geometry.min_y().saturating_add(10) {
            return false;
        }
        let x = f64::from(x);
        let y = f64::from(y);
        let z = f64::from(z);
        let region = fbm_3d(
            x / 220.0,
            y / 110.0,
            z / 220.0,
            self.seed ^ 0x4341_5652,
            2,
            0.5,
        );
        if region < -0.18 {
            return false;
        }
        let tunnel_a = fbm_3d(x / 28.0, y / 5.5, z / 28.0, self.seed ^ 0x4341_5641, 2, 0.5);
        if tunnel_a.abs() >= 0.024 {
            return false;
        }
        let tunnel_b = fbm_3d(x / 34.0, y / 6.5, z / 34.0, self.seed ^ 0x4341_5642, 2, 0.5);
        tunnel_b.abs() < 0.032
    }

    fn settings(self) -> Option<TellusWorldgenSettings> {
        match self.mode {
            WorldgenMode::VanillaLike => None,
            WorldgenMode::TellusLike(settings) => Some(settings),
        }
    }

    fn temperature(
        self,
        x: f64,
        height: f64,
        z: f64,
        settings: Option<TellusWorldgenSettings>,
    ) -> f64 {
        let local = fbm_2d(x / 2_100.0, z / 2_100.0, self.seed ^ 0x5445_4D50, 3, 0.5);
        let Some(settings) = settings else {
            return local;
        };
        let blocks_per_degree =
            111_319.491_666_666_67 / settings.world_scale_meters_per_block.max(0.001);
        let latitude = (-z / blocks_per_degree)
            .to_radians()
            .sinh()
            .atan()
            .to_degrees();
        let latitude_cooling = (latitude.abs() / 85.051_128_78).clamp(0.0, 1.0) * 2.0 - 1.0;
        let altitude_cooling =
            ((height - f64::from(settings.sea_level)).max(0.0) / 128.0).min(1.0) * 0.85;
        (local - latitude_cooling * settings.climate_strength.max(0.0) - altitude_cooling)
            .clamp(-1.0, 1.0)
    }

    fn clamp_height(self, height: f64) -> i32 {
        let min = self.geometry.min_y().saturating_add(2);
        let max = self.geometry.max_y().saturating_sub(2).min(300).max(min);
        height.round().clamp(f64::from(min), f64::from(max)) as i32
    }
}

fn normalized(value: f64) -> f64 {
    ((value + 1.0) * 0.5).clamp(0.0, 1.0)
}

fn remap(value: f64, low: f64, high: f64) -> f64 {
    ((value - low) / (high - low)).clamp(0.0, 1.0)
}

fn smootherstep(value: f64) -> f64 {
    value * value * value * (value * (value * 6.0 - 15.0) + 10.0)
}

fn lerp(a: f64, b: f64, weight: f64) -> f64 {
    a + (b - a) * weight
}

#[cfg(test)]
#[path = "router_tests.rs"]
mod tests;
