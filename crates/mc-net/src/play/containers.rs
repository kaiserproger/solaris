use super::*;

#[derive(Debug, Clone)]
pub(super) enum ActiveContainer {
    CraftingTable(CraftingTableWindow),
    Furnace(FurnaceWindow),
    Chest(ChestWindow),
}

impl ActiveContainer {
    pub(super) fn container_id(&self) -> i32 {
        match self {
            Self::CraftingTable(window) => window.container_id,
            Self::Furnace(window) => window.container_id,
            Self::Chest(window) => window.container_id,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct CraftingTableWindow {
    pub(super) container_id: i32,
    pub(super) state_id: i32,
    pub(super) input: [ItemStack; 9],
    pub(super) result: ItemStack,
}

impl CraftingTableWindow {
    pub(super) fn new(container_id: i32) -> Self {
        Self {
            container_id,
            state_id: 1,
            input: std::array::from_fn(|_| ItemStack::EMPTY),
            result: ItemStack::EMPTY,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct FurnaceWindow {
    pub(super) container_id: i32,
    pub(super) position: mc_world::BlockPos,
    pub(super) kind: FurnaceKind,
    pub(super) state_id: i32,
}

impl FurnaceWindow {
    pub(super) fn new(position: mc_world::BlockPos, container_id: i32, kind: FurnaceKind) -> Self {
        Self {
            container_id,
            position,
            kind,
            state_id: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FurnaceKind {
    Furnace,
    Smoker,
    BlastFurnace,
}

#[derive(Debug, Clone)]
pub(super) struct ChestWindow {
    pub(super) container_id: i32,
    pub(super) positions: Vec<mc_world::BlockPos>,
    pub(super) state_id: i32,
}

impl ChestWindow {
    pub(super) fn new(mut positions: Vec<mc_world::BlockPos>, container_id: i32) -> Self {
        positions.sort_by_key(|pos| (pos.x, pos.y, pos.z));
        positions.dedup();
        debug_assert!(!positions.is_empty());
        debug_assert!(positions.len() <= 2);
        Self {
            container_id,
            positions,
            state_id: 1,
        }
    }

    pub(super) fn position(&self) -> mc_world::BlockPos {
        self.positions[0]
    }

    pub(super) fn menu_type(&self) -> i32 {
        if self.positions.len() == 2 {
            DOUBLE_CHEST_MENU_TYPE_ID
        } else {
            CHEST_MENU_TYPE_ID
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct ChestView {
    pub(super) chests: Vec<ChestBlockEntity>,
}

impl ChestView {
    pub(super) fn storage_slots(&self) -> usize {
        self.chests.len() * SINGLE_CHEST_STORAGE_SLOTS
    }
}

pub(super) fn is_furnace_state(
    state: &InteractionState,
    block_state: mc_world::BlockStateId,
) -> bool {
    furnace_menu_title_for_state(state, block_state).is_some()
}

pub(super) fn furnace_menu_title_for_state(
    state: &InteractionState,
    block_state: mc_world::BlockStateId,
) -> Option<&'static str> {
    state
        .blocks
        .by_id(block_state)
        .and_then(|block_state| furnace_menu_title_for_block_id(block_state.block.id.as_str()))
}

pub(super) fn furnace_kind_for_state(
    state: &InteractionState,
    block_state: mc_world::BlockStateId,
) -> Option<FurnaceKind> {
    state
        .blocks
        .by_id(block_state)
        .and_then(|block_state| furnace_kind_for_block_id(block_state.block.id.as_str()))
}

pub(super) fn furnace_kind_for_block_id(id: &str) -> Option<FurnaceKind> {
    match id {
        "minecraft:furnace" => Some(FurnaceKind::Furnace),
        "minecraft:smoker" => Some(FurnaceKind::Smoker),
        "minecraft:blast_furnace" => Some(FurnaceKind::BlastFurnace),
        _ => None,
    }
}

pub(super) fn furnace_menu_title_for_block_id(id: &str) -> Option<&'static str> {
    match id {
        "minecraft:furnace" => Some("Furnace"),
        "minecraft:smoker" => Some("Smoker"),
        "minecraft:blast_furnace" => Some("Blast Furnace"),
        _ => None,
    }
}

pub(super) fn is_chest_state(
    state: &InteractionState,
    block_state: mc_world::BlockStateId,
) -> bool {
    state
        .blocks
        .by_id(block_state)
        .is_some_and(|block_state| block_state.block.id.as_str() == "minecraft:chest")
}

pub(super) fn is_barrel_state(
    state: &InteractionState,
    block_state: mc_world::BlockStateId,
) -> bool {
    state
        .blocks
        .by_id(block_state)
        .is_some_and(|block_state| block_state.block.id.as_str() == "minecraft:barrel")
}

pub(super) fn is_crafting_table_state(
    state: &InteractionState,
    block_state: mc_world::BlockStateId,
) -> bool {
    state
        .blocks
        .by_id(block_state)
        .is_some_and(|block_state| block_state.block.id.as_str() == "minecraft:crafting_table")
}

pub(super) fn find_smelting_recipe_for_item(
    state: &InteractionState,
    kind: FurnaceKind,
    item_id: u32,
) -> Option<mc_data::recipes::Recipe> {
    find_cooking_recipe_for_item(&state.recipes, &state.items, &state.tags, kind, item_id)
}

pub(super) fn find_campfire_recipe_for_item(
    state: &InteractionState,
    item_id: u32,
) -> Option<mc_data::recipes::Recipe> {
    find_campfire_recipe_in(&state.recipes, &state.items, &state.tags, item_id)
}

pub(super) fn find_campfire_recipe_in(
    recipes: &[mc_data::recipes::Recipe],
    items: &ItemRegistry,
    tags: &TagsData,
    item_id: u32,
) -> Option<mc_data::recipes::Recipe> {
    recipes.iter().find_map(|recipe| {
        let mc_data::recipes::RecipeKind::CampfireCooking(smelting) = &recipe.kind else {
            return None;
        };
        ingredient_accepts_item(items, tags, item_id, &smelting.ingredient).then(|| recipe.clone())
    })
}

pub(super) fn find_cooking_recipe_for_item(
    recipes: &[mc_data::recipes::Recipe],
    items: &ItemRegistry,
    tags: &TagsData,
    kind: FurnaceKind,
    item_id: u32,
) -> Option<mc_data::recipes::Recipe> {
    recipes.iter().find_map(|recipe| {
        let smelting = match (&recipe.kind, kind) {
            (mc_data::recipes::RecipeKind::Smelting(smelting), FurnaceKind::Furnace) => smelting,
            (mc_data::recipes::RecipeKind::Smoking(smelting), FurnaceKind::Smoker) => smelting,
            (mc_data::recipes::RecipeKind::Blasting(smelting), FurnaceKind::BlastFurnace) => {
                smelting
            }
            _ => return None,
        };
        ingredient_accepts_item(items, tags, item_id, &smelting.ingredient).then(|| recipe.clone())
    })
}

pub(super) fn is_fuel_item(state: &InteractionState, item_id: u32) -> bool {
    let coal = state
        .items
        .id_of(&Identifier::parse("minecraft:coal").expect("static identifier"));
    let charcoal = state
        .items
        .id_of(&Identifier::parse("minecraft:charcoal").expect("static identifier"));
    Some(item_id) == coal || Some(item_id) == charcoal
}

pub(super) fn furnace_menu_title_nbt(title: &str) -> Vec<u8> {
    let mut out = Vec::new();
    mc_nbt::write_network(
        &mut out,
        &Tag::Compound(vec![("text".to_string(), Tag::String(title.to_string()))]),
    )
    .expect("static text component is valid NBT");
    out
}

pub(super) fn crafting_menu_title_nbt() -> Vec<u8> {
    let mut out = Vec::new();
    mc_nbt::write_network(
        &mut out,
        &Tag::Compound(vec![(
            "text".to_string(),
            Tag::String("Crafting".to_string()),
        )]),
    )
    .expect("static text component is valid NBT");
    out
}

pub(super) fn chest_menu_title_nbt(title: &str) -> Vec<u8> {
    let mut out = Vec::new();
    mc_nbt::write_network(
        &mut out,
        &Tag::Compound(vec![("text".to_string(), Tag::String(title.to_string()))]),
    )
    .expect("static text component is valid NBT");
    out
}

pub(super) fn next_container_id(state: &mut InteractionState) -> i32 {
    let id = state.next_container_id;
    state.next_container_id += 1;
    if state.next_container_id > FURNACE_CONTAINER_ID_MAX {
        state.next_container_id = FURNACE_CONTAINER_ID_MIN;
    }
    id
}

pub(super) fn store_active_container(state: &mut InteractionState) {
    match state.active_container.take() {
        Some(ActiveContainer::Furnace(window)) => {
            state
                .sessions
                .unregister_furnace_viewer(state.session_id, window.position);
        }
        Some(ActiveContainer::Chest(window)) => {
            state
                .sessions
                .unregister_chest_viewer(state.session_id, window.position());
        }
        Some(ActiveContainer::CraftingTable(window)) => {
            for stack in window.input {
                let max_stack = item_max_stack(&state.item_facts, &state.items, &stack);
                let (remaining, _) = state.inventory.merge_stack(stack, max_stack);
                if !remaining.is_empty() {
                    debug!(
                        item_id = remaining.item_id,
                        count = remaining.count,
                        "dropping crafting remainder because inventory is full"
                    );
                }
            }
        }
        None => {}
    }
}
