//! Altitude and climate routing over the shared deterministic terrain field.

use super::{SEA_LEVEL, TellusWorldgenSettings, TerrainGenerator, TerrainSample, WorldgenMode};
use mc_data::Identifier;

const RIVER_BIOME_WIDTH: f64 = 0.025;
const CLIMATE_TRANSITION_WIDTH: f64 = 0.08;
const WETLAND_BIOME_WEIGHT: f64 = 0.30;
pub(super) const BEACH_HEIGHT_ABOVE_SEA: i32 = 2;

impl TerrainGenerator {
    pub(super) fn biome_for(&self, world_x: i32, world_z: i32, height: i32) -> Identifier {
        let sample = self.density_router().sample(world_x, world_z);
        match self.worldgen_mode {
            WorldgenMode::VanillaLike => self.vanilla_biome_for(world_x, world_z, height, sample),
            WorldgenMode::TellusLike(settings) => {
                self.tellus_biome_for(world_x, world_z, height, settings, sample)
            }
        }
    }

    pub(super) fn vanilla_biome_for(
        &self,
        world_x: i32,
        world_z: i32,
        height: i32,
        sample: TerrainSample,
    ) -> Identifier {
        let continental = sample.continentalness;
        let ridges = sample.ridges;

        if height < SEA_LEVEL - 8 {
            return self.ocean_biome_for(sample, true);
        }
        if let Some(biome) =
            self.riparian_biome_for(i64::from(height), i64::from(SEA_LEVEL), sample)
        {
            return biome;
        }
        if height < SEA_LEVEL - 1 {
            return self.ocean_biome_for(sample, false);
        }
        if continental.abs() < 0.025 && height <= SEA_LEVEL + BEACH_HEIGHT_ABOVE_SEA {
            return self.shore_biome_for(sample);
        }
        if height > 118 || ridges > 0.22 {
            return self.biomes.pick(
                &self.biomes.mountain,
                world_x,
                world_z,
                self.seed,
                0x4D4F_554E,
            );
        }
        if height < 18 {
            return self
                .biomes
                .pick(&self.biomes.cave, world_x, world_z, self.seed, 0x4341_5645);
        }
        self.climate_biome_for(world_x, world_z, sample, 0)
    }

    pub(super) fn tellus_biome_for(
        &self,
        world_x: i32,
        world_z: i32,
        height: i32,
        settings: TellusWorldgenSettings,
        sample: TerrainSample,
    ) -> Identifier {
        let sea_level = settings.sea_level;
        let height_y = i64::from(height);
        let sea_y = i64::from(sea_level);
        let land_mask = sample.continentalness;
        let mountain = sample.ridges;

        if settings.water_enabled {
            if let Some(biome) = self.riparian_biome_for(height_y, sea_y, sample) {
                return biome;
            }
            if height_y < sea_y - 18 {
                return self.ocean_biome_for(sample, true);
            }
            if height_y < sea_y - 1 {
                return self.ocean_biome_for(sample, false);
            }
        }
        // Low inland river banks retain their climate, not an ocean beach surface.
        if land_mask.abs() < 0.025 && height_y <= sea_y + i64::from(BEACH_HEIGHT_ABOVE_SEA) {
            return self.shore_biome_for(sample);
        }
        // A ridge field may cross its threshold on a low coastal shelf. Only
        // route that shelf to a rocky mountain surface once the terrain has
        // actually risen above ordinary lowland.
        if height_y > sea_y + 86 || (mountain > 0.22 && land_mask > 0.08 && height_y >= sea_y + 18)
        {
            return self.biomes.pick(
                &self.biomes.mountain,
                world_x,
                world_z,
                self.seed,
                0x544D_4F55,
            );
        }
        if height < 18 {
            return self
                .biomes
                .pick(&self.biomes.cave, world_x, world_z, self.seed, 0x5443_4156);
        }
        self.climate_biome_for(world_x, world_z, sample, 0x5400_0000)
    }

    fn ocean_biome_for(&self, sample: TerrainSample, deep: bool) -> Identifier {
        let warm = |threshold| climate_above(sample.temperature, threshold, sample.climate_domain);
        let name = if !warm(-0.25) {
            if deep {
                "deep_frozen_ocean"
            } else {
                "frozen_ocean"
            }
        } else if !warm(-0.12) {
            if deep {
                "deep_cold_ocean"
            } else {
                "cold_ocean"
            }
        } else if !warm(0.12) {
            if deep { "deep_ocean" } else { "ocean" }
        } else if deep {
            "deep_lukewarm_ocean"
        } else if warm(0.25) {
            "warm_ocean"
        } else {
            "lukewarm_ocean"
        };
        let bucket = if deep {
            &self.biomes.deep_ocean
        } else {
            &self.biomes.ocean
        };
        bucket
            .iter()
            .find(|biome| biome.path() == name)
            .or_else(|| bucket.first())
            .unwrap_or(&self.biomes.default)
            .clone()
    }

    fn shore_biome_for(&self, sample: TerrainSample) -> Identifier {
        let name = if !climate_above(sample.temperature, -0.25, sample.climate_domain) {
            "snowy_beach"
        } else if sample.erosion < 0.35 {
            "stony_shore"
        } else {
            "beach"
        };
        self.biomes
            .beach
            .iter()
            .find(|biome| biome.path() == name)
            .or_else(|| self.biomes.beach.first())
            .unwrap_or(&self.biomes.default)
            .clone()
    }

    fn riparian_biome_for(
        &self,
        height: i64,
        sea: i64,
        sample: TerrainSample,
    ) -> Option<Identifier> {
        let (river_width, minimum_land) = match self.worldgen_mode {
            WorldgenMode::VanillaLike => (RIVER_BIOME_WIDTH, -0.05),
            WorldgenMode::TellusLike(_) => (RIVER_BIOME_WIDTH * 0.65, -0.02),
        };
        let (bucket, warm_threshold, warm_name) = if sample.river.abs() < river_width
            && sample.continentalness > minimum_land
            && height < sea
        {
            (&self.biomes.river, -0.25, "river")
        } else if sample.wetland > WETLAND_BIOME_WEIGHT && (sea - 2..=sea + 1).contains(&height) {
            (&self.biomes.swamp, 0.14, "mangrove_swamp")
        } else {
            return None;
        };
        let warm = climate_above(sample.temperature, warm_threshold, sample.climate_domain);
        bucket
            .iter()
            .find(|biome| (biome.path() == warm_name) == warm)
            .or_else(|| bucket.first())
            .cloned()
    }

    fn climate_biome_for(
        &self,
        world_x: i32,
        world_z: i32,
        sample: TerrainSample,
        salt: u64,
    ) -> Identifier {
        let temperature = sample.temperature;
        let moisture = sample.moisture;
        let domain = sample.climate_domain;
        let (bucket, bucket_salt) = if !climate_above(temperature, -0.25, domain) {
            (&self.biomes.cold, 0x434F_4C44)
        } else if climate_above(temperature, 0.12, domain)
            && !climate_above(moisture, 0.08, -domain)
        {
            (&self.biomes.hot_dry, 0x484F_5444)
        } else if climate_above(temperature, 0.14, domain) && climate_above(moisture, 0.12, -domain)
        {
            (&self.biomes.jungle, 0x4A55_4E47)
        } else if climate_above(moisture, 0.12, -domain) {
            (&self.biomes.temperate_forest, 0x464F_5253)
        } else {
            (&self.biomes.grassland, 0x4752_4153)
        };
        self.biomes
            .pick(bucket, world_x, world_z, self.seed, bucket_salt ^ salt)
    }
}

fn climate_above(value: f64, threshold: f64, domain: f64) -> bool {
    if value <= threshold - CLIMATE_TRANSITION_WIDTH {
        false
    } else if value >= threshold + CLIMATE_TRANSITION_WIDTH {
        true
    } else {
        value + domain.clamp(-1.0, 1.0) * CLIMATE_TRANSITION_WIDTH > threshold
    }
}
