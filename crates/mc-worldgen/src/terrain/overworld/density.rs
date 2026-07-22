use mc_world::chunk::ChunkGeometry;

use crate::noise::{fbm_2d, fbm_3d};

use super::super::{SEA_LEVEL, TellusWorldgenSettings, WorldgenMode};

const WARP_SCALE: f64 = 1_700.0;
const WARP_STRENGTH: f64 = 220.0;
const CONTINENT_SCALE: f64 = 3_600.0;
const COAST_DETAIL_SCALE: f64 = 900.0;
const EROSION_SCALE: f64 = 1_150.0;
const MOUNTAIN_PROVINCE_SCALE: f64 = 2_700.0;
const RIDGE_SCALE: f64 = 760.0;
const HILL_SCALE: f64 = 210.0;
const RIVER_SCALE: f64 = 980.0;
const RIVER_DETAIL_SCALE: f64 = 280.0;
const SPAWN_LAND_RADIUS: f64 = 224.0;

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

/// One coordinate-derived authority for Overworld shape and climate.
///
/// The router is stateless, so neighboring chunks can be generated in any
/// order and on different workers without a repair or stitching pass.
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

        let warp_x = fbm_2d(
            x / (WARP_SCALE * scale),
            z / (WARP_SCALE * scale),
            self.seed ^ 0x5752_5058,
            3,
            0.5,
        ) * WARP_STRENGTH
            * scale;
        let warp_z = fbm_2d(
            x / (WARP_SCALE * scale),
            z / (WARP_SCALE * scale),
            self.seed ^ 0x5752_505A,
            3,
            0.5,
        ) * WARP_STRENGTH
            * scale;
        let wx = x + warp_x;
        let wz = z + warp_z;

        let continent = fbm_2d(
            wx / (CONTINENT_SCALE * scale),
            wz / (CONTINENT_SCALE * scale),
            self.seed ^ 0x434F_4E54,
            5,
            0.52,
        );
        let coast_detail = fbm_2d(
            wx / (COAST_DETAIL_SCALE * scale),
            wz / (COAST_DETAIL_SCALE * scale),
            self.seed ^ 0x434F_4153,
            3,
            0.5,
        );
        let continentalness = (continent + coast_detail * 0.16).max(0.30 * spawn_weight);
        let erosion = normalized(fbm_2d(
            wx / (EROSION_SCALE * scale),
            wz / (EROSION_SCALE * scale),
            self.seed ^ 0x4552_4F53,
            4,
            0.53,
        ));
        let province = smoothstep(
            (fbm_2d(
                wx / (MOUNTAIN_PROVINCE_SCALE * scale),
                wz / (MOUNTAIN_PROVINCE_SCALE * scale),
                self.seed ^ 0x4D4F_554E,
                3,
                0.52,
            ) - 0.08)
                / 0.52,
        ) * smoothstep((continentalness - 0.04) / 0.36);
        let ridge_distance = fbm_2d(
            wx / (RIDGE_SCALE * scale),
            wz / (RIDGE_SCALE * scale),
            self.seed ^ 0x5249_4447,
            4,
            0.5,
        )
        .abs();
        let ridge_shape = smoothstep((0.50 - ridge_distance) / 0.30).powi(2);
        let ridges = province * ridge_shape;
        let hills = fbm_2d(
            wx / HILL_SCALE,
            wz / HILL_SCALE,
            self.seed ^ 0x4849_4C4C,
            4,
            0.48,
        );

        let raw_river = (fbm_2d(
            wx / (RIVER_SCALE * scale),
            wz / (RIVER_SCALE * scale),
            self.seed ^ 0x5249_5645,
            3,
            0.5,
        ) + fbm_2d(
            wx / (RIVER_DETAIL_SCALE * scale),
            wz / (RIVER_DETAIL_SCALE * scale),
            self.seed ^ 0x5249_5644,
            2,
            0.5,
        ) * 0.13)
            .abs();

        let sea_level = settings.map_or(SEA_LEVEL, |value| value.sea_level);
        let coast = smoothstep((continentalness + 0.28) / 0.50);
        let ocean_depth_scale = settings.map_or(1.0, |value| value.oceanic_height_scale.max(0.0));
        let ocean_floor = f64::from(sea_level)
            - (10.0 + (-continentalness).max(0.0) * 62.0) * ocean_depth_scale
            + hills * 3.0;
        let land_scale = settings.map_or(1.0, |value| value.terrestrial_height_scale.max(0.0));
        let rolling_land = f64::from(sea_level)
            + 8.0
            + continentalness.max(0.0) * 28.0 * land_scale
            + hills * (5.0 + erosion * 4.0) * land_scale;
        let mountain_height = settings.map_or(96.0, |_| 210.0) * land_scale;
        let mountain_uplift = ridges * (1.0 - erosion * 0.72) * mountain_height;
        let mut height = lerp(ocean_floor, rolling_land + mountain_uplift, coast);

        // Rivers flatten an existing land surface. They never subtract a shaft
        // from the density field, so a river cannot turn into a vertical hole.
        let river_land = smoothstep((continentalness + 0.05) / 0.10);
        let river = raw_river
            .max(spawn_weight * 0.13)
            .max((1.0 - river_land) * 0.04);
        let river_strength = 1.0 - smoothstep((river - 0.022) / 0.075);
        let river_floor = f64::from(sea_level) - 4.0 + hills.abs().min(0.75);
        let river_blend = river_strength * river_land * (1.0 - spawn_weight);
        height = lerp(height, river_floor, river_blend);

        let temperature = self.temperature(x, z, settings);
        let moisture = fbm_2d(
            wx / (1_450.0 * scale),
            wz / (1_450.0 * scale),
            self.seed ^ 0x4D4F_4953,
            4,
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
        if y <= self.geometry.min_y().saturating_add(6) || y >= 48 {
            return false;
        }
        let x = f64::from(x);
        let y = f64::from(y);
        let z = f64::from(z);
        let warp = fbm_3d(
            x / 190.0,
            y / 120.0,
            z / 190.0,
            self.seed ^ 0x4341_5657,
            1,
            0.5,
        ) * 18.0;
        let xw = x + warp;
        let zw = z - warp;
        let tunnel_a = fbm_3d(
            xw / 68.0,
            y / 10.0,
            zw / 68.0,
            self.seed ^ 0x4341_5641,
            2,
            0.52,
        );
        if tunnel_a.abs() < 0.055 {
            let tunnel_b = fbm_3d(
                xw / 76.0,
                y / 9.0,
                zw / 76.0,
                self.seed ^ 0x4341_5642,
                2,
                0.52,
            );
            if tunnel_b.abs() < 0.13 {
                return true;
            }
        }
        if y >= -12.0 {
            return false;
        }
        let chamber = fbm_3d(
            x / 105.0,
            y / 58.0,
            z / 105.0,
            self.seed ^ 0x4341_5643,
            3,
            0.54,
        );
        if chamber <= 0.78 {
            return false;
        }
        let chamber_mask = fbm_3d(
            x / 42.0,
            y / 36.0,
            z / 42.0,
            self.seed ^ 0x4341_564D,
            2,
            0.5,
        );
        chamber_mask > 0.10
    }

    fn settings(self) -> Option<TellusWorldgenSettings> {
        match self.mode {
            WorldgenMode::VanillaLike => None,
            WorldgenMode::TellusLike(settings) => Some(settings),
        }
    }

    fn temperature(self, x: f64, z: f64, settings: Option<TellusWorldgenSettings>) -> f64 {
        let local = fbm_2d(x / 1_900.0, z / 1_900.0, self.seed ^ 0x5445_4D50, 3, 0.5);
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
        (local - latitude_cooling * settings.climate_strength.max(0.0)).clamp(-1.0, 1.0)
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
mod tests {
    use super::*;
    use mc_world::chunk::OVERWORLD_GEOMETRY;

    #[test]
    fn surface_is_deterministic_continuous_and_spawn_safe() {
        for seed in -32..32 {
            let router = DensityRouter::new(seed, OVERWORLD_GEOMETRY, WorldgenMode::VanillaLike);
            assert!(router.sample(0, 0).surface_y >= SEA_LEVEL + 3);
            for x in -96..96 {
                let current = router.sample(x, 31).surface_y;
                let next = router.sample(x + 1, 31).surface_y;
                assert_eq!(current, router.sample(x, 31).surface_y);
                assert!(
                    (current - next).abs() <= 3,
                    "terrain step at x={x}: {current} -> {next}"
                );
            }
        }
    }

    #[test]
    fn caves_are_sparse_locally_coherent_and_never_vertical_shafts() {
        let mut caves = 0;
        let mut isolated = 0;
        let mut sampled = 0;
        for seed in -4..4 {
            let router = DensityRouter::new(seed, OVERWORLD_GEOMETRY, WorldgenMode::VanillaLike);
            for x in (-96..=96).step_by(8) {
                for z in (-96..=96).step_by(8) {
                    let mut longest = 0;
                    let mut current = 0;
                    for y in -48..=40 {
                        sampled += 1;
                        if router.is_cave(x, y, z) {
                            caves += 1;
                            current += 1;
                            longest = longest.max(current);
                            if ![
                                (1, 0, 0),
                                (-1, 0, 0),
                                (0, 1, 0),
                                (0, -1, 0),
                                (0, 0, 1),
                                (0, 0, -1),
                            ]
                            .into_iter()
                            .any(|(dx, dy, dz)| router.is_cave(x + dx, y + dy, z + dz))
                            {
                                isolated += 1;
                            }
                        } else {
                            current = 0;
                        }
                    }
                    assert!(
                        longest <= 20,
                        "seed {seed} opened a {longest}-block shaft at {x},{z}"
                    );
                }
            }
        }
        assert!(caves > 32, "sample should include caves");
        assert!(
            caves * 100 <= sampled * 12,
            "caves occupied {caves} of {sampled} sampled underground cells"
        );
        assert!(
            isolated * 100 <= caves,
            "{isolated} of {caves} cave cells were isolated"
        );
    }
}
