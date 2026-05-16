use super::*;

/// Item->default-block-state lookup for items whose identifier also
/// names a registered block.
#[derive(Debug, Clone, Default)]
pub(super) struct ItemToBlockTable {
    entries: Vec<(u32, mc_world::BlockStateId)>,
    crop_entries: Vec<CropPlacementEntry>,
    empty_bucket_item: Option<u32>,
    water_bucket_item: Option<u32>,
    lava_bucket_item: Option<u32>,
    water_source: Option<mc_world::BlockStateId>,
    lava_source: Option<mc_world::BlockStateId>,
}

#[derive(Debug, Clone)]
struct CropPlacementEntry {
    item_id: u32,
    soil_block: Identifier,
    crop_state: mc_world::BlockStateId,
}

impl ItemToBlockTable {
    pub(super) fn build(items: &ItemRegistry, blocks: &mc_world::BlockRegistry) -> Self {
        let entries = items
            .iter()
            .filter_map(|(item_name, item_pid)| {
                blocks
                    .block(item_name)
                    .map(|block| (item_pid, block.default))
            })
            .collect();
        let crop_entries = wheat_crop_placement_entry(items, blocks)
            .into_iter()
            .collect();
        let empty_bucket_item = item_id(items, "minecraft:bucket");
        let water_bucket_item = item_id(items, "minecraft:water_bucket");
        let lava_bucket_item = item_id(items, "minecraft:lava_bucket");
        let water_source = fluid_state_with_level(blocks, FluidKind::Water, 0);
        let lava_source = fluid_state_with_level(blocks, FluidKind::Lava, 0);
        Self {
            entries,
            crop_entries,
            empty_bucket_item,
            water_bucket_item,
            lava_bucket_item,
            water_source,
            lava_source,
        }
    }

    pub(super) fn resolve(&self, item_id: u32) -> Option<mc_world::BlockStateId> {
        self.entries
            .iter()
            .find_map(|(id, state)| (*id == item_id).then_some(*state))
    }

    pub(super) fn resolve_for_use_on(
        &self,
        items: &ItemRegistry,
        item_id: u32,
        clicked_state: mc_world::BlockStateId,
        direction: Direction,
        blocks: &mc_world::BlockRegistry,
    ) -> Option<mc_world::BlockStateId> {
        if direction.normal().1 == 1
            && let Some(clicked) = blocks.by_id(clicked_state)
            && let Some(entry) = self
                .crop_entries
                .iter()
                .find(|entry| entry.item_id == item_id && clicked.block.id == entry.soil_block)
        {
            return Some(entry.crop_state);
        }
        if is_sign_item(items, item_id) {
            return sign_state_for_use_on(items, item_id, direction, blocks);
        }
        self.resolve(item_id)
    }

    pub(super) fn empty_bucket_item(&self) -> Option<u32> {
        self.empty_bucket_item
    }

    pub(super) fn filled_bucket_item(&self, kind: FluidKind) -> Option<u32> {
        match kind {
            FluidKind::Water => self.water_bucket_item,
            FluidKind::Lava => self.lava_bucket_item,
        }
    }

    pub(super) fn bucket_fluid_kind(&self, item_id: u32) -> Option<FluidKind> {
        if Some(item_id) == self.water_bucket_item {
            Some(FluidKind::Water)
        } else if Some(item_id) == self.lava_bucket_item {
            Some(FluidKind::Lava)
        } else {
            None
        }
    }

    pub(super) fn fluid_source_state(&self, kind: FluidKind) -> Option<mc_world::BlockStateId> {
        match kind {
            FluidKind::Water => self.water_source,
            FluidKind::Lava => self.lava_source,
        }
    }
}

fn is_sign_item(items: &ItemRegistry, item_id: u32) -> bool {
    items.name_of(item_id).is_some_and(|item| {
        let path = item.path();
        path.ends_with("_sign") && !path.ends_with("_hanging_sign")
    })
}

fn sign_state_for_use_on(
    items: &ItemRegistry,
    item_id: u32,
    direction: Direction,
    blocks: &mc_world::BlockRegistry,
) -> Option<mc_world::BlockStateId> {
    let item = items.name_of(item_id)?;
    let path = item.path();
    if !path.ends_with("_sign") || path.ends_with("_hanging_sign") {
        return None;
    }
    if direction == Direction::Down {
        return None;
    }
    if direction == Direction::Up {
        return blocks.block(item).map(|block| block.default);
    }
    let wood = path.strip_suffix("_sign")?;
    let wall = Identifier::parse(format!("{}:{}_wall_sign", item.namespace(), wood)).ok()?;
    blocks.block(&wall).map(|block| block.default)
}

fn item_id(items: &ItemRegistry, name: &str) -> Option<u32> {
    items.id_of(&Identifier::parse(name).expect("static identifier"))
}

fn wheat_crop_placement_entry(
    items: &ItemRegistry,
    blocks: &mc_world::BlockRegistry,
) -> Option<CropPlacementEntry> {
    let wheat_seeds = Identifier::parse("minecraft:wheat_seeds").expect("static identifier");
    let farmland = Identifier::parse("minecraft:farmland").expect("static identifier");
    let wheat = Identifier::parse("minecraft:wheat").expect("static identifier");
    let item_id = items.id_of(&wheat_seeds)?;
    blocks.block(&farmland)?;
    let crop_state = crop_state_with_age(blocks, &wheat, 0)?;
    Some(CropPlacementEntry {
        item_id,
        soil_block: farmland,
        crop_state,
    })
}

pub(super) fn fallback_crafting_recipes() -> Vec<mc_data::recipes::Recipe> {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, ShapedRecipe,
        ShapelessRecipe, SmeltingRecipe,
    };

    let item =
        |id: &str| IngredientAlternative::Item(Identifier::parse(id).expect("static identifier"));
    let tag =
        |id: &str| IngredientAlternative::Tag(Identifier::parse(id).expect("static identifier"));
    let ingredient = |alternatives: Vec<IngredientAlternative>| Ingredient { alternatives };
    let shaped = |id: &str,
                  pattern: Vec<&str>,
                  key: Vec<(char, Vec<IngredientAlternative>)>,
                  result: &str,
                  count: u32| {
        Recipe {
            id: Identifier::parse(id).expect("static identifier"),
            kind: RecipeKind::Shaped(ShapedRecipe {
                pattern: pattern.into_iter().map(str::to_string).collect(),
                key: key
                    .into_iter()
                    .map(|(ch, alternatives)| (ch, ingredient(alternatives)))
                    .collect(),
            }),
            result: RecipeResult {
                item: Identifier::parse(result).expect("static identifier"),
                count,
            },
        }
    };
    let smelting = |id: &str, input: &str, result: &str| Recipe {
        id: Identifier::parse(id).expect("static identifier"),
        kind: RecipeKind::Smelting(SmeltingRecipe {
            ingredient: ingredient(vec![item(input)]),
            cooking_time: 200,
        }),
        result: RecipeResult {
            item: Identifier::parse(result).expect("static identifier"),
            count: 1,
        },
    };

    let mut recipes = vec![
        shaped(
            "minecraft:torch",
            vec!["X", "#"],
            vec![
                (
                    'X',
                    vec![item("minecraft:coal"), item("minecraft:charcoal")],
                ),
                ('#', vec![item("minecraft:stick")]),
            ],
            "minecraft:torch",
            4,
        ),
        Recipe {
            id: Identifier::parse("minecraft:oak_planks").expect("static identifier"),
            kind: RecipeKind::Shapeless(ShapelessRecipe {
                ingredients: vec![ingredient(vec![tag("minecraft:oak_logs")])],
            }),
            result: RecipeResult {
                item: Identifier::parse("minecraft:oak_planks").expect("static identifier"),
                count: 4,
            },
        },
        shaped(
            "minecraft:stick",
            vec!["#", "#"],
            vec![('#', vec![tag("minecraft:planks")])],
            "minecraft:stick",
            4,
        ),
        shaped(
            "minecraft:crafting_table",
            vec!["##", "##"],
            vec![('#', vec![tag("minecraft:planks")])],
            "minecraft:crafting_table",
            1,
        ),
        shaped(
            "minecraft:wooden_pickaxe",
            vec!["###", " X ", " X "],
            vec![
                ('#', vec![tag("minecraft:planks")]),
                ('X', vec![item("minecraft:stick")]),
            ],
            "minecraft:wooden_pickaxe",
            1,
        ),
        shaped(
            "minecraft:wooden_axe",
            vec!["##", "#X", " X"],
            vec![
                ('#', vec![tag("minecraft:planks")]),
                ('X', vec![item("minecraft:stick")]),
            ],
            "minecraft:wooden_axe",
            1,
        ),
        shaped(
            "minecraft:wooden_shovel",
            vec!["#", "X", "X"],
            vec![
                ('#', vec![tag("minecraft:planks")]),
                ('X', vec![item("minecraft:stick")]),
            ],
            "minecraft:wooden_shovel",
            1,
        ),
        shaped(
            "minecraft:wooden_sword",
            vec!["#", "#", "X"],
            vec![
                ('#', vec![tag("minecraft:planks")]),
                ('X', vec![item("minecraft:stick")]),
            ],
            "minecraft:wooden_sword",
            1,
        ),
        shaped(
            "minecraft:stone_pickaxe",
            vec!["###", " X ", " X "],
            vec![
                ('#', vec![item("minecraft:cobblestone")]),
                ('X', vec![item("minecraft:stick")]),
            ],
            "minecraft:stone_pickaxe",
            1,
        ),
        shaped(
            "minecraft:stone_axe",
            vec!["##", "#X", " X"],
            vec![
                ('#', vec![item("minecraft:cobblestone")]),
                ('X', vec![item("minecraft:stick")]),
            ],
            "minecraft:stone_axe",
            1,
        ),
        shaped(
            "minecraft:stone_shovel",
            vec!["#", "X", "X"],
            vec![
                ('#', vec![item("minecraft:cobblestone")]),
                ('X', vec![item("minecraft:stick")]),
            ],
            "minecraft:stone_shovel",
            1,
        ),
        shaped(
            "minecraft:stone_sword",
            vec!["#", "#", "X"],
            vec![
                ('#', vec![item("minecraft:cobblestone")]),
                ('X', vec![item("minecraft:stick")]),
            ],
            "minecraft:stone_sword",
            1,
        ),
        shaped(
            "minecraft:furnace",
            vec!["###", "# #", "###"],
            vec![('#', vec![item("minecraft:cobblestone")])],
            "minecraft:furnace",
            1,
        ),
    ];
    recipes.extend([
        smelting(
            "minecraft:iron_ingot_from_smelting_raw_iron",
            "minecraft:raw_iron",
            "minecraft:iron_ingot",
        ),
        smelting(
            "minecraft:gold_ingot_from_smelting_raw_gold",
            "minecraft:raw_gold",
            "minecraft:gold_ingot",
        ),
        smelting(
            "minecraft:copper_ingot_from_smelting_raw_copper",
            "minecraft:raw_copper",
            "minecraft:copper_ingot",
        ),
        smelting(
            "minecraft:cooked_beef",
            "minecraft:beef",
            "minecraft:cooked_beef",
        ),
        smelting(
            "minecraft:cooked_porkchop",
            "minecraft:porkchop",
            "minecraft:cooked_porkchop",
        ),
        smelting(
            "minecraft:cooked_chicken",
            "minecraft:chicken",
            "minecraft:cooked_chicken",
        ),
        smelting(
            "minecraft:baked_potato",
            "minecraft:potato",
            "minecraft:baked_potato",
        ),
    ]);
    recipes
}
