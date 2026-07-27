//! Data-driven vanilla villager trade specs for Java Edition 26.1.2.
//!
//! The first supported profession slice is the novice toolsmith catalog used by
//! the generated village vertical. Values come from the local 26.1.2
//! `VillagerTrades` oracle and remain protocol-neutral until registry resolution.

use crate::Identifier;

pub const TOOLSMITH_JOB_SITE_26_1_2: &str = "minecraft:smithing_table";

#[must_use]
pub fn supported_profession_for_job_site_26_1_2(block: &Identifier) -> Option<&'static str> {
    (block.as_str() == TOOLSMITH_JOB_SITE_26_1_2).then_some("toolsmith")
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
    fn supported_job_site_mapping_is_exact_and_fail_closed() {
        assert_eq!(
            supported_profession_for_job_site_26_1_2(
                &Identifier::parse("minecraft:smithing_table").unwrap()
            ),
            Some("toolsmith")
        );
        assert_eq!(
            supported_profession_for_job_site_26_1_2(
                &Identifier::parse("minecraft:blast_furnace").unwrap()
            ),
            None
        );
    }
}
