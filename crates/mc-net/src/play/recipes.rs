use super::*;

fn recipe_ingredients(
    recipe: &mc_data::recipes::Recipe,
) -> Option<Vec<&mc_data::recipes::Ingredient>> {
    match &recipe.kind {
        mc_data::recipes::RecipeKind::Shapeless(shapeless) => {
            Some(shapeless.ingredients.iter().collect())
        }
        mc_data::recipes::RecipeKind::Shaped(shaped) => {
            let mut ingredients = Vec::new();
            for row in &shaped.pattern {
                for ch in row.chars().filter(|ch| *ch != ' ') {
                    ingredients.push(shaped.key.get(&ch)?);
                }
            }
            Some(ingredients)
        }
        mc_data::recipes::RecipeKind::Smelting(_)
        | mc_data::recipes::RecipeKind::Blasting(_)
        | mc_data::recipes::RecipeKind::Smoking(_)
        | mc_data::recipes::RecipeKind::CampfireCooking(_) => None,
    }
}

fn matching_ingredient_slot(
    state: &InteractionState,
    available: &[i32; 46],
    ingredient: &mc_data::recipes::Ingredient,
) -> Option<usize> {
    for (slot, available_count) in available.iter().enumerate().take(45).skip(9) {
        let current = &state.inventory.slots[slot];
        if *available_count > 0
            && ingredient_accepts_item(&state.items, &state.tags, current.item_id, ingredient)
        {
            return Some(slot);
        }
    }
    None
}

pub(super) fn ingredient_accepts_item(
    items: &ItemRegistry,
    tags: &TagsData,
    item_id: u32,
    ingredient: &mc_data::recipes::Ingredient,
) -> bool {
    ingredient
        .alternatives
        .iter()
        .any(|alternative| ingredient_alternative_accepts_item(items, tags, item_id, alternative))
}

fn ingredient_alternative_accepts_item(
    items: &ItemRegistry,
    tags: &TagsData,
    item_id: u32,
    alternative: &mc_data::recipes::IngredientAlternative,
) -> bool {
    match alternative {
        mc_data::recipes::IngredientAlternative::Item(item) => items.id_of(item) == Some(item_id),
        mc_data::recipes::IngredientAlternative::Tag(tag) => {
            let item_registry = Identifier::parse("minecraft:item").expect("static identifier");
            tags.registries
                .get(&item_registry)
                .and_then(|item_tags| item_tags.get(tag))
                .is_some_and(|entries| {
                    entries
                        .iter()
                        .any(|entry| u32::try_from(*entry).ok() == Some(item_id))
                })
        }
    }
}

fn inventory_has_room_for_output(state: &InteractionState, item_id: u32, count: i32) -> bool {
    let mut remaining = count;
    let max_stack = item_max_stack(
        &state.item_facts,
        &state.items,
        &ItemStack::new(item_id, count),
    );
    for slot in 9..=44 {
        let current = &state.inventory.slots[slot];
        if current.is_empty() {
            remaining -= remaining.min(max_stack);
        } else if current.item_id == item_id
            && current.damage.is_none()
            && current.count < max_stack
        {
            remaining -= remaining.min(max_stack - current.count);
        }
        if remaining <= 0 {
            return true;
        }
    }
    false
}

fn craft_recipe_once(
    state: &mut InteractionState,
    recipe: &mc_data::recipes::Recipe,
) -> Option<Vec<(usize, ItemStack)>> {
    let ingredients = recipe_ingredients(recipe)?;
    if ingredients.is_empty() {
        return None;
    }
    let output_item_id = state.items.id_of(&recipe.result.item)?;
    let output_count = i32::try_from(recipe.result.count).ok()?;
    if output_count <= 0 || !inventory_has_room_for_output(state, output_item_id, output_count) {
        return None;
    }

    let mut available = std::array::from_fn(|slot| state.inventory.slots[slot].count.max(0));
    let mut consumed_slots = Vec::with_capacity(ingredients.len());
    for ingredient in ingredients {
        let slot = matching_ingredient_slot(state, &available, ingredient)?;
        available[slot] -= 1;
        consumed_slots.push(slot);
    }

    let mut changed = BTreeMap::new();
    for slot in consumed_slots {
        let current = &mut state.inventory.slots[slot];
        current.count -= 1;
        if current.count <= 0 {
            *current = ItemStack::EMPTY;
        }
        changed.insert(slot, current.clone());
    }

    let output = ItemStack::new(output_item_id, output_count);
    let max_stack = item_max_stack(&state.item_facts, &state.items, &output);
    let (remaining, output_changed) = state.inventory.merge_stack(output, max_stack);
    if !remaining.is_empty() {
        return None;
    }
    for (slot, stack) in output_changed {
        changed.insert(slot, stack);
    }
    Some(changed.into_iter().collect())
}

pub(super) fn craft_recipe(
    state: &mut InteractionState,
    recipe: &mc_data::recipes::Recipe,
    use_max_items: bool,
) -> Option<Vec<(usize, ItemStack)>> {
    if !use_max_items {
        return craft_recipe_once(state, recipe);
    }

    let mut all_changed = BTreeMap::new();
    while let Some(changed) = craft_recipe_once(state, recipe) {
        for (slot, stack) in changed {
            all_changed.insert(slot, stack);
        }
    }
    (!all_changed.is_empty()).then(|| all_changed.into_iter().collect())
}
