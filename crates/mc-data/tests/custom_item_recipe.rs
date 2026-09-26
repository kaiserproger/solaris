use mc_data::inventory_semantics_26_1_2::{can_stack, take_from_stack};
use mc_data::item_components::{CustomItemDefinition, ItemFacts, ItemFactsTable};
use mc_data::item_semantics_26_1_2::{equippable_player_slot, max_stack_for_stack};
use mc_data::items::{ItemRegistry, ItemReport};
use mc_data::recipes::{
    Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, ShapelessRecipe,
    crafting_result_from_input,
};
use mc_data::{Identifier, ItemStack};

fn id(name: &str) -> Identifier {
    Identifier::parse(name).unwrap()
}

#[test]
fn custom_material_crafts_into_equippable_blade_without_matching_its_paper_carrier() {
    let items = ItemRegistry::from_report(&[ItemReport {
        id: id("minecraft:paper"),
        protocol_id: 1,
    }]);
    let ruby = id("ruby-live:ruby");
    let blade = id("ruby-live:blade");
    let facts = ItemFactsTable::default()
        .with_custom_items(
            [
                CustomItemDefinition {
                    id: ruby.clone(),
                    carrier: id("minecraft:paper"),
                    name: "Ruby".into(),
                    crafting_ingredient: Some(id("minecraft:paper")),
                    facts: ItemFacts {
                        max_stack_size: Some(16),
                        ..ItemFacts::default()
                    },
                },
                CustomItemDefinition {
                    id: blade.clone(),
                    carrier: id("minecraft:paper"),
                    name: "Ruby Blade".into(),
                    crafting_ingredient: Some(ruby.clone()),
                    facts: ItemFacts {
                        max_stack_size: Some(1),
                        max_damage: Some(3),
                        weapon: true,
                        attack_damage_modifier: Some(5.0),
                        equippable_slot: Some("head".into()),
                        ..ItemFacts::default()
                    },
                },
            ],
            &items,
        )
        .unwrap();
    let recipes = [ruby.clone(), blade.clone()]
        .into_iter()
        .map(|result| {
            let ingredient = if result == ruby {
                id("minecraft:paper")
            } else {
                ruby.clone()
            };
            Recipe {
                id: result.clone(),
                kind: RecipeKind::Shapeless(ShapelessRecipe {
                    ingredients: vec![Ingredient {
                        alternatives: vec![IngredientAlternative::Item(ingredient)],
                    }],
                }),
                result: RecipeResult {
                    item: result,
                    count: 1,
                    stew_effects: Vec::new(),
                },
            }
        })
        .collect::<Vec<_>>();
    let tags = mc_data::tags::TagsData::default();
    let paper = ItemStack::new(1, 1);
    let mut grid = std::array::from_fn(|_| ItemStack::EMPTY);
    grid[0] = paper.clone();
    let material = crafting_result_from_input(&items, &facts, &tags, &recipes, &grid);
    assert_eq!(material.item_model.as_deref(), Some(&ruby));
    assert_eq!(max_stack_for_stack(&facts, &items, &material), 16);

    let mut stack = material.clone();
    stack.count = 2;
    let split = take_from_stack(&mut stack, 1);
    assert!(can_stack(&stack, &split));
    assert!(!can_stack(&stack, &paper));
    assert_eq!(stack.item_model.as_deref(), Some(&ruby));

    grid[0] = split;
    let sword = crafting_result_from_input(&items, &facts, &tags, &recipes, &grid);
    assert_eq!(sword.item_model.as_deref(), Some(&blade));
    assert_eq!(equippable_player_slot(&facts, &items, &sword), Some(5));
    assert_eq!(max_stack_for_stack(&facts, &items, &sword), 1);
    assert!(!can_stack(&sword, &material));

    // The same wire carrier cannot satisfy a recipe for the canonical identity.
    let blade_recipe = &recipes[1..];
    grid[0] = paper;
    assert!(crafting_result_from_input(&items, &facts, &tags, blade_recipe, &grid).is_empty());
    grid[0] = ItemStack::new(2, 1).with_item_model(ruby);
    assert!(crafting_result_from_input(&items, &facts, &tags, blade_recipe, &grid).is_empty());
}
