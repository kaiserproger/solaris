use crate::noise::fbm_2d;

use super::{OverworldRouter, TerrainSample};

const CONTINENT_SCALE: f64 = 3_600.0;
const CONTINENT_DETAIL_SCALE: f64 = 1_250.0;
const WARP_SCALE: f64 = 1_500.0;
const WARP_STRENGTH: f64 = 180.0;
const EROSION_SCALE: f64 = 1_650.0;
const UPLAND_SCALE: f64 = 820.0;
const HILL_LONG_SCALE: f64 = 720.0;
const HILL_CROSS_SCALE: f64 = 280.0;
const DETAIL_SCALE: f64 = 190.0;
const MOUNTAIN_SCALE_A: f64 = 2_200.0;
const MOUNTAIN_SCALE_B: f64 = 1_550.0;
const MOUNTAIN_DETAIL_LONG_SCALE: f64 = 520.0;
const MOUNTAIN_DETAIL_CROSS_SCALE: f64 = 210.0;
const RIVER_SCALE: f64 = 1_850.0;
const RIVER_DETAIL_SCALE: f64 = 610.0;

pub(super) fn sample(router: OverworldRouter, block_x: i32, block_z: i32) -> TerrainSample {
    let settings = router.settings();
    let scale = settings
        .map(|value| (value.world_scale_meters_per_block / 30.0).clamp(0.25, 8.0))
        .unwrap_or(1.0);
    let x = f64::from(block_x);
    let z = f64::from(block_z);
    let sea = f64::from(router.sea_level());

    let warp_x = fbm_2d(
        x / (WARP_SCALE * scale),
        z / (WARP_SCALE * scale),
        router.seed ^ 0x5752_5058,
        3,
        0.5,
    ) * WARP_STRENGTH
        * scale;
    let warp_z = fbm_2d(
        x / (WARP_SCALE * scale),
        z / (WARP_SCALE * scale),
        router.seed ^ 0x5752_505A,
        3,
        0.5,
    ) * WARP_STRENGTH
        * scale;
    let wx = x + warp_x;
    let wz = z + warp_z;

    let continent_macro = fbm_2d(
        wx / (CONTINENT_SCALE * scale),
        wz / (CONTINENT_SCALE * scale),
        router.seed ^ 0x434F_4E54,
        5,
        0.53,
    );
    let continent_detail = fbm_2d(
        wx / (CONTINENT_DETAIL_SCALE * scale),
        wz / (CONTINENT_DETAIL_SCALE * scale),
        router.seed ^ 0x434F_4445,
        3,
        0.5,
    );
    let continentalness = continent_macro * 0.82 + continent_detail * 0.18;
    let land = smootherstep(remap(continentalness, -0.20, 0.09));

    let erosion = normalized(fbm_2d(
        wx / (EROSION_SCALE * scale),
        wz / (EROSION_SCALE * scale),
        router.seed ^ 0x4552_4F53,
        4,
        0.52,
    ));
    let upland = fbm_2d(
        wx / (UPLAND_SCALE * scale),
        wz / (UPLAND_SCALE * scale),
        router.seed ^ 0x5550_4C44,
        3,
        0.5,
    );
    let hills = rolling_hills(router, wx, wz, scale);
    let detail = fbm_2d(
        x / (DETAIL_SCALE * scale),
        z / (DETAIL_SCALE * scale),
        router.seed ^ 0x4445_544C,
        2,
        0.45,
    );
    let mountain_detail = fbm_2d(
        (wx + wz * 0.31) / (MOUNTAIN_DETAIL_LONG_SCALE * scale),
        (wz - wx * 0.24) / (MOUNTAIN_DETAIL_CROSS_SCALE * scale),
        router.seed ^ 0x4D44_544C,
        4,
        0.52,
    );

    // Two differently oriented ridge fields produce long, branching ranges
    // instead of isolated noise peaks.
    let ridge_a = 1.0
        - fbm_2d(
            (wx + wz * 0.28) / (MOUNTAIN_SCALE_A * scale),
            (wz - wx * 0.16) / (MOUNTAIN_SCALE_A * scale),
            router.seed ^ 0x5249_4441,
            4,
            0.5,
        )
        .abs();
    let ridge_b = 1.0
        - fbm_2d(
            (wx - wz * 0.41) / (MOUNTAIN_SCALE_B * scale),
            (wz + wx * 0.22) / (MOUNTAIN_SCALE_B * scale),
            router.seed ^ 0x5249_4442,
            3,
            0.5,
        )
        .abs();
    let ridge_shape = ridge_a
        .clamp(0.0, 1.0)
        .powi(5)
        .max(ridge_b.clamp(0.0, 1.0).powi(6) * 0.72);
    let mountain_domain = smootherstep(remap(continentalness, 0.10, 0.46))
        * smootherstep(remap(1.0 - erosion, 0.30, 0.78));
    let ridges = ridge_shape * mountain_domain;

    let ocean_scale = settings.map_or(1.0, |value| value.oceanic_height_scale.max(0.0));
    let land_scale = settings.map_or(1.0, |value| value.terrestrial_height_scale.max(0.0));
    let deep_ocean = smootherstep(remap(-continentalness, 0.10, 0.62));
    let ocean_floor = sea - (10.0 + deep_ocean * 38.0) * ocean_scale + hills * 2.5 * ocean_scale;
    let rolling_land = sea
        + 7.0
        + continentalness.max(0.0) * 20.0 * land_scale
        + upland * (7.0 + (1.0 - erosion) * 12.0) * land_scale
        + hills * 8.0 * land_scale
        + detail * 1.8 * land_scale;
    let mountain_height = settings.map_or(98.0, |_| 142.0) * land_scale;
    let mountain_relief = ridges * (mountain_height + mountain_detail * 36.0 * land_scale);
    let mut height = lerp(ocean_floor, rolling_land + mountain_relief, land);

    // A warped zero contour forms continuous valleys. Mountain relief suppresses
    // it before it can become a local sink.
    let river_field = fbm_2d(
        wx / (RIVER_SCALE * scale),
        wz / (RIVER_SCALE * scale),
        router.seed ^ 0x5249_5641,
        3,
        0.5,
    ) + fbm_2d(
        x / (RIVER_DETAIL_SCALE * scale),
        z / (RIVER_DETAIL_SCALE * scale),
        router.seed ^ 0x5249_5642,
        2,
        0.45,
    ) * 0.11;
    let river_distance = river_field.abs();
    let river_channel = 1.0 - smootherstep(remap(river_distance, 0.020, 0.078));
    let low_relief = 1.0 - smootherstep(remap(ridges, 0.025, 0.16));
    let inland = smootherstep(remap(continentalness, -0.02, 0.18));
    let river_weight = river_channel * inland * low_relief;
    let river_floor = sea - 3.0 + detail.abs() * 0.55;
    height = lerp(height, river_floor, river_weight);

    let temperature = temperature(router, x, height, z);
    let moisture = fbm_2d(
        wx / (1_700.0 * scale),
        wz / (1_700.0 * scale),
        router.seed ^ 0x4D4F_4953,
        3,
        0.52,
    );

    TerrainSample {
        surface_y: router.clamp_height(height),
        continentalness,
        ridges,
        // Biome routing sees a river only after the valley is substantially
        // carved; shallow shoulders remain their surrounding land biome.
        river: river_distance.max((1.0 - river_weight) * 0.16),
        temperature,
        moisture,
    }
}

pub(super) fn rolling_hills(router: OverworldRouter, x: f64, z: f64, scale: f64) -> f64 {
    // Stretch and rotate rolling relief so hills form long shoulders instead
    // of round, short-scale bumps independent from the mountain ranges.
    fbm_2d(
        (x + z * 0.34) / (HILL_LONG_SCALE * scale),
        (z - x * 0.18) / (HILL_CROSS_SCALE * scale),
        router.seed ^ 0x4849_4C4C,
        3,
        0.5,
    )
}

pub(super) fn temperature(router: OverworldRouter, x: f64, height: f64, z: f64) -> f64 {
    let local = fbm_2d(x / 2_100.0, z / 2_100.0, router.seed ^ 0x5445_4D50, 3, 0.5);
    let Some(settings) = router.settings() else {
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
