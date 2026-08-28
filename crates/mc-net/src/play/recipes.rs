use super::*;
use mc_protocol::packets::play::{
    ClientboundRecipeBookAdd, ClientboundUpdateRecipes, RecipeBookDisplay, RecipeBookEntry,
    RecipeBookIngredient, RecipeBookSlotDisplay, StonecutterRecipeEntry,
};

pub(super) fn initial_recipe_update(
    recipes: &[mc_data::recipes::Recipe],
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
) -> ClientboundUpdateRecipes {
    let stonecutter_recipes = recipes
        .iter()
        .filter_map(|recipe| stonecutter_recipe_entry(recipe, items, item_facts))
        .collect();
    ClientboundUpdateRecipes {
        item_sets: Vec::new(),
        stonecutter_recipes,
    }
}

pub(super) fn stonecutter_recipe_entry(
    recipe: &mc_data::recipes::Recipe,
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
) -> Option<StonecutterRecipeEntry> {
    let mc_data::recipes::RecipeKind::Stonecutting(stonecutting) = &recipe.kind else {
        return None;
    };
    if recipe.result.item.as_str() == "minecraft:air" {
        return None;
    }
    let result_item_id = items.id_of(&recipe.result.item)?;
    let item_id = i32::try_from(result_item_id).ok()?;
    let count = i32::try_from(recipe.result.count).ok()?;
    let result = ItemStack::new(result_item_id, count);
    if count <= 0 || count > item_max_stack(item_facts, items, &result) {
        return None;
    }
    Some(StonecutterRecipeEntry {
        input: recipe_book_requirement(&stonecutting.ingredient, items)?,
        result: RecipeBookSlotDisplay::ItemStack { item_id, count },
    })
}

pub(super) fn initial_recipe_book(
    recipes: &[mc_data::recipes::Recipe],
    items: &ItemRegistry,
) -> ClientboundRecipeBookAdd {
    let entries = recipes
        .iter()
        .enumerate()
        .filter_map(|(display_id, recipe)| recipe_book_entry(display_id, recipe, items))
        .collect();
    ClientboundRecipeBookAdd {
        entries,
        replace: true,
    }
}

fn recipe_book_entry(
    display_id: usize,
    recipe: &mc_data::recipes::Recipe,
    items: &ItemRegistry,
) -> Option<RecipeBookEntry> {
    let display_id = i32::try_from(display_id).ok()?;
    let result_item_id = i32::try_from(items.id_of(&recipe.result.item)?).ok()?;
    let result_count = i32::try_from(recipe.result.count).ok()?;
    if result_count <= 0 {
        return None;
    }
    let result = RecipeBookSlotDisplay::ItemStack {
        item_id: result_item_id,
        count: result_count,
    };

    let (display, category_id, crafting_requirements) = match &recipe.kind {
        mc_data::recipes::RecipeKind::Shapeless(shapeless) => {
            let ingredients = shapeless
                .ingredients
                .iter()
                .map(|ingredient| recipe_book_slot(ingredient, items))
                .collect::<Option<Vec<_>>>()?;
            let requirements = recipe_book_requirements(&shapeless.ingredients, items);
            let crafting_station = named_recipe_book_item(items, "minecraft:crafting_table")?;
            (
                RecipeBookDisplay::Shapeless {
                    ingredients,
                    result,
                    crafting_station,
                },
                3,
                requirements,
            )
        }
        mc_data::recipes::RecipeKind::Shaped(shaped) => {
            let height = i32::try_from(shaped.pattern.len()).ok()?;
            let width = i32::try_from(shaped.pattern.first()?.chars().count()).ok()?;
            if width <= 0 || height <= 0 {
                return None;
            }
            let mut ingredients = Vec::with_capacity((width * height) as usize);
            let mut requirement_sources = Vec::new();
            for row in &shaped.pattern {
                if i32::try_from(row.chars().count()).ok()? != width {
                    return None;
                }
                for key in row.chars() {
                    if key == ' ' {
                        ingredients.push(RecipeBookSlotDisplay::Empty);
                    } else {
                        let ingredient = shaped.key.get(&key)?;
                        ingredients.push(recipe_book_slot(ingredient, items)?);
                        requirement_sources.push(ingredient.clone());
                    }
                }
            }
            let requirements = recipe_book_requirements(&requirement_sources, items);
            let crafting_station = named_recipe_book_item(items, "minecraft:crafting_table")?;
            (
                RecipeBookDisplay::Shaped {
                    width,
                    height,
                    ingredients,
                    result,
                    crafting_station,
                },
                3,
                requirements,
            )
        }
        mc_data::recipes::RecipeKind::Smelting(cooking) => (
            recipe_book_cooking_display(cooking, result, items, "minecraft:furnace")?,
            6,
            None,
        ),
        mc_data::recipes::RecipeKind::Blasting(cooking) => (
            recipe_book_cooking_display(cooking, result, items, "minecraft:blast_furnace")?,
            8,
            None,
        ),
        mc_data::recipes::RecipeKind::Smoking(cooking) => (
            recipe_book_cooking_display(cooking, result, items, "minecraft:smoker")?,
            9,
            None,
        ),
        mc_data::recipes::RecipeKind::CampfireCooking(cooking) => (
            recipe_book_cooking_display(cooking, result, items, "minecraft:campfire")?,
            12,
            None,
        ),
        mc_data::recipes::RecipeKind::Stonecutting(_) => return None,
    };

    Some(RecipeBookEntry {
        display_id,
        display,
        group: None,
        category_id,
        crafting_requirements,
        flags: 0,
    })
}

fn named_recipe_book_item(items: &ItemRegistry, name: &str) -> Option<RecipeBookSlotDisplay> {
    let name = Identifier::parse(name).ok()?;
    let item_id = i32::try_from(items.id_of(&name)?).ok()?;
    Some(RecipeBookSlotDisplay::Item { item_id })
}

fn recipe_book_slot(
    ingredient: &mc_data::recipes::Ingredient,
    items: &ItemRegistry,
) -> Option<RecipeBookSlotDisplay> {
    let mut alternatives = ingredient
        .alternatives
        .iter()
        .map(|alternative| match alternative {
            mc_data::recipes::IngredientAlternative::Item(item) => items
                .id_of(item)
                .and_then(|item_id| i32::try_from(item_id).ok())
                .map(|item_id| RecipeBookSlotDisplay::Item { item_id }),
            mc_data::recipes::IngredientAlternative::Tag(tag) => {
                Some(RecipeBookSlotDisplay::Tag(tag.clone()))
            }
        })
        .collect::<Option<Vec<_>>>()?;
    match alternatives.len() {
        0 => None,
        1 => alternatives.pop(),
        _ => Some(RecipeBookSlotDisplay::Composite(alternatives)),
    }
}

fn recipe_book_requirements(
    ingredients: &[mc_data::recipes::Ingredient],
    items: &ItemRegistry,
) -> Option<Vec<RecipeBookIngredient>> {
    ingredients
        .iter()
        .map(|ingredient| recipe_book_requirement(ingredient, items))
        .collect()
}

fn recipe_book_requirement(
    ingredient: &mc_data::recipes::Ingredient,
    items: &ItemRegistry,
) -> Option<RecipeBookIngredient> {
    if let [mc_data::recipes::IngredientAlternative::Tag(tag)] = ingredient.alternatives.as_slice()
    {
        return Some(RecipeBookIngredient::Tag(tag.clone()));
    }
    let item_ids = ingredient
        .alternatives
        .iter()
        .map(|alternative| match alternative {
            mc_data::recipes::IngredientAlternative::Item(item) => {
                items.id_of(item).and_then(|id| i32::try_from(id).ok())
            }
            mc_data::recipes::IngredientAlternative::Tag(_) => None,
        })
        .collect::<Option<Vec<_>>>()?;
    (!item_ids.is_empty()).then_some(RecipeBookIngredient::Items(item_ids))
}

fn recipe_book_cooking_display(
    recipe: &mc_data::recipes::SmeltingRecipe,
    result: RecipeBookSlotDisplay,
    items: &ItemRegistry,
    station: &str,
) -> Option<RecipeBookDisplay> {
    Some(RecipeBookDisplay::Furnace {
        ingredient: recipe_book_slot(&recipe.ingredient, items)?,
        fuel: RecipeBookSlotDisplay::AnyFuel,
        result,
        crafting_station: named_recipe_book_item(items, station)?,
        duration: i32::try_from(recipe.cooking_time).ok()?,
        // Recipe JSON loaded by mc-data does not yet retain cooking XP.
        experience: 0.0,
    })
}

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
        | mc_data::recipes::RecipeKind::CampfireCooking(_)
        | mc_data::recipes::RecipeKind::Stonecutting(_) => None,
    }
}

pub(super) fn recipe_fits_grid(
    recipe: &mc_data::recipes::Recipe,
    width: usize,
    height: usize,
) -> bool {
    match &recipe.kind {
        mc_data::recipes::RecipeKind::Shapeless(shapeless) => {
            shapeless.ingredients.len() <= width.saturating_mul(height)
        }
        mc_data::recipes::RecipeKind::Shaped(shaped) => {
            let recipe_height = shaped.pattern.len();
            let recipe_width = shaped
                .pattern
                .iter()
                .map(|row| row.chars().count())
                .max()
                .unwrap_or(0);
            recipe_width <= width && recipe_height <= height
        }
        mc_data::recipes::RecipeKind::Smelting(_)
        | mc_data::recipes::RecipeKind::Blasting(_)
        | mc_data::recipes::RecipeKind::Smoking(_)
        | mc_data::recipes::RecipeKind::CampfireCooking(_)
        | mc_data::recipes::RecipeKind::Stonecutting(_) => false,
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
    mc_data::recipes::ingredient_accepts_item(items, tags, item_id, ingredient)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CraftedItem {
    pub(super) item_id: u32,
    pub(super) count: u64,
    pub(super) craft_count: u32,
}

impl CraftedItem {
    pub(super) fn from_single_result(result: &ItemStack) -> Option<Self> {
        if result.is_empty() {
            return None;
        }
        Some(Self {
            item_id: result.item_id,
            count: u64::try_from(result.count).ok()?,
            craft_count: 1,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CraftRecipeOutcome {
    pub(super) changed_slots: Vec<(usize, ItemStack)>,
    pub(super) crafted: CraftedItem,
}

pub(super) fn craft_recipe(
    state: &mut InteractionState,
    recipe: &mc_data::recipes::Recipe,
    use_max_items: bool,
) -> Option<CraftRecipeOutcome> {
    let item_id = state.items.id_of(&recipe.result.item)?;
    if !use_max_items {
        return Some(CraftRecipeOutcome {
            changed_slots: craft_recipe_once(state, recipe)?,
            crafted: CraftedItem {
                item_id,
                count: u64::from(recipe.result.count),
                craft_count: 1,
            },
        });
    }

    let max_crafts = state.inventory.slots[9..=44]
        .iter()
        .map(|stack| {
            let max_stack = item_max_stack(&state.item_facts, &state.items, stack);
            stack.count.clamp(0, max_stack) as usize
        })
        .fold(0usize, usize::saturating_add);
    let mut all_changed = BTreeMap::new();
    let mut craft_count = 0_u32;
    for _ in 0..max_crafts {
        let Some(changed) = craft_recipe_once(state, recipe) else {
            break;
        };
        craft_count += 1;
        for (slot, stack) in changed {
            all_changed.insert(slot, stack);
        }
    }
    let count = u64::from(recipe.result.count) * u64::from(craft_count);
    (!all_changed.is_empty()).then(|| CraftRecipeOutcome {
        changed_slots: all_changed.into_iter().collect(),
        crafted: CraftedItem {
            item_id,
            count,
            craft_count,
        },
    })
}

#[cfg(test)]
mod tests {
    use mc_data::items::{ItemRegistry, ItemReport};
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, ShapedRecipe,
        ShapelessRecipe, SmeltingRecipe, StonecuttingRecipe,
    };
    use mc_protocol::packets::play::{
        RecipeBookDisplay, RecipeBookIngredient, RecipeBookSlotDisplay, StonecutterRecipeEntry,
    };

    use super::*;

    fn id(value: &str) -> Identifier {
        Identifier::parse(value).unwrap()
    }

    fn item(name: &str, protocol_id: u32) -> ItemReport {
        ItemReport {
            id: id(name),
            protocol_id,
        }
    }

    #[test]
    fn initial_recipe_update_exposes_supported_stonecutter_offers() {
        let items = ItemRegistry::from_report(&[
            item("minecraft:air", 0),
            item("minecraft:cobblestone", 14),
            item("minecraft:cobblestone_slab", 16),
            item("minecraft:limited_result", 17),
        ]);
        let recipe = |name: &str, result: &str, count| Recipe {
            id: id(name),
            kind: RecipeKind::Stonecutting(StonecuttingRecipe {
                ingredient: Ingredient {
                    alternatives: vec![IngredientAlternative::Item(id("minecraft:cobblestone"))],
                },
            }),
            result: RecipeResult {
                item: id(result),
                count,
            },
        };
        let recipes = vec![
            recipe(
                "minecraft:cobblestone_slab_from_cobblestone_stonecutting",
                "minecraft:cobblestone_slab",
                2,
            ),
            recipe("minecraft:air_output", "minecraft:air", 1),
            recipe("minecraft:zero_output", "minecraft:cobblestone_slab", 0),
            recipe(
                "minecraft:over_stack_output",
                "minecraft:cobblestone_slab",
                65,
            ),
            recipe(
                "minecraft:over_item_stack_output",
                "minecraft:limited_result",
                2,
            ),
            recipe(
                "minecraft:overflow_output",
                "minecraft:cobblestone_slab",
                u32::MAX,
            ),
        ];

        let item_facts = ItemFactsTable::from_entries([(
            id("minecraft:limited_result"),
            mc_data::item_components::ItemFacts {
                max_stack_size: Some(1),
                ..mc_data::item_components::ItemFacts::default()
            },
        )]);
        let packet = initial_recipe_update(&recipes, &items, &item_facts);

        assert!(packet.item_sets.is_empty());
        assert_eq!(
            packet.stonecutter_recipes,
            vec![StonecutterRecipeEntry {
                input: RecipeBookIngredient::Items(vec![14]),
                result: RecipeBookSlotDisplay::ItemStack {
                    item_id: 16,
                    count: 2,
                },
            }]
        );
    }

    #[test]
    fn initial_recipe_book_preserves_executor_ids_and_supported_shapes() {
        let items = ItemRegistry::from_report(&[
            item("minecraft:birch_log", 1),
            item("minecraft:birch_planks", 2),
            item("minecraft:stick", 3),
            item("minecraft:cobblestone", 4),
            item("minecraft:stone", 5),
            item("minecraft:crafting_table", 6),
            item("minecraft:furnace", 7),
            item("minecraft:blast_furnace", 8),
            item("minecraft:smoker", 9),
            item("minecraft:campfire", 10),
        ]);
        let recipes = vec![
            Recipe {
                id: id("minecraft:birch_planks"),
                kind: RecipeKind::Shapeless(ShapelessRecipe {
                    ingredients: vec![Ingredient {
                        alternatives: vec![IngredientAlternative::Tag(id("minecraft:birch_logs"))],
                    }],
                }),
                result: RecipeResult {
                    item: id("minecraft:birch_planks"),
                    count: 4,
                },
            },
            Recipe {
                id: id("minecraft:missing_output"),
                kind: RecipeKind::Shapeless(ShapelessRecipe {
                    ingredients: vec![Ingredient {
                        alternatives: vec![IngredientAlternative::Item(id("minecraft:birch_log"))],
                    }],
                }),
                result: RecipeResult {
                    item: id("minecraft:not_registered"),
                    count: 1,
                },
            },
            Recipe {
                id: id("minecraft:stick"),
                kind: RecipeKind::Shaped(ShapedRecipe {
                    pattern: vec!["A ".to_owned(), " A".to_owned()],
                    key: [(
                        'A',
                        Ingredient {
                            alternatives: vec![IngredientAlternative::Item(id(
                                "minecraft:birch_planks",
                            ))],
                        },
                    )]
                    .into_iter()
                    .collect(),
                }),
                result: RecipeResult {
                    item: id("minecraft:stick"),
                    count: 4,
                },
            },
            Recipe {
                id: id("minecraft:stone"),
                kind: RecipeKind::Smelting(SmeltingRecipe {
                    ingredient: Ingredient {
                        alternatives: vec![IngredientAlternative::Item(id(
                            "minecraft:cobblestone",
                        ))],
                    },
                    cooking_time: 200,
                    experience_milli: 0,
                }),
                result: RecipeResult {
                    item: id("minecraft:stone"),
                    count: 1,
                },
            },
        ];

        let packet = initial_recipe_book(&recipes, &items);

        assert!(packet.replace);
        assert_eq!(
            packet
                .entries
                .iter()
                .map(|entry| entry.display_id)
                .collect::<Vec<_>>(),
            vec![0, 2, 3]
        );
        assert_eq!(packet.entries[0].category_id, 3);
        assert_eq!(
            packet.entries[0].crafting_requirements,
            Some(vec![RecipeBookIngredient::Tag(id("minecraft:birch_logs"))])
        );
        assert_eq!(
            packet.entries[0].display,
            RecipeBookDisplay::Shapeless {
                ingredients: vec![RecipeBookSlotDisplay::Tag(id("minecraft:birch_logs"))],
                result: RecipeBookSlotDisplay::ItemStack {
                    item_id: 2,
                    count: 4,
                },
                crafting_station: RecipeBookSlotDisplay::Item { item_id: 6 },
            }
        );
        assert_eq!(
            packet.entries[1].display,
            RecipeBookDisplay::Shaped {
                width: 2,
                height: 2,
                ingredients: vec![
                    RecipeBookSlotDisplay::Item { item_id: 2 },
                    RecipeBookSlotDisplay::Empty,
                    RecipeBookSlotDisplay::Empty,
                    RecipeBookSlotDisplay::Item { item_id: 2 },
                ],
                result: RecipeBookSlotDisplay::ItemStack {
                    item_id: 3,
                    count: 4,
                },
                crafting_station: RecipeBookSlotDisplay::Item { item_id: 6 },
            }
        );
        assert_eq!(
            packet.entries[1].crafting_requirements,
            Some(vec![
                RecipeBookIngredient::Items(vec![2]),
                RecipeBookIngredient::Items(vec![2]),
            ])
        );
        assert_eq!(packet.entries[2].category_id, 6);
        assert_eq!(
            packet.entries[2].display,
            RecipeBookDisplay::Furnace {
                ingredient: RecipeBookSlotDisplay::Item { item_id: 4 },
                fuel: RecipeBookSlotDisplay::AnyFuel,
                result: RecipeBookSlotDisplay::ItemStack {
                    item_id: 5,
                    count: 1,
                },
                crafting_station: RecipeBookSlotDisplay::Item { item_id: 7 },
                duration: 200,
                experience: 0.0,
            }
        );
        assert!(packet.entries[2].crafting_requirements.is_none());
    }
}
