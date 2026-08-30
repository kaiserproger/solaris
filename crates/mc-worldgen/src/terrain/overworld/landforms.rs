use crate::noise::fbm_2d;

use super::{OverworldRouter, TerrainSample, drainage};

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
// Climate is evaluated in three spatial bands. The macro band keeps a family
// coherent over a region, the regional band bends its boundary, and the local
// band is deliberately capped so it can only add variation inside transition
// margins rather than fragmenting the family domain.
const CLIMATE_MACRO_SCALE: f64 = 12_000.0;
const CLIMATE_DOMAIN_SCALE: f64 = 3_600.0;
const CLIMATE_LOCAL_SCALE: f64 = 1_600.0;
// Preserve cold and hot reachability by widening the centered climate range
// instead of shifting every region toward one family.
const CLIMATE_TEMPERATURE_RANGE: f64 = 1.15;
const CLIMATE_TEMPERATURE_BIAS: f64 = 0.04;
const CLIMATE_WARP_SCALE: f64 = 4_200.0;
const CLIMATE_WARP_STRENGTH: f64 = 760.0;
const CLIMATE_MACRO_WEIGHT: f64 = 0.50;
const CLIMATE_REGIONAL_WEIGHT: f64 = 0.40;
const CLIMATE_LOCAL_WEIGHT: f64 = 0.06;
const CLIMATE_DOMAIN_WEIGHT: f64 = 0.04;

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

    // Coarse drainage cells choose one acyclic branch down a seeded hydraulic
    // elevation, accumulate local upstream runoff, and expose the distance to
    // the resulting segment network.
    // Keep carving alive through the coast band so channels meet the ocean rather
    // than disappearing at the old inland mask boundary.
    let drainage = if continentalness > -0.48 && ridges < 0.18 {
        drainage::sample(router.seed, block_x, block_z, scale)
    } else {
        drainage::DrainageSample::default()
    };
    let low_relief = 1.0 - smootherstep(remap(ridges, 0.025, 0.16));
    let coast_connection = 1.0 - smootherstep(remap(-continentalness, 0.18, 0.48));
    let river_weight = drainage.channel_weight * coast_connection * low_relief;
    let channel_depth = (2.4 + drainage.accumulation * 0.10).clamp(2.4, 3.6);
    let river_floor = sea - channel_depth + detail.abs() * 0.45;
    height = lerp(height, river_floor, river_weight);

    let (base_temperature, moisture, climate_domain) = climate(router, x, z, scale);
    let temperature = temperature_with_influence(router, base_temperature, height, z);

    TerrainSample {
        surface_y: router.clamp_height(height),
        continentalness,
        ridges,
        // Biome routing sees a river only after the accumulated channel is
        // substantially carved; shallow shoulders remain surrounding land.
        river: drainage.river_distance.max((1.0 - river_weight) * 0.10),
        temperature,
        moisture,
        climate_domain,
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

#[cfg(test)]
pub(super) fn temperature(router: OverworldRouter, x: f64, height: f64, z: f64) -> f64 {
    let scale = router
        .settings()
        .map(|settings| (settings.world_scale_meters_per_block / 30.0).clamp(0.25, 8.0))
        .unwrap_or(1.0);
    let (base_temperature, _, _) = climate(router, x, z, scale);
    temperature_with_influence(router, base_temperature, height, z)
}

fn climate(router: OverworldRouter, x: f64, z: f64, scale: f64) -> (f64, f64, f64) {
    // The warp is lower-frequency than local detail and is shared by every
    // climate band. It curves otherwise smooth boundaries without making an
    // independent per-axis stripe field.
    let warp_x = fbm_2d(
        (x + z * 0.31) / (CLIMATE_WARP_SCALE * scale),
        (z - x * 0.19) / (CLIMATE_WARP_SCALE * scale),
        router.seed ^ 0x434C_5758,
        3,
        0.5,
    ) * CLIMATE_WARP_STRENGTH
        * scale;
    let warp_z = fbm_2d(
        (x - z * 0.27) / (CLIMATE_WARP_SCALE * scale),
        (z + x * 0.23) / (CLIMATE_WARP_SCALE * scale),
        router.seed ^ 0x434C_575A,
        3,
        0.5,
    ) * CLIMATE_WARP_STRENGTH
        * scale;
    let climate_x = x + warp_x;
    let climate_z = z + warp_z;

    // Macro fields establish the shared regional tendency. Separate stable
    // salts keep temperature and moisture independent while the common warp
    // gives their boundaries compatible, non-axis-aligned geometry.
    let macro_temperature = fbm_2d(
        (climate_x + climate_z * 0.37) / (CLIMATE_MACRO_SCALE * scale),
        (climate_z - climate_x * 0.21) / (CLIMATE_MACRO_SCALE * scale),
        router.seed ^ 0x434C_4D54,
        4,
        0.52,
    );
    let macro_moisture = fbm_2d(
        (climate_x - climate_z * 0.29) / (CLIMATE_MACRO_SCALE * scale),
        (climate_z + climate_x * 0.17) / (CLIMATE_MACRO_SCALE * scale),
        router.seed ^ 0x434C_4D4D,
        4,
        0.52,
    );

    // Regional fields supply broad transition bands without reducing the
    // macro field to a single directional gradient.
    let regional_temperature = fbm_2d(
        (climate_x + climate_z * 0.43) / (CLIMATE_DOMAIN_SCALE * scale),
        (climate_z - climate_x * 0.33) / (CLIMATE_DOMAIN_SCALE * scale),
        router.seed ^ 0x434C_4254,
        4,
        0.52,
    );
    let regional_moisture = fbm_2d(
        (climate_x - climate_z * 0.35) / (CLIMATE_DOMAIN_SCALE * scale),
        (climate_z + climate_x * 0.25) / (CLIMATE_DOMAIN_SCALE * scale),
        router.seed ^ 0x434C_424D,
        4,
        0.52,
    );

    // Domain identity intentionally omits local noise. Its fixed 0.68/0.32
    // macro/regional blend keeps neighboring samples in one family over
    // thousands of blocks, while the selector's existing transition width
    // still receives gradual, seed-specific boundaries.
    let macro_domain = fbm_2d(
        (climate_x + climate_z * 0.19) / (CLIMATE_MACRO_SCALE * 1.12 * scale),
        (climate_z - climate_x * 0.41) / (CLIMATE_MACRO_SCALE * 1.12 * scale),
        router.seed ^ 0x434C_444D,
        3,
        0.5,
    );
    let regional_domain = fbm_2d(
        (climate_x - climate_z * 0.23) / (CLIMATE_DOMAIN_SCALE * 0.88 * scale),
        (climate_z + climate_x * 0.31) / (CLIMATE_DOMAIN_SCALE * 0.88 * scale),
        router.seed ^ 0x434C_444F,
        3,
        0.5,
    );
    let climate_domain = (macro_domain * 0.68 + regional_domain * 0.32).clamp(-1.0, 1.0);

    // Local variation is bounded explicitly. It breaks up monotony inside a
    // region, but its 0.06 contribution stays inside the selector margin.
    let local_temperature = fbm_2d(
        (climate_x + climate_z * 0.11) / (CLIMATE_LOCAL_SCALE * scale),
        (climate_z - climate_x * 0.07) / (CLIMATE_LOCAL_SCALE * scale),
        router.seed ^ 0x434C_4C54,
        3,
        0.5,
    );
    let local_moisture = fbm_2d(
        (climate_x + climate_z * 0.18) / (CLIMATE_LOCAL_SCALE * scale),
        (climate_z - climate_x * 0.14) / (CLIMATE_LOCAL_SCALE * scale),
        router.seed ^ 0x434C_4C4D,
        3,
        0.52,
    );

    (
        ((macro_temperature * CLIMATE_MACRO_WEIGHT
            + regional_temperature * CLIMATE_REGIONAL_WEIGHT
            + local_temperature * CLIMATE_LOCAL_WEIGHT)
            * CLIMATE_TEMPERATURE_RANGE
            + climate_domain * CLIMATE_DOMAIN_WEIGHT
            + CLIMATE_TEMPERATURE_BIAS)
            .clamp(-1.0, 1.0),
        (macro_moisture * CLIMATE_MACRO_WEIGHT
            + regional_moisture * CLIMATE_REGIONAL_WEIGHT
            + local_moisture * CLIMATE_LOCAL_WEIGHT
            - climate_domain * CLIMATE_DOMAIN_WEIGHT)
            .clamp(-1.0, 1.0),
        climate_domain,
    )
}

fn temperature_with_influence(
    router: OverworldRouter,
    base_temperature: f64,
    height: f64,
    z: f64,
) -> f64 {
    let Some(settings) = router.settings() else {
        return base_temperature;
    };
    let blocks_per_degree =
        111_319.491_666_666_67 / settings.world_scale_meters_per_block.max(0.001);
    let latitude = (-z / blocks_per_degree)
        .to_radians()
        .sinh()
        .atan()
        .to_degrees();
    let latitude_cooling = (latitude.abs() / 85.051_128_78).clamp(0.0, 1.0);
    let altitude_cooling =
        ((height - f64::from(settings.sea_level)).max(0.0) / 128.0).min(1.0) * 0.85;
    (base_temperature - latitude_cooling * settings.climate_strength.max(0.0) - altitude_cooling)
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
