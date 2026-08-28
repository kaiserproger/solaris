//! Protocol-neutral block-name semantics used by 26.1.2 gameplay adapters.

#[must_use]
pub fn passive_herd_fallback_surface_name(name: &str) -> bool {
    matches!(
        name,
        "minecraft:dirt"
            | "minecraft:coarse_dirt"
            | "minecraft:podzol"
            | "minecraft:sand"
            | "minecraft:red_sand"
            | "minecraft:snow_block"
            | "minecraft:moss_block"
            | "minecraft:mycelium"
    )
}

#[must_use]
pub fn passable_block_name(name: &str) -> bool {
    matches!(
        name,
        "minecraft:air"
            | "minecraft:short_grass"
            | "minecraft:tall_grass"
            | "minecraft:short_dry_grass"
            | "minecraft:tall_dry_grass"
            | "minecraft:fern"
            | "minecraft:large_fern"
            | "minecraft:dead_bush"
            | "minecraft:bush"
            | "minecraft:firefly_bush"
            | "minecraft:dandelion"
            | "minecraft:poppy"
            | "minecraft:blue_orchid"
            | "minecraft:allium"
            | "minecraft:azure_bluet"
            | "minecraft:red_tulip"
            | "minecraft:orange_tulip"
            | "minecraft:white_tulip"
            | "minecraft:pink_tulip"
            | "minecraft:oxeye_daisy"
            | "minecraft:cornflower"
            | "minecraft:lily_of_the_valley"
            | "minecraft:wither_rose"
            | "minecraft:torchflower"
            | "minecraft:open_eyeblossom"
            | "minecraft:closed_eyeblossom"
            | "minecraft:sunflower"
            | "minecraft:lilac"
            | "minecraft:rose_bush"
            | "minecraft:peony"
            | "minecraft:pitcher_plant"
            | "minecraft:pink_petals"
            | "minecraft:wildflowers"
            | "minecraft:sugar_cane"
            | "minecraft:wheat"
            | "minecraft:carrots"
            | "minecraft:potatoes"
            | "minecraft:beetroots"
            | "minecraft:torchflower_crop"
            | "minecraft:pitcher_crop"
            | "minecraft:melon_stem"
            | "minecraft:attached_melon_stem"
            | "minecraft:pumpkin_stem"
            | "minecraft:attached_pumpkin_stem"
            | "minecraft:sweet_berry_bush"
            | "minecraft:nether_wart"
            | "minecraft:kelp"
            | "minecraft:kelp_plant"
            | "minecraft:seagrass"
            | "minecraft:tall_seagrass"
            | "minecraft:bubble_column"
            | "minecraft:torch"
            | "minecraft:wall_torch"
            | "minecraft:soul_torch"
            | "minecraft:soul_wall_torch"
            | "minecraft:redstone_torch"
            | "minecraft:redstone_wall_torch"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passable_names_cover_plants_crops_and_torches_but_not_solids() {
        for name in [
            "minecraft:air",
            "minecraft:blue_orchid",
            "minecraft:wheat",
            "minecraft:kelp",
            "minecraft:redstone_wall_torch",
        ] {
            assert!(passable_block_name(name), "{name}");
        }
        for name in [
            "minecraft:stone",
            "minecraft:flower_pot",
            "minecraft:oak_planks",
        ] {
            assert!(!passable_block_name(name), "{name}");
        }
    }

    #[test]
    fn passive_herd_fallbacks_are_natural_generated_surfaces_only() {
        for name in [
            "minecraft:dirt",
            "minecraft:coarse_dirt",
            "minecraft:podzol",
            "minecraft:sand",
            "minecraft:red_sand",
            "minecraft:snow_block",
            "minecraft:moss_block",
            "minecraft:mycelium",
        ] {
            assert!(passive_herd_fallback_surface_name(name), "{name}");
        }
        for name in [
            "minecraft:stone",
            "minecraft:oak_planks",
            "minecraft:oak_leaves",
        ] {
            assert!(!passive_herd_fallback_surface_name(name), "{name}");
        }
    }
}
