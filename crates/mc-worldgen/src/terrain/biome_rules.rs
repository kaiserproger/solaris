use mc_data::Identifier;
use mc_data::biomes::BiomeWorldgenData;

use crate::noise::fbm_2d;

use super::feature_hash;

const BIOME_PICK_WARP_SCALE: f64 = 520.0;
const BIOME_PICK_WARP_AMPLITUDE: f64 = 54.0;
const BIOME_PICK_NOISE_SCALE: f64 = 460.0;

const OVERWORLD_BIOME_IDS: &[&str] = &[
    "minecraft:mushroom_fields",
    "minecraft:deep_frozen_ocean",
    "minecraft:frozen_ocean",
    "minecraft:deep_cold_ocean",
    "minecraft:cold_ocean",
    "minecraft:deep_ocean",
    "minecraft:ocean",
    "minecraft:deep_lukewarm_ocean",
    "minecraft:lukewarm_ocean",
    "minecraft:warm_ocean",
    "minecraft:stony_shore",
    "minecraft:swamp",
    "minecraft:mangrove_swamp",
    "minecraft:snowy_slopes",
    "minecraft:snowy_plains",
    "minecraft:snowy_beach",
    "minecraft:windswept_gravelly_hills",
    "minecraft:grove",
    "minecraft:windswept_hills",
    "minecraft:snowy_taiga",
    "minecraft:windswept_forest",
    "minecraft:taiga",
    "minecraft:plains",
    "minecraft:meadow",
    "minecraft:beach",
    "minecraft:forest",
    "minecraft:old_growth_spruce_taiga",
    "minecraft:flower_forest",
    "minecraft:birch_forest",
    "minecraft:dark_forest",
    "minecraft:pale_garden",
    "minecraft:savanna_plateau",
    "minecraft:savanna",
    "minecraft:jungle",
    "minecraft:badlands",
    "minecraft:desert",
    "minecraft:wooded_badlands",
    "minecraft:jagged_peaks",
    "minecraft:stony_peaks",
    "minecraft:frozen_river",
    "minecraft:river",
    "minecraft:ice_spikes",
    "minecraft:old_growth_pine_taiga",
    "minecraft:sunflower_plains",
    "minecraft:old_growth_birch_forest",
    "minecraft:sparse_jungle",
    "minecraft:bamboo_jungle",
    "minecraft:eroded_badlands",
    "minecraft:windswept_savanna",
    "minecraft:cherry_grove",
    "minecraft:frozen_peaks",
    "minecraft:dripstone_caves",
    "minecraft:lush_caves",
    "minecraft:deep_dark",
];

#[derive(Clone)]
pub struct BiomeRules {
    pub(super) default: Identifier,
    pub(super) all: Vec<Identifier>,
    pub(super) deep_ocean: Vec<Identifier>,
    pub(super) ocean: Vec<Identifier>,
    pub(super) beach: Vec<Identifier>,
    pub(super) river: Vec<Identifier>,
    pub(super) swamp: Vec<Identifier>,
    pub(super) cold: Vec<Identifier>,
    pub(super) temperate_forest: Vec<Identifier>,
    pub(super) grassland: Vec<Identifier>,
    pub(super) hot_dry: Vec<Identifier>,
    pub(super) mountain: Vec<Identifier>,
    pub(super) jungle: Vec<Identifier>,
    pub(super) cave: Vec<Identifier>,
}

impl BiomeRules {
    #[must_use]
    pub fn vanilla_overworld() -> Self {
        let ids = |names: &[&str]| -> Vec<Identifier> {
            names
                .iter()
                .map(|name| Identifier::parse(*name).expect("static biome identifier"))
                .collect()
        };
        Self {
            default: Identifier::parse("minecraft:plains").expect("static biome identifier"),
            all: ids(OVERWORLD_BIOME_IDS),
            deep_ocean: ids(&[
                "minecraft:deep_frozen_ocean",
                "minecraft:deep_cold_ocean",
                "minecraft:deep_ocean",
                "minecraft:deep_lukewarm_ocean",
            ]),
            ocean: ids(&[
                "minecraft:frozen_ocean",
                "minecraft:cold_ocean",
                "minecraft:ocean",
                "minecraft:lukewarm_ocean",
                "minecraft:warm_ocean",
            ]),
            beach: ids(&[
                "minecraft:beach",
                "minecraft:snowy_beach",
                "minecraft:stony_shore",
            ]),
            river: ids(&["minecraft:river", "minecraft:frozen_river"]),
            swamp: ids(&["minecraft:swamp", "minecraft:mangrove_swamp"]),
            cold: ids(&[
                "minecraft:snowy_plains",
                "minecraft:snowy_taiga",
                "minecraft:taiga",
                "minecraft:old_growth_pine_taiga",
                "minecraft:old_growth_spruce_taiga",
                "minecraft:ice_spikes",
                "minecraft:grove",
            ]),
            temperate_forest: ids(&[
                "minecraft:forest",
                "minecraft:flower_forest",
                "minecraft:birch_forest",
                "minecraft:old_growth_birch_forest",
                "minecraft:dark_forest",
                "minecraft:pale_garden",
                "minecraft:windswept_forest",
                "minecraft:cherry_grove",
            ]),
            grassland: ids(&[
                "minecraft:plains",
                "minecraft:sunflower_plains",
                "minecraft:meadow",
                "minecraft:mushroom_fields",
            ]),
            hot_dry: ids(&[
                "minecraft:desert",
                "minecraft:savanna",
                "minecraft:savanna_plateau",
                "minecraft:windswept_savanna",
                "minecraft:badlands",
                "minecraft:wooded_badlands",
                "minecraft:eroded_badlands",
            ]),
            mountain: ids(&[
                "minecraft:jagged_peaks",
                "minecraft:frozen_peaks",
                "minecraft:stony_peaks",
                "minecraft:snowy_slopes",
                "minecraft:windswept_hills",
                "minecraft:windswept_gravelly_hills",
            ]),
            jungle: ids(&[
                "minecraft:jungle",
                "minecraft:sparse_jungle",
                "minecraft:bamboo_jungle",
            ]),
            cave: ids(&[
                "minecraft:dripstone_caves",
                "minecraft:lush_caves",
                "minecraft:deep_dark",
            ]),
        }
    }

    #[must_use]
    pub fn from_worldgen_data(data: &BiomeWorldgenData) -> Option<Self> {
        let fallback = Self::vanilla_overworld();
        let tag = |name: &str| -> Vec<Identifier> {
            data.tag(&Identifier::parse(format!("minecraft:{name}")).expect("static biome tag"))
                .to_vec()
        };
        let by_name = |names: &[&str]| -> Vec<Identifier> {
            names
                .iter()
                .filter_map(|name| Identifier::parse(format!("minecraft:{name}")).ok())
                .filter(|id| data.biomes().any(|biome| biome == id))
                .collect()
        };
        let all = tag("is_overworld");
        if all.is_empty() {
            return None;
        }
        let deep_ocean = tag("is_deep_ocean");
        let ocean = without(tag("is_ocean"), &deep_ocean);
        Some(Self {
            default: Identifier::parse("minecraft:plains").expect("static biome identifier"),
            all,
            deep_ocean: or_fallback(deep_ocean, &fallback.deep_ocean),
            ocean: or_fallback(ocean, &fallback.ocean),
            beach: or_fallback(
                union([tag("is_beach"), by_name(&["stony_shore"])]),
                &fallback.beach,
            ),
            river: or_fallback(tag("is_river"), &fallback.river),
            swamp: or_fallback(by_name(&["swamp", "mangrove_swamp"]), &fallback.swamp),
            cold: or_fallback(
                union([
                    tag("is_taiga"),
                    by_name(&["snowy_plains", "ice_spikes", "grove"]),
                ]),
                &fallback.cold,
            ),
            temperate_forest: or_fallback(
                union([tag("is_forest"), by_name(&["cherry_grove", "pale_garden"])]),
                &fallback.temperate_forest,
            ),
            grassland: or_fallback(
                by_name(&["plains", "sunflower_plains", "meadow", "mushroom_fields"]),
                &fallback.grassland,
            ),
            hot_dry: or_fallback(
                union([tag("is_badlands"), tag("is_savanna"), by_name(&["desert"])]),
                &fallback.hot_dry,
            ),
            mountain: or_fallback(
                union([tag("is_mountain"), tag("is_hill")]),
                &fallback.mountain,
            ),
            jungle: or_fallback(tag("is_jungle"), &fallback.jungle),
            cave: or_fallback(
                by_name(&["dripstone_caves", "lush_caves", "deep_dark"]),
                &fallback.cave,
            ),
        })
    }

    pub(super) fn pick(&self, bucket: &[Identifier], x: i32, z: i32, salt: u64) -> Identifier {
        if bucket.is_empty() {
            return self.default.clone();
        }
        if bucket.len() == 1 {
            return bucket[0].clone();
        }

        let warp_x = fbm_2d(
            x as f64 / BIOME_PICK_WARP_SCALE,
            z as f64 / BIOME_PICK_WARP_SCALE,
            (salt ^ 0x4257_4152_5058) as i64,
            2,
            0.5,
        ) * BIOME_PICK_WARP_AMPLITUDE;
        let warp_z = fbm_2d(
            x as f64 / BIOME_PICK_WARP_SCALE,
            z as f64 / BIOME_PICK_WARP_SCALE,
            (salt ^ 0x4257_4152_505A) as i64,
            2,
            0.5,
        ) * BIOME_PICK_WARP_AMPLITUDE;
        let value = fbm_2d(
            (x as f64 + warp_x) / BIOME_PICK_NOISE_SCALE,
            (z as f64 + warp_z) / BIOME_PICK_NOISE_SCALE,
            salt as i64,
            3,
            0.55,
        );
        let band = (((value + 1.0) * 0.5).clamp(0.0, 0.999_999) * bucket.len() as f64) as usize;
        let offset = feature_hash(0, bucket.len() as i32, 0, 0, salt) as usize % bucket.len();
        let idx = (band + offset) % bucket.len();
        bucket[idx].clone()
    }

    pub(super) fn is_ocean(&self, biome: &Identifier) -> bool {
        self.ocean.contains(biome) || self.deep_ocean.contains(biome)
    }

    pub(super) fn is_beach_or_shore(&self, biome: &Identifier) -> bool {
        self.beach.contains(biome)
    }

    pub(super) fn is_river(&self, biome: &Identifier) -> bool {
        self.river.contains(biome)
    }

    pub(super) fn is_surface_water(&self, biome: &Identifier) -> bool {
        self.is_ocean(biome) || self.is_river(biome)
    }
}

fn or_fallback(values: Vec<Identifier>, fallback: &[Identifier]) -> Vec<Identifier> {
    if values.is_empty() {
        fallback.to_vec()
    } else {
        values
    }
}

fn union<const N: usize>(lists: [Vec<Identifier>; N]) -> Vec<Identifier> {
    let mut out = Vec::new();
    for id in lists.into_iter().flatten() {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

fn without(mut values: Vec<Identifier>, excluded: &[Identifier]) -> Vec<Identifier> {
    values.retain(|id| !excluded.contains(id));
    values
}

#[cfg(test)]
#[path = "biome_rules_tests.rs"]
mod tests;
