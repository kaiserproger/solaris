use mc_world::chunk::ChunkGeometry;

use crate::noise::{fbm_2d, fbm_3d};

use super::super::{SEA_LEVEL, TellusWorldgenSettings, WorldgenMode};

const DOMAIN_WARP_SCALE: f64 = 1_400.0;
const DOMAIN_WARP_AMPLITUDE: f64 = 180.0;
const CONTINENT_SCALE: f64 = 2_200.0;
const EROSION_SCALE: f64 = 980.0;
const RIDGE_SCALE: f64 = 720.0;
const DETAIL_SCALE: f64 = 150.0;
const RIVER_SCALE: f64 = 760.0;
const SPAWN_LAND_RADIUS: f64 = 192.0;

#[derive(Clone, Copy, Debug)]
pub(in crate::terrain) struct TerrainSample {
    pub(in crate::terrain) surface_y: i32,
    pub(in crate::terrain) continentalness: f64,
    pub(in crate::terrain) ridges: f64,
    pub(in crate::terrain) river: f64,
    pub(in crate::terrain) temperature: f64,
    pub(in crate::terrain) moisture: f64,
}

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
        let settings = match self.mode {
            WorldgenMode::VanillaLike => None,
            WorldgenMode::TellusLike(settings) => Some(settings),
        };
        let scale = settings
            .map(|value| (value.world_scale_meters_per_block / 30.0).clamp(0.2, 8.0))
            .unwrap_or(1.0);
        let x_f = f64::from(x);
        let z_f = f64::from(z);
        let warp_x = fbm_2d(
            x_f / (DOMAIN_WARP_SCALE * scale),
            z_f / (DOMAIN_WARP_SCALE * scale),
            self.seed ^ 0x4F56_5758,
            3,
            0.52,
        ) * DOMAIN_WARP_AMPLITUDE
            * scale;
        let warp_z = fbm_2d(
            x_f / (DOMAIN_WARP_SCALE * scale),
            z_f / (DOMAIN_WARP_SCALE * scale),
            self.seed ^ 0x4F56_575A,
            3,
            0.52,
        ) * DOMAIN_WARP_AMPLITUDE
            * scale;
        let warped_x = x_f + warp_x;
        let warped_z = z_f + warp_z;

        let raw_continentalness = fbm_2d(
            warped_x / (CONTINENT_SCALE * scale),
            warped_z / (CONTINENT_SCALE * scale),
            self.seed ^ 0x434F_4E54,
            5,
            0.54,
        );
        let spawn_weight = 1.0 - smoothstep((x_f.hypot(z_f) / SPAWN_LAND_RADIUS).clamp(0.0, 1.0));
        let continentalness = raw_continentalness.max(0.34 * spawn_weight);
        let erosion = normalized(fbm_2d(
            warped_x / (EROSION_SCALE * scale),
            warped_z / (EROSION_SCALE * scale),
            self.seed ^ 0x4552_4F53,
            4,
            0.55,
        ));
        let ridge_noise = fbm_2d(
            warped_x / (RIDGE_SCALE * scale),
            warped_z / (RIDGE_SCALE * scale),
            self.seed ^ 0x5249_4447,
            4,
            0.5,
        );
        let ridges = smoothstep(((1.0 - ridge_noise.abs()) - 0.42) / 0.58).powi(4);
        let detail = fbm_2d(
            warped_x / DETAIL_SCALE,
            warped_z / DETAIL_SCALE,
            self.seed ^ 0x4445_5441,
            4,
            0.48,
        );
        let river = fbm_2d(
            warped_x / (RIVER_SCALE * scale),
            warped_z / (RIVER_SCALE * scale),
            self.seed ^ 0x5249_5645,
            3,
            0.5,
        )
        .abs()
        .max(spawn_weight * 0.12);
        let temperature = self.temperature(x_f, z_f, settings);
        let moisture = fbm_2d(
            warped_x / (1_250.0 * scale),
            warped_z / (1_250.0 * scale),
            self.seed ^ 0x4D4F_4953,
            4,
            0.52,
        );

        let sea_level = settings.map_or(SEA_LEVEL, |value| value.sea_level);
        let coast = smoothstep(((continentalness + 0.22) / 0.44).clamp(0.0, 1.0));
        let ocean = f64::from(sea_level) - 8.0 - (-continentalness).max(0.0) * 78.0 + detail * 2.5;
        let mountain_mask =
            smoothstep(((continentalness - 0.12) / 0.46).clamp(0.0, 1.0)) * (1.0 - erosion).powi(2);
        let mountain_scale = settings
            .map(|value| 210.0 * value.terrestrial_height_scale.max(0.0))
            .unwrap_or(74.0);
        let mode_uplift = settings
            .map(|value| 3.0 * value.terrestrial_height_scale.max(0.0))
            .unwrap_or(0.0);
        let upland = f64::from(sea_level)
            + 7.0
            + mode_uplift
            + continentalness.max(0.0) * 24.0
            + detail * (5.0 + erosion * 4.0)
            + ridges * mountain_mask * mountain_scale;
        let mut height = ocean * (1.0 - coast) + upland * coast;

        let river_strength = 1.0 - smoothstep((river / 0.085).clamp(0.0, 1.0));
        let river_land = smoothstep(((continentalness + 0.08) / 0.03).clamp(0.0, 1.0));
        let river_floor = f64::from(sea_level) - 4.5 + detail.abs() * 0.5;
        let river_blend = river_strength * river_land * (1.0 - spawn_weight) * 0.92;
        height = height * (1.0 - river_blend) + river_floor * river_blend;

        if let Some(settings) = settings
            && continentalness < -0.05
        {
            let scaled_depth =
                (f64::from(sea_level) - height) * settings.oceanic_height_scale.max(0.0);
            height = f64::from(sea_level) - scaled_depth;
        }

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
        let x = f64::from(x);
        let y = f64::from(y);
        let z = f64::from(z);
        let warp = fbm_3d(
            x / 180.0,
            y / 130.0,
            z / 180.0,
            self.seed ^ 0x4341_5657,
            1,
            0.5,
        ) * 20.0;
        let warped_x = x + warp;
        let warped_z = z - warp;
        let vertical_perturbation =
            fbm_3d(x / 48.0, y / 8.0, z / 48.0, self.seed ^ 0x4341_5647, 1, 0.5);
        let tunnel_a = fbm_3d(
            warped_x / 58.0,
            y / 24.0,
            warped_z / 58.0,
            self.seed ^ 0x4341_5641,
            2,
            0.52,
        ) + vertical_perturbation * 0.2;
        let tunnel_b = fbm_3d(
            warped_x / 67.0,
            y / 29.0,
            warped_z / 67.0,
            self.seed ^ 0x4341_5642,
            2,
            0.52,
        ) - vertical_perturbation * 0.14;
        let chamber = fbm_3d(
            x / 96.0,
            y / 48.0,
            z / 96.0,
            self.seed ^ 0x4341_5643,
            2,
            0.56,
        ) + vertical_perturbation * 0.22;
        (tunnel_a.abs() < 0.08 && tunnel_b.abs() < 0.19) || chamber > 0.72
    }

    fn temperature(self, x: f64, z: f64, settings: Option<TellusWorldgenSettings>) -> f64 {
        let local = fbm_2d(x / 1_700.0, z / 1_700.0, self.seed ^ 0x5445_4D50, 3, 0.5);
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
        let max = self.geometry.max_y().saturating_sub(2).min(250).max(min);
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

#[cfg(test)]
mod tests {
    use super::*;
    use mc_world::chunk::OVERWORLD_GEOMETRY;

    #[test]
    fn samples_are_deterministic_and_locally_continuous() {
        let router = DensityRouter::new(42, OVERWORLD_GEOMETRY, WorldgenMode::VanillaLike);
        for x in -128..128 {
            let sample = router.sample(x, 31);
            assert_eq!(sample.surface_y, router.sample(x, 31).surface_y);
            assert!(
                (sample.surface_y - router.sample(x + 1, 31).surface_y).abs() <= 3,
                "terrain step at x={x}"
            );
        }
    }

    #[test]
    fn spawn_area_is_dry_land_for_random_seeds() {
        for seed in -64..64 {
            let router = DensityRouter::new(seed, OVERWORLD_GEOMETRY, WorldgenMode::VanillaLike);
            assert!(router.sample(0, 0).surface_y >= SEA_LEVEL + 3);
        }
    }

    #[test]
    fn caves_are_sparse_in_a_vertical_column() {
        let router = DensityRouter::new(7, OVERWORLD_GEOMETRY, WorldgenMode::VanillaLike);
        let carved = (-48..40).filter(|&y| router.is_cave(91, y, -37)).count();
        assert!(carved < 32, "cave router opened {carved} of 88 cells");
    }

    #[test]
    fn cave_density_does_not_open_entire_vertical_columns() {
        for seed in -8..8 {
            let router = DensityRouter::new(seed, OVERWORLD_GEOMETRY, WorldgenMode::VanillaLike);
            let offset = (seed * 17_i64).rem_euclid(48) as i32;
            for x in ((-192 + offset)..=(192 + offset)).step_by(48) {
                for z in ((-192 - offset)..=(192 - offset)).step_by(48) {
                    let mut longest = 0;
                    let mut current = 0;
                    for y in (-48..=40).step_by(2) {
                        if router.is_cave(x, y, z) {
                            current += 1;
                            longest = longest.max(current);
                        } else {
                            current = 0;
                        }
                    }
                    assert!(
                        longest <= 32,
                        "seed {seed} opened {} continuous vertical cave blocks at {x},{z}",
                        longest * 2
                    );
                }
            }
        }
    }

    #[test]
    fn sampled_cave_cells_are_not_isolated() {
        let mut caves = 0;
        let mut isolated = 0;
        for seed in -4..4 {
            let router = DensityRouter::new(seed, OVERWORLD_GEOMETRY, WorldgenMode::VanillaLike);
            for x in (-96..=96).step_by(8) {
                for z in (-96..=96).step_by(8) {
                    for y in (-48..=40).step_by(4) {
                        if !router.is_cave(x, y, z) {
                            continue;
                        }
                        caves += 1;
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
                    }
                }
            }
        }
        assert!(caves > 32, "sample should include enough cave cells");
        assert!(
            isolated * 100 <= caves,
            "{isolated} of {caves} sampled cave cells were isolated"
        );
    }
}
