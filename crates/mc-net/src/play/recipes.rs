use super::containers::crafting_remainder_for_item;
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
    let result = recipe.result.to_stack(items, item_facts)?;
    if result.count > item_max_stack(item_facts, items, &result) {
        return None;
    }
    Some(StonecutterRecipeEntry {
        input: recipe_book_requirement(&stonecutting.ingredient, items)?,
        result: RecipeBookSlotDisplay::ItemStack(result),
    })
}

pub(super) fn initial_recipe_book(
    recipes: &[mc_data::recipes::Recipe],
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
) -> ClientboundRecipeBookAdd {
    let entries = recipes
        .iter()
        .enumerate()
        .filter_map(|(display_id, recipe)| recipe_book_entry(display_id, recipe, items, item_facts))
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
    item_facts: &ItemFactsTable,
) -> Option<RecipeBookEntry> {
    let display_id = i32::try_from(display_id).ok()?;
    let result = RecipeBookSlotDisplay::ItemStack(recipe.result.to_stack(items, item_facts)?);

    let (display, category_id, crafting_requirements) = match &recipe.kind {
        mc_data::recipes::RecipeKind::Shapeless(shapeless) => {
            let ingredients = shapeless
                .ingredients
                .iter()
                .map(|ingredient| recipe_book_slot(ingredient, items, item_facts))
                .collect::<Option<Vec<_>>>()?;
            let requirements = recipe_book_requirements(&shapeless.ingredients, items, item_facts);
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
                        ingredients.push(recipe_book_slot(ingredient, items, item_facts)?);
                        requirement_sources.push(ingredient.clone());
                    }
                }
            }
            let requirements = recipe_book_requirements(&requirement_sources, items, item_facts);
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
            recipe_book_cooking_display(cooking, result, items, item_facts, "minecraft:furnace")?,
            6,
            None,
        ),
        mc_data::recipes::RecipeKind::Blasting(cooking) => (
            recipe_book_cooking_display(
                cooking,
                result,
                items,
                item_facts,
                "minecraft:blast_furnace",
            )?,
            8,
            None,
        ),
        mc_data::recipes::RecipeKind::Smoking(cooking) => (
            recipe_book_cooking_display(cooking, result, items, item_facts, "minecraft:smoker")?,
            9,
            None,
        ),
        mc_data::recipes::RecipeKind::CampfireCooking(cooking) => (
            recipe_book_cooking_display(cooking, result, items, item_facts, "minecraft:campfire")?,
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
    item_facts: &ItemFactsTable,
) -> Option<RecipeBookSlotDisplay> {
    let mut alternatives = ingredient
        .alternatives
        .iter()
        .map(|alternative| match alternative {
            mc_data::recipes::IngredientAlternative::Item(item) => {
                if let Some(custom) = item_facts.custom(item) {
                    let item_id = items.id_of(&custom.carrier)?;
                    Some(RecipeBookSlotDisplay::ItemStack(
                        ItemStack::new(item_id, 1)
                            .with_item_model(custom.id.clone())
                            .with_custom_name(&custom.name),
                    ))
                } else {
                    items
                        .id_of(item)
                        .and_then(|item_id| i32::try_from(item_id).ok())
                        .map(|item_id| RecipeBookSlotDisplay::Item { item_id })
                }
            }
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
    item_facts: &ItemFactsTable,
) -> Option<Vec<RecipeBookIngredient>> {
    ingredients
        .iter()
        .map(|ingredient| {
            if ingredient.alternatives.iter().any(|alternative| {
                matches!(alternative, mc_data::recipes::IngredientAlternative::Item(item) if item_facts.custom(item).is_some())
            }) {
                None
            } else {
                recipe_book_requirement(ingredient, items)
            }
        })
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
    item_facts: &ItemFactsTable,
    station: &str,
) -> Option<RecipeBookDisplay> {
    Some(RecipeBookDisplay::Furnace {
        ingredient: recipe_book_slot(&recipe.ingredient, items, item_facts)?,
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
    inventory: &PlayerInventory,
    available: &[i32; 46],
    ingredient: &mc_data::recipes::Ingredient,
) -> Option<usize> {
    for (slot, available_count) in available.iter().enumerate().take(45).skip(9) {
        let current = &inventory.slots[slot];
        if *available_count > 0
            && mc_data::recipes::ingredient_accepts_stack(
                &state.items,
                &state.item_facts,
                &state.tags,
                current,
                ingredient,
            )
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

fn inventory_has_room_for_output(
    state: &InteractionState,
    inventory: &PlayerInventory,
    output: &ItemStack,
) -> bool {
    let mut remaining = output.count;
    let max_stack = item_max_stack(&state.item_facts, &state.items, output);
    for slot in 9..=44 {
        let current = &inventory.slots[slot];
        if current.is_empty() {
            remaining -= remaining.min(max_stack);
        } else if mc_data::inventory_semantics_26_1_2::can_stack(current, output)
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

type CraftingStepChanges = (Vec<(usize, ItemStack)>, Vec<ItemStack>);

fn craft_recipe_once(
    state: &InteractionState,
    inventory: &mut PlayerInventory,
    recipe: &mc_data::recipes::Recipe,
) -> Option<CraftingStepChanges> {
    let ingredients = recipe_ingredients(recipe)?;
    if ingredients.is_empty() {
        return None;
    }
    let output = recipe.result.to_stack(&state.items, &state.item_facts)?;
    if !inventory_has_room_for_output(state, inventory, &output) {
        return None;
    }

    let mut available = std::array::from_fn(|slot| inventory.slots[slot].count.max(0));
    let mut consumed_slots = Vec::with_capacity(ingredients.len());
    for ingredient in ingredients {
        let slot = matching_ingredient_slot(state, inventory, &available, ingredient)?;
        available[slot] -= 1;
        consumed_slots.push(slot);
    }

    let mut remainders = Vec::new();
    let mut changed = BTreeMap::new();
    for slot in consumed_slots {
        let current = &mut inventory.slots[slot];
        let item_id = current.item_id;
        current.count -= 1;
        if current.count <= 0 {
            *current = ItemStack::EMPTY;
        }
        changed.insert(slot, current.clone());
        if let Some(remainder) = crafting_remainder_for_item(&state.items, item_id) {
            remainders.push(remainder);
        }
    }

    let max_stack = item_max_stack(&state.item_facts, &state.items, &output);
    let (remaining, output_changed) = inventory.merge_stack(output, max_stack);
    if !remaining.is_empty() {
        return None;
    }
    for (slot, stack) in output_changed {
        changed.insert(slot, stack);
    }
    let mut overflow = Vec::new();
    for remainder in remainders {
        let max_stack = item_max_stack(&state.item_facts, &state.items, &remainder);
        let (remaining, slots) = inventory.merge_stack(remainder, max_stack);
        for (slot, stack) in slots {
            changed.insert(slot, stack);
        }
        if !remaining.is_empty() {
            overflow.push(remaining);
        }
    }
    Some((changed.into_iter().collect(), overflow))
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
    pub(super) overflow_remainders: Vec<ItemStack>,
}

pub(super) fn craft_recipe(
    state: &InteractionState,
    recipe: &mc_data::recipes::Recipe,
    use_max_items: bool,
) -> Option<(PlayerInventory, CraftRecipeOutcome)> {
    let item_id = recipe
        .result
        .to_stack(&state.items, &state.item_facts)?
        .item_id;
    let mut inventory = state.inventory.clone();
    if !use_max_items {
        let (changed_slots, overflow_remainders) =
            craft_recipe_once(state, &mut inventory, recipe)?;
        return Some((
            inventory,
            CraftRecipeOutcome {
                changed_slots,
                overflow_remainders,
                crafted: CraftedItem {
                    item_id,
                    count: u64::from(recipe.result.count),
                    craft_count: 1,
                },
            },
        ));
    }

    let max_crafts = inventory.slots[9..=44]
        .iter()
        .map(|stack| {
            let max_stack = item_max_stack(&state.item_facts, &state.items, stack);
            stack.count.clamp(0, max_stack) as usize
        })
        .fold(0usize, usize::saturating_add);
    let mut all_changed = BTreeMap::new();
    let mut overflow_remainders = Vec::new();
    let mut craft_count = 0_u32;
    for _ in 0..max_crafts {
        let Some((changed, overflow)) = craft_recipe_once(state, &mut inventory, recipe) else {
            break;
        };
        craft_count += 1;
        overflow_remainders.extend(overflow);
        for (slot, stack) in changed {
            all_changed.insert(slot, stack);
        }
    }
    let count = u64::from(recipe.result.count) * u64::from(craft_count);
    (!all_changed.is_empty()).then(|| {
        (
            inventory,
            CraftRecipeOutcome {
                changed_slots: all_changed.into_iter().collect(),
                overflow_remainders,
                crafted: CraftedItem {
                    item_id,
                    count,
                    craft_count,
                },
            },
        )
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
                stew_effects: Vec::new(),
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
                result: RecipeBookSlotDisplay::ItemStack(ItemStack::new(16, 2)),
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
                    stew_effects: Vec::new(),
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
                    stew_effects: Vec::new(),
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
                    stew_effects: Vec::new(),
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
                    stew_effects: Vec::new(),
                },
            },
        ];

        let packet = initial_recipe_book(&recipes, &items, &ItemFactsTable::default());

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
                result: RecipeBookSlotDisplay::ItemStack(ItemStack::new(2, 4)),
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
                result: RecipeBookSlotDisplay::ItemStack(ItemStack::new(3, 4)),
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
                result: RecipeBookSlotDisplay::ItemStack(ItemStack::new(5, 1)),
                crafting_station: RecipeBookSlotDisplay::Item { item_id: 7 },
                duration: 200,
                experience: 0.0,
            }
        );
        assert!(packet.entries[2].crafting_requirements.is_none());
    }

    #[test]
    fn initial_recipe_book_round_trips_full_embedded_baseline() {
        use mc_protocol::packets::{Packet, play::ClientboundRecipeBookAdd};

        let recipes = mc_data::recipes::solaris_required_recipes();
        let items = mc_data::items::solaris_required_items();
        let packet = initial_recipe_book(&recipes, &items, &ItemFactsTable::default());
        // Stonecutting has no book display; everything else must survive.
        let unsupported = recipes
            .iter()
            .filter(|recipe| matches!(recipe.kind, RecipeKind::Stonecutting(_)))
            .count();
        assert!(!packet.entries.is_empty());
        assert_eq!(packet.entries.len(), recipes.len() - unsupported);
        let mut wire = Vec::new();
        packet.encode(&mut wire).expect("full baseline encodes");
        let back = ClientboundRecipeBookAdd::decode(&mut wire.as_slice())
            .expect("full baseline round-trips");
        assert_eq!(back, packet);
    }

    #[test]
    fn recipe_book_craft_preserves_bucket_remainder_when_inventory_is_full() {
        let milk = id("minecraft:milk_bucket");
        let output = id("minecraft:test_output");
        let items = std::sync::Arc::new(ItemRegistry::from_report(&[
            item("minecraft:milk_bucket", 1),
            item("minecraft:bucket", 2),
            item("minecraft:test_output", 3),
            item("minecraft:dirt", 4),
        ]));
        let mut state = crate::play::tests::interaction_state_for_items(items);
        for slot in 9..=44 {
            state.inventory.slots[slot] = ItemStack::new(4, 64);
        }
        state.inventory.slots[9] = ItemStack::new(1, 2);
        state.inventory.slots[10] = ItemStack::new(3, 63);
        let recipe = Recipe {
            id: id("minecraft:test_bucket_recipe"),
            kind: RecipeKind::Shapeless(ShapelessRecipe {
                ingredients: vec![Ingredient {
                    alternatives: vec![IngredientAlternative::Item(milk)],
                }],
            }),
            result: RecipeResult {
                item: output,
                count: 1,
                stew_effects: Vec::new(),
            },
        };

        let (inventory, outcome) = craft_recipe(&state, &recipe, false).unwrap();
        assert_eq!(inventory.slots[9], ItemStack::new(1, 1));
        assert_eq!(inventory.slots[10], ItemStack::new(3, 64));
        assert_eq!(outcome.overflow_remainders, vec![ItemStack::new(2, 1)]);
    }
    #[test]
    fn custom_item_recipe_book_keeps_identity_and_never_autofills_a_paper_carrier() {
        use mc_data::item_components::{CustomItemDefinition, ItemFacts};
        use mc_protocol::packets::Packet;

        let ruby = id("ruby-live:ruby");
        let blade = id("ruby-live:blade");
        let items = ItemRegistry::from_report(&[
            item("minecraft:paper", 1),
            item("minecraft:crafting_table", 2),
        ]);
        let facts = ItemFactsTable::default()
            .with_custom_items(
                [
                    CustomItemDefinition {
                        id: ruby.clone(),
                        carrier: id("minecraft:paper"),
                        name: "Ruby".to_owned(),
                        crafting_ingredient: Some(id("minecraft:paper")),
                        facts: ItemFacts {
                            max_stack_size: Some(16),
                            ..ItemFacts::default()
                        },
                    },
                    CustomItemDefinition {
                        id: blade.clone(),
                        carrier: id("minecraft:paper"),
                        name: "Ruby Blade".to_owned(),
                        crafting_ingredient: Some(ruby.clone()),
                        facts: ItemFacts {
                            max_stack_size: Some(1),
                            max_damage: Some(3),
                            ..ItemFacts::default()
                        },
                    },
                ],
                &items,
            )
            .unwrap();
        let recipe = Recipe {
            id: blade.clone(),
            kind: RecipeKind::Shapeless(ShapelessRecipe {
                ingredients: vec![Ingredient {
                    alternatives: vec![IngredientAlternative::Item(ruby.clone())],
                }],
            }),
            result: RecipeResult {
                item: blade.clone(),
                count: 1,
                stew_effects: Vec::new(),
            },
        };
        let packet = initial_recipe_book(&[recipe], &items, &facts);
        let mut wire = Vec::new();
        packet.encode(&mut wire).unwrap();
        let decoded = ClientboundRecipeBookAdd::decode(&mut wire.as_slice()).unwrap();
        assert_eq!(decoded.entries.len(), 1);
        assert!(decoded.entries[0].crafting_requirements.is_none());
        let RecipeBookDisplay::Shapeless {
            ingredients,
            result,
            ..
        } = &decoded.entries[0].display
        else {
            panic!("expected shapeless client display");
        };
        assert!(
            matches!(&ingredients[0], RecipeBookSlotDisplay::ItemStack(stack)
            if stack.item_model.as_deref() == Some(&ruby))
        );
        assert!(matches!(result, RecipeBookSlotDisplay::ItemStack(stack)
            if stack.item_model.as_deref() == Some(&blade)));
    }
}
