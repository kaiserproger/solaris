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
    item_id: u32,
) -> Option<usize> {
    match item_facts
        .get(items.name_of(item_id)?)?
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
    if stack.is_empty() || stack.damage.is_some() {
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
}
