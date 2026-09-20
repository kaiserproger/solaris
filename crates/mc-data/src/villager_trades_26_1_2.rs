//! Data-driven vanilla villager trade specs for Java Edition 26.1.2.
//!
//! The first supported profession slice is the novice toolsmith catalog used by
//! the generated village vertical. Values come from the local 26.1.2
//! `VillagerTrades` oracle and remain protocol-neutral until registry resolution.

use crate::Identifier;

pub const TOOLSMITH_JOB_SITE_26_1_2: &str = "minecraft:smithing_table";

/// 26.1.2 job-site POI block to `villager_profession` registry name. Every
/// vanilla 26.1.2 job site maps to its profession; unknown blocks fail
/// closed so unsupported job sites never assign a profession.
#[must_use]
pub fn supported_profession_for_job_site_26_1_2(block: &Identifier) -> Option<&'static str> {
    match block.as_str() {
        "minecraft:composter" => Some("farmer"),
        "minecraft:barrel" => Some("fisherman"),
        "minecraft:cartography_table" => Some("cartographer"),
        "minecraft:brewing_stand" => Some("cleric"),
        "minecraft:lectern" => Some("librarian"),
        "minecraft:fletching_table" => Some("fletcher"),
        "minecraft:loom" => Some("shepherd"),
        "minecraft:smithing_table" => Some("toolsmith"),
        "minecraft:blast_furnace" => Some("armorer"),
        "minecraft:grindstone" => Some("weaponsmith"),
        "minecraft:smoker" => Some("butcher"),
        "minecraft:cauldron" => Some("leatherworker"),
        "minecraft:stonecutter" => Some("mason"),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VillagerTradeCostSpec {
    pub item: Identifier,
    pub count: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VillagerTradeOfferSpec {
    pub key: &'static str,
    pub cost_a: VillagerTradeCostSpec,
    pub cost_b: Option<VillagerTradeCostSpec>,
    pub result_item: Identifier,
    pub result_count: i32,
    pub max_uses: i32,
    pub xp: i32,
    pub price_multiplier: f32,
}

#[must_use]
pub fn toolsmith_novice_offers_26_1_2() -> Vec<VillagerTradeOfferSpec> {
    vec![
        // Vanilla names the shared smith offer `smith/1/coal_emerald` and
        // includes it in toolsmith level 1 through the `common_smith` trade tag;
        // it is intentionally not under the `toolsmith/` namespace.
        offer(
            "minecraft:smith/1/coal_emerald",
            ("minecraft:coal", 15),
            ("minecraft:emerald", 1),
            (16, 2, 0.05),
        ),
        offer(
            "minecraft:toolsmith/1/emerald_stone_axe",
            ("minecraft:emerald", 1),
            ("minecraft:stone_axe", 1),
            (12, 1, 0.2),
        ),
        offer(
            "minecraft:toolsmith/1/emerald_stone_shovel",
            ("minecraft:emerald", 1),
            ("minecraft:stone_shovel", 1),
            (12, 1, 0.2),
        ),
        offer(
            "minecraft:toolsmith/1/emerald_stone_pickaxe",
            ("minecraft:emerald", 1),
            ("minecraft:stone_pickaxe", 1),
            (12, 1, 0.2),
        ),
        offer(
            "minecraft:toolsmith/1/emerald_stone_hoe",
            ("minecraft:emerald", 1),
            ("minecraft:stone_hoe", 1),
            (12, 1, 0.2),
        ),
    ]
}

fn offer(
    key: &'static str,
    cost: (&str, i32),
    result: (&str, i32),
    policy: (i32, i32, f32),
) -> VillagerTradeOfferSpec {
    let (cost_item, cost_count) = cost;
    let (result_item, result_count) = result;
    let (max_uses, xp, price_multiplier) = policy;
    VillagerTradeOfferSpec {
        key,
        cost_a: VillagerTradeCostSpec {
            item: Identifier::parse(cost_item).expect("static trade cost identifier"),
            count: cost_count,
        },
        cost_b: None,
        result_item: Identifier::parse(result_item).expect("static trade result identifier"),
        result_count,
        max_uses,
        xp,
        price_multiplier,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn novice_toolsmith_catalog_matches_local_26_1_2_oracle() {
        let offers = toolsmith_novice_offers_26_1_2();
        assert_eq!(offers.len(), 5);
        assert_eq!(offers[0].key, "minecraft:smith/1/coal_emerald");
        assert_eq!(offers[0].cost_a.item.as_str(), "minecraft:coal");
        assert_eq!(offers[0].cost_a.count, 15);
        assert_eq!(offers[0].result_item.as_str(), "minecraft:emerald");
        assert_eq!(offers[0].max_uses, 16);
        assert_eq!(offers[0].xp, 2);
        assert_eq!(offers[0].price_multiplier, 0.05);
        assert!(offers[1..].iter().all(|offer| {
            offer.cost_a.item.as_str() == "minecraft:emerald"
                && offer.cost_a.count == 1
                && offer.result_count == 1
                && offer.max_uses == 12
                && offer.xp == 1
                && offer.price_multiplier == 0.2
        }));
    }

    #[test]
    fn every_vanilla_job_site_maps_to_its_registry_profession() {
        let cases = [
            ("minecraft:composter", "farmer"),
            ("minecraft:barrel", "fisherman"),
            ("minecraft:cartography_table", "cartographer"),
            ("minecraft:brewing_stand", "cleric"),
            ("minecraft:lectern", "librarian"),
            ("minecraft:fletching_table", "fletcher"),
            ("minecraft:loom", "shepherd"),
            ("minecraft:smithing_table", "toolsmith"),
            ("minecraft:blast_furnace", "armorer"),
            ("minecraft:grindstone", "weaponsmith"),
            ("minecraft:smoker", "butcher"),
            ("minecraft:cauldron", "leatherworker"),
            ("minecraft:stonecutter", "mason"),
        ];
        for (job_site, profession) in cases {
            assert_eq!(
                supported_profession_for_job_site_26_1_2(&Identifier::parse(job_site).unwrap()),
                Some(profession),
                "{job_site}"
            );
        }
        // Non-job-site blocks fail closed.
        assert_eq!(
            supported_profession_for_job_site_26_1_2(
                &Identifier::parse("minecraft:stone").unwrap()
            ),
            None
        );
    }
}
