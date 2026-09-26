//! Protocol-neutral item-name semantics used by 26.1.2 gameplay rules.

#[must_use]
pub fn is_durability_tool_path(path: &str) -> bool {
    path.ends_with("_axe")
        || path.ends_with("_hoe")
        || path.ends_with("_pickaxe")
        || path.ends_with("_shovel")
        || path.ends_with("_sword")
}

#[must_use]
pub fn is_mining_loot_enchantable_path(path: &str) -> bool {
    path.ends_with("_pickaxe")
        || path.ends_with("_axe")
        || path.ends_with("_shovel")
        || path.ends_with("_hoe")
}

#[must_use]
pub fn equippable_player_slot(
    item_facts: &crate::item_components::ItemFactsTable,
    items: &crate::items::ItemRegistry,
    stack: &crate::ItemStack,
) -> Option<usize> {
    match item_facts
        .facts_for_stack(stack, items)?
        .equippable_slot
        .as_deref()?
    {
        "head" => Some(5),
        "chest" => Some(6),
        "legs" => Some(7),
        "feet" => Some(8),
        _ => None,
    }
}

#[must_use]
pub fn max_stack_for_stack(
    item_facts: &crate::item_components::ItemFactsTable,
    items: &crate::items::ItemRegistry,
    stack: &crate::ItemStack,
) -> i32 {
    if stack.is_empty() {
        return 1;
    }
    if let Some(model) = stack.item_model.as_deref()
        && item_facts.custom(model).is_some()
    {
        let Some(definition) = item_facts.custom_for_stack(stack, items) else {
            return 0;
        };
        if stack.damage.is_some() || definition.facts.max_damage.is_some() {
            return 1;
        }
        return definition
            .facts
            .max_stack_size
            .and_then(|value| i32::try_from(value).ok())
            .unwrap_or(64)
            .clamp(1, 64);
    }
    if stack.damage.is_some() {
        return 1;
    }
    let Some(name) = items.name_of(stack.item_id) else {
        return 64;
    };
    if let Some(max_stack) = item_facts
        .get(name)
        .and_then(|facts| facts.max_stack_size)
        .and_then(|value| i32::try_from(value).ok())
    {
        return max_stack.max(1);
    }
    let path = name.path();
    if max_tool_damage_for_path(path).is_some()
        || matches!(
            path,
            "shield"
                | "bow"
                | "crossbow"
                | "trident"
                | "fishing_rod"
                | "shears"
                | "flint_and_steel"
                | "water_bucket"
                | "lava_bucket"
        )
        || path.ends_with("_helmet")
        || path.ends_with("_chestplate")
        || path.ends_with("_leggings")
        || path.ends_with("_boots")
    {
        1
    } else if path == "bucket" {
        16
    } else {
        64
    }
}

#[must_use]
pub fn max_tool_damage_for_path(path: &str) -> Option<i32> {
    if !is_durability_tool_path(path) {
        return None;
    }
    let max = if path.starts_with("wooden_") {
        59
    } else if path.starts_with("stone_") {
        131
    } else if path.starts_with("iron_") {
        250
    } else if path.starts_with("diamond_") {
        1561
    } else if path.starts_with("golden_") {
        32
    } else if path.starts_with("netherite_") {
        2031
    } else {
        return None;
    };
    Some(max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_paths_and_vanilla_durability_are_versioned_data_rules() {
        assert!(is_durability_tool_path("diamond_pickaxe"));
        assert!(is_durability_tool_path("wooden_sword"));
        assert!(!is_durability_tool_path("shield"));
        assert!(is_mining_loot_enchantable_path("diamond_pickaxe"));
        assert!(is_mining_loot_enchantable_path("iron_hoe"));
        assert!(!is_mining_loot_enchantable_path("netherite_sword"));
        assert_eq!(max_tool_damage_for_path("wooden_axe"), Some(59));
        assert_eq!(max_tool_damage_for_path("stone_hoe"), Some(131));
        assert_eq!(max_tool_damage_for_path("iron_pickaxe"), Some(250));
        assert_eq!(max_tool_damage_for_path("diamond_shovel"), Some(1561));
        assert_eq!(max_tool_damage_for_path("golden_sword"), Some(32));
        assert_eq!(max_tool_damage_for_path("netherite_axe"), Some(2031));
        assert_eq!(max_tool_damage_for_path("copper_axe"), None);
        assert_eq!(max_tool_damage_for_path("stick"), None);
    }

    #[test]
    fn registered_item_identity_sets_stack_limit_independent_of_carrier() {
        use crate::Identifier;
        use crate::item_components::{CustomItemDefinition, ItemFacts, ItemFactsTable};
        use crate::items::{ItemRegistry, ItemReport};

        let id = |value: &str| Identifier::parse(value.to_owned()).unwrap();
        let items = ItemRegistry::from_report(&[
            ItemReport {
                id: id("minecraft:paper"),
                protocol_id: 1,
            },
            ItemReport {
                id: id("minecraft:iron_sword"),
                protocol_id: 2,
            },
        ]);
        let facts = ItemFactsTable::default()
            .with_custom_items(
                [CustomItemDefinition {
                    id: id("ruby-live:ruby"),
                    carrier: id("minecraft:paper"),
                    name: "Ruby".to_owned(),
                    crafting_ingredient: None,
                    facts: ItemFacts {
                        max_stack_size: Some(4),
                        weapon: false,
                        ..ItemFacts::default()
                    },
                }],
                &items,
            )
            .unwrap();
        let ruby_paper = crate::ItemStack::new(1, 2).with_item_model(id("ruby-live:ruby"));
        let wrong_carrier = crate::ItemStack::new(2, 1).with_item_model(id("ruby-live:ruby"));
        assert_eq!(max_stack_for_stack(&facts, &items, &ruby_paper), 4);
        assert_eq!(max_stack_for_stack(&facts, &items, &wrong_carrier), 0);
        assert_eq!(
            max_stack_for_stack(&facts, &items, &crate::ItemStack::new(1, 2)),
            64
        );
    }

    #[test]
    fn registered_head_slot_uses_identity_not_paper_carrier() {
        use crate::Identifier;
        use crate::item_components::{CustomItemDefinition, ItemFacts, ItemFactsTable};
        use crate::items::{ItemRegistry, ItemReport};

        let id = |value: &str| Identifier::parse(value.to_owned()).unwrap();
        let items = ItemRegistry::from_report(&[ItemReport {
            id: id("minecraft:paper"),
            protocol_id: 1,
        }]);
        let facts = ItemFactsTable::default()
            .with_custom_items(
                [CustomItemDefinition {
                    id: id("ruby-live:helm"),
                    carrier: id("minecraft:paper"),
                    name: "Ruby Helm".to_owned(),
                    crafting_ingredient: None,
                    facts: ItemFacts {
                        max_stack_size: Some(1),
                        equippable_slot: Some("head".to_owned()),
                        ..ItemFacts::default()
                    },
                }],
                &items,
            )
            .unwrap();
        let helm = crate::ItemStack::new(1, 1).with_item_model(id("ruby-live:helm"));
        assert_eq!(equippable_player_slot(&facts, &items, &helm), Some(5));
        assert_eq!(
            equippable_player_slot(&facts, &items, &crate::ItemStack::new(1, 1)),
            None
        );
    }
}
