use mc_world::chunk::ChunkGeometry;

use crate::noise::{fbm_2d, fbm_3d};

use super::super::{SEA_LEVEL, TellusWorldgenSettings, WorldgenMode};

const CONTINENT_SCALE: f64 = 3_200.0;
const COAST_SCALE: f64 = 1_400.0;
const PLATE_SCALE: f64 = 4_100.0;
const RIDGE_SCALE: f64 = 1_150.0;
const EROSION_SCALE: f64 = 1_900.0;
const HILL_SCALE: f64 = 520.0;
const DETAIL_SCALE: f64 = 170.0;
const RIVER_SCALE: f64 = 1_650.0;
const RIVER_DETAIL_SCALE: f64 = 520.0;
const SPAWN_LAND_RADIUS: f64 = 256.0;

#[derive(Clone, Copy, Debug)]
pub(in crate::terrain) struct TerrainSample {
    pub(in crate::terrain) surface_y: i32,
    pub(in crate::terrain) continentalness: f64,
    pub(in crate::terrain) ridges: f64,
    /// Distance from the centre of the closest river field.
    pub(in crate::terrain) river: f64,
    pub(in crate::terrain) temperature: f64,
    pub(in crate::terrain) moisture: f64,
}

/// Stateless terrain authority shared by chunk generation, spawn selection,
/// biome routing, structures, and decoration support checks.
#[derive(Clone, Copy, Debug)]
pub(in crate::terrain) struct DensityRouter {
    seed: i64,
    geometry: ChunkGeometry,
    mode: WorldgenMode,
}

impl DensityRouter {
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

    pub(in crate::terrain) fn sample(self, x: i32, z: i32) -> TerrainSample {
        let settings = self.settings();
        let scale = settings
            .map(|value| (value.world_scale_meters_per_block / 30.0).clamp(0.25, 8.0))
            .unwrap_or(1.0);
        let x = f64::from(x);
        let z = f64::from(z);
        let spawn_weight = 1.0 - smoothstep((x.hypot(z) / SPAWN_LAND_RADIUS).clamp(0.0, 1.0));

        let continent = fbm_2d(
            x / (CONTINENT_SCALE * scale),
            z / (CONTINENT_SCALE * scale),
            self.seed ^ 0x434F_4E54,
            4,
            0.52,
        );
        let coast = fbm_2d(
            x / (COAST_SCALE * scale),
            z / (COAST_SCALE * scale),
            self.seed ^ 0x434F_4153,
            2,
            0.48,
        );
        let continentalness = (continent + coast * 0.13 + 0.08).max(0.34 * spawn_weight);

        let erosion = normalized(fbm_2d(
            x / (EROSION_SCALE * scale),
            z / (EROSION_SCALE * scale),
            self.seed ^ 0x4552_4F53,
            3,
            0.52,
        ));
        let plate = smoothstep(
            (fbm_2d(
                x / (PLATE_SCALE * scale),
                z / (PLATE_SCALE * scale),
                self.seed ^ 0x504C_4154,
                3,
                0.5,
            ) - 0.08)
                / 0.52,
        ) * smoothstep((continentalness - 0.02) / 0.34);
        let ridge_distance = fbm_2d(
            x / (RIDGE_SCALE * scale),
            z / (RIDGE_SCALE * scale),
            self.seed ^ 0x5249_4447,
            3,
            0.5,
        )
        .abs();
        let ridge_shape = smoothstep((0.38 - ridge_distance) / 0.28).powi(2);
        let ridges = plate * ridge_shape;

        let hills = fbm_2d(
            x / (HILL_SCALE * scale),
            z / (HILL_SCALE * scale),
            self.seed ^ 0x4849_4C4C,
            3,
            0.5,
        );
        let detail = fbm_2d(
            x / (DETAIL_SCALE * scale),
            z / (DETAIL_SCALE * scale),
            self.seed ^ 0x4445_5441,
            2,
            0.45,
        );

        let sea_level = settings.map_or(SEA_LEVEL, |value| value.sea_level);
        let ocean_scale = settings.map_or(1.0, |value| value.oceanic_height_scale.max(0.0));
        let land_scale = settings.map_or(1.0, |value| value.terrestrial_height_scale.max(0.0));
        let land_weight = smoothstep((continentalness + 0.24) / 0.48);
        let ocean_floor = f64::from(sea_level)
            - (12.0 + (-continentalness).max(0.0) * 48.0) * ocean_scale
            + hills * 2.0;
        let relief = if settings.is_some() { 1.12 } else { 1.0 };
        let lowlands = f64::from(sea_level)
            + 8.0
            + continentalness.max(0.0) * 24.0 * land_scale * relief
            + hills * 6.0 * land_scale * relief
            + detail * 1.5 * land_scale * relief;
        let mountain_scale = settings.map_or(84.0, |_| 142.0) * land_scale;
        let mountain = ridges * (0.42 + (1.0 - erosion) * 0.58) * mountain_scale;
        let mut height = lerp(ocean_floor, lowlands + mountain, land_weight);

        let raw_river = (fbm_2d(
            x / (RIVER_SCALE * scale),
            z / (RIVER_SCALE * scale),
            self.seed ^ 0x5249_5645,
            2,
            0.5,
        ) + fbm_2d(
            x / (RIVER_DETAIL_SCALE * scale),
            z / (RIVER_DETAIL_SCALE * scale),
            self.seed ^ 0x5249_5644,
            2,
            0.45,
        ) * 0.10)
            .abs();
        let river_land = smoothstep((continentalness + 0.03) / 0.12);
        let river = (raw_river * 0.55)
            .max(spawn_weight * 0.14)
            .max((1.0 - river_land) * 0.04);
        let river_strength = 1.0 - smoothstep((river - 0.025) / 0.080);
        let river_floor = f64::from(sea_level) - 4.0 + detail.abs().min(0.8);
        height = lerp(
            height,
            river_floor,
            river_strength * river_land * (1.0 - spawn_weight),
        );

        let temperature = self.temperature(x, height, z, settings);
        let moisture = fbm_2d(
            x / (1_700.0 * scale),
            z / (1_700.0 * scale),
            self.seed ^ 0x4D4F_4953,
            3,
            0.52,
        );

        TerrainSample {
            surface_y: self.clamp_height(height),
            continentalness,
            ridges,
            river,
            temperature,
            moisture,
        }
    }

    pub(in crate::terrain) fn is_cave(self, x: i32, y: i32, z: i32) -> bool {
        if y <= self.geometry.min_y().saturating_add(7) || y >= 40 {
            return false;
        }
        let x = f64::from(x);
        let y = f64::from(y);
        let z = f64::from(z);
        let region = fbm_3d(
            x / 180.0,
            y / 96.0,
            z / 180.0,
            self.seed ^ 0x4341_5652,
            2,
            0.5,
        );
        if region < -0.28 {
            return false;
        }
        let tunnel_a = fbm_3d(x / 48.0, y / 4.5, z / 48.0, self.seed ^ 0x4341_5641, 2, 0.5);
        if tunnel_a.abs() >= 0.040 {
            return false;
        }
        let tunnel_b = fbm_3d(x / 56.0, y / 5.0, z / 56.0, self.seed ^ 0x4341_5642, 2, 0.5);
        tunnel_b.abs() < 0.060
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

fn smoothstep(value: f64) -> f64 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

fn lerp(a: f64, b: f64, weight: f64) -> f64 {
    a + (b - a) * weight
}

#[cfg(test)]
#[path = "density_tests.rs"]
mod tests;
