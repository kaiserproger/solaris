//! Protocol-neutral enchanting selection rules for Java Edition 26.1.2.

use crate::Identifier;
use crate::item_components::ItemFactsTable;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnchantingOffer {
    pub button_id: i32,
    pub required_level: i32,
    pub lapis_cost: i32,
    pub enchantment_level: i32,
}

#[must_use]
pub fn enchanting_offer(bookshelf_count: u8, button_id: i32) -> Option<EnchantingOffer> {
    match button_id {
        0 => Some(EnchantingOffer {
            button_id,
            required_level: 1,
            lapis_cost: 1,
            enchantment_level: 1,
        }),
        1 if bookshelf_count >= 5 => Some(EnchantingOffer {
            button_id,
            required_level: 10,
            lapis_cost: 2,
            enchantment_level: 2,
        }),
        2 if bookshelf_count >= 15 => Some(EnchantingOffer {
            button_id,
            required_level: 30,
            lapis_cost: 3,
            enchantment_level: 3,
        }),
        _ => None,
    }
}

#[must_use]
pub fn item_is_efficiency_enchantable(item_facts: &ItemFactsTable, item: &Identifier) -> bool {
    item_facts
        .get(item)
        .and_then(|facts| facts.tool.as_ref())
        .is_some()
        || crate::item_semantics_26_1_2::is_mining_loot_enchantable_path(item.path())
        || item.path() == "shears"
}

#[must_use]
pub fn supported_enchantment_for_item(
    item_facts: &ItemFactsTable,
    item: &Identifier,
) -> Option<Identifier> {
    let enchantment = if item.path().ends_with("_sword") {
        "minecraft:sharpness"
    } else if crate::armor::builtin().entry(item).is_some() {
        "minecraft:protection"
    } else if item_is_efficiency_enchantable(item_facts, item) {
        "minecraft:efficiency"
    } else {
        return None;
    };
    Some(Identifier::parse(enchantment).expect("static enchantment identifier"))
}

#[must_use]
pub fn additional_enchantment_for_offer(
    item: &Identifier,
    button_id: i32,
) -> Option<(Identifier, i32)> {
    if !crate::item_semantics_26_1_2::is_mining_loot_enchantable_path(item.path()) {
        return None;
    }
    let (enchantment, level) = match button_id {
        1 => ("minecraft:fortune", 2),
        2 => ("minecraft:silk_touch", 1),
        _ => return None,
    };
    Some((
        Identifier::parse(enchantment).expect("static enchantment identifier"),
        level,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_tiers_follow_bookshelf_thresholds() {
        assert_eq!(
            enchanting_offer(0, 0),
            Some(EnchantingOffer {
                button_id: 0,
                required_level: 1,
                lapis_cost: 1,
                enchantment_level: 1,
            })
        );
        assert!(enchanting_offer(4, 1).is_none());
        assert_eq!(enchanting_offer(5, 1).unwrap().required_level, 10);
        assert!(enchanting_offer(14, 2).is_none());
        assert_eq!(enchanting_offer(15, 2).unwrap().required_level, 30);
    }

    #[test]
    fn item_selection_covers_swords_armor_tools_and_extra_loot_enchantments() {
        let facts = ItemFactsTable::default();
        let sword = Identifier::parse("minecraft:diamond_sword").unwrap();
        let helmet = Identifier::parse("minecraft:diamond_helmet").unwrap();
        let pickaxe = Identifier::parse("minecraft:diamond_pickaxe").unwrap();
        assert_eq!(
            supported_enchantment_for_item(&facts, &sword)
                .unwrap()
                .as_str(),
            "minecraft:sharpness"
        );
        assert_eq!(
            supported_enchantment_for_item(&facts, &helmet)
                .unwrap()
                .as_str(),
            "minecraft:protection"
        );
        assert_eq!(
            supported_enchantment_for_item(&facts, &pickaxe)
                .unwrap()
                .as_str(),
            "minecraft:efficiency"
        );
        assert_eq!(
            additional_enchantment_for_offer(&pickaxe, 1)
                .unwrap()
                .0
                .as_str(),
            "minecraft:fortune"
        );
        assert_eq!(
            additional_enchantment_for_offer(&pickaxe, 2)
                .unwrap()
                .0
                .as_str(),
            "minecraft:silk_touch"
        );
        assert!(additional_enchantment_for_offer(&sword, 2).is_none());
    }
}
