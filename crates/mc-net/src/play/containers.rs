use mc_data::items::ItemRegistry;
use mc_data::tags::TagsData;
use mc_nbt::Tag;

use crate::error::ConnectionError;
use crate::play::recipes::ingredient_accepts_item;
use crate::play::{
    FURNACE_CONTAINER_ID_MAX, FURNACE_CONTAINER_ID_MIN, InteractionState, PlayerPose,
    settle_player_inventory_returns,
};

mod chest;
mod crafting;
mod enchanting;
mod furnace;
pub(in crate::play) mod quickcraft;
mod script_menu;
#[cfg(test)]
mod script_menu_tests;
mod stonecutter;

pub(in crate::play) use chest::{
    ChestClickAction, ChestClickInput, ChestView, ChestWindow, adjacent_chest_positions,
    chest_slot_stacks, chest_wire_items, plan_click as plan_chest_click,
};
#[cfg(test)]
pub(in crate::play) use chest::{
    DOUBLE_CHEST_MENU_TYPE_ID, SINGLE_CHEST_STORAGE_SLOTS, apply_quick_move_click,
    apply_swap_click, apply_throw_click, chest_player_slot, set_chest_menu_stack,
};
pub(in crate::play) use crafting::{
    CRAFTING_MENU_TYPE_ID, CraftingTableWindow, crafting_menu_title_nbt,
    crafting_table_input_from_projection, crafting_table_input_projection, crafting_wire_items,
    refresh_crafting_result,
};
#[cfg(test)]
pub(in crate::play) use crafting::{
    crafting_result_from_input, inventory_crafting_input, refresh_inventory_crafting_result,
    repair_item_crafting_result,
};
#[cfg(test)]
pub(in crate::play) use enchanting::item_is_efficiency_enchantable;
pub(in crate::play) use enchanting::{
    ENCHANTING_MENU_SLOT_COUNT, ENCHANTING_MENU_TYPE_ID, EnchantingTableWindow,
    can_place_in_enchanting_menu_slot, count_valid_enchanting_bookshelves, enchant_item_candidate,
    enchanting_data_values, enchanting_menu_stack, enchanting_menu_title_nbt, enchanting_offer,
    enchanting_player_slot, enchanting_table_input_from_projection,
    enchanting_table_input_projection, enchanting_wire_items, is_lapis_stack,
    set_enchanting_menu_stack, supported_enchantment_for_item,
};
#[cfg(test)]
pub(in crate::play) use furnace::{
    BLAST_FURNACE_MENU_TYPE_ID, FURNACE_MENU_TYPE_ID, SMOKER_MENU_TYPE_ID,
    furnace_experience_award, stack_to_furnace_slot,
};
pub(in crate::play) use furnace::{
    FURNACE_MENU_SLOT_COUNT, FurnaceClickAction, FurnaceClickInput, FurnaceKind,
    decrement_furnace_slot, find_cooking_recipe_for_item, furnace_data_values,
    furnace_experience_seed, furnace_fuel_ticks, furnace_kind_for_block_id,
    furnace_menu_title_for_block_id, furnace_output_was_taken, furnace_slot_to_stack, plan_click,
    tick,
};
pub(in crate::play) use quickcraft::{QuickCraftClick, QuickCraftOutcome, QuickCraftState};
pub(in crate::play) use script_menu::{
    ScriptMenuClick, ScriptMenuClickDisposition, ScriptMenuOpenError, ScriptMenuWindow,
    client_close_matches,
};
pub(in crate::play) use stonecutter::{
    STONECUTTER_MENU_TYPE_ID, StonecutterClickAction, StonecutterClickInput, StonecutterWindow,
    plan_click as plan_stonecutter_click, select_stonecutter_recipe, set_stonecutter_input,
    stonecutter_input_array, stonecutter_input_from_projection, stonecutter_input_projection,
    stonecutter_menu_title_nbt, stonecutter_wire_items,
};

#[derive(Debug, Clone)]
pub(super) enum ActiveContainer {
    CraftingTable(Box<CraftingTableWindow>),
    EnchantingTable(EnchantingTableWindow),
    Stonecutter(StonecutterWindow),
    Furnace(FurnaceWindow),
    Chest(ChestWindow),
    Script(ScriptMenuWindow),
}

impl ActiveContainer {
    pub(super) fn container_id(&self) -> i32 {
        match self {
            Self::CraftingTable(window) => window.container_id,
            Self::EnchantingTable(window) => window.container_id,
            Self::Stonecutter(window) => window.container_id,
            Self::Furnace(window) => window.container_id,
            Self::Chest(window) => window.container_id,
            Self::Script(window) => window.container_id,
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

    pub(super) fn menu_type(&self) -> i32 {
        self.kind.menu_type()
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

pub(super) fn is_enchanting_table_state(
    state: &InteractionState,
    block_state: mc_world::BlockStateId,
) -> bool {
    state
        .blocks
        .by_id(block_state)
        .is_some_and(|block_state| block_state.block.id.as_str() == "minecraft:enchanting_table")
}

pub(super) fn is_stonecutter_state(
    state: &InteractionState,
    block_state: mc_world::BlockStateId,
) -> bool {
    state
        .blocks
        .by_id(block_state)
        .is_some_and(|block_state| block_state.block.id.as_str() == "minecraft:stonecutter")
}

pub(super) fn unsupported_survival_station_for_state(
    state: &InteractionState,
    block_state: mc_world::BlockStateId,
) -> Option<&'static str> {
    state.blocks.by_id(block_state).and_then(|block_state| {
        unsupported_survival_station_for_block_id(block_state.block.id.as_str())
    })
}

pub(super) fn unsupported_survival_station_for_block_id(id: &str) -> Option<&'static str> {
    match id {
        "minecraft:brewing_stand" => Some("brewing stand"),
        "minecraft:anvil" | "minecraft:chipped_anvil" | "minecraft:damaged_anvil" => Some("anvil"),
        "minecraft:smithing_table" => Some("smithing table"),
        "minecraft:grindstone" => Some("grindstone"),
        "minecraft:loom" => Some("loom"),
        "minecraft:cartography_table" => Some("cartography table"),
        "minecraft:composter" => Some("composter"),
        "minecraft:cauldron"
        | "minecraft:water_cauldron"
        | "minecraft:lava_cauldron"
        | "minecraft:powder_snow_cauldron" => Some("cauldron"),
        "minecraft:lectern" => Some("lectern"),
        "minecraft:fletching_table" => Some("fletching table"),
        "minecraft:beacon" => Some("beacon"),
        "minecraft:crafter" => Some("crafter"),
        _ => None,
    }
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

pub(super) fn is_fuel_item_id(items: &ItemRegistry, item_id: u32) -> bool {
    furnace_fuel_ticks(items, item_id).is_some()
}

pub(super) fn furnace_menu_title_nbt(title: &str) -> Result<Vec<u8>, mc_protocol::CodecError> {
    let mut out = Vec::new();
    mc_nbt::write_network(
        &mut out,
        &Tag::Compound(vec![("text".to_string(), Tag::String(title.to_string()))]),
    )?;
    Ok(out)
}

pub(super) fn chest_menu_title_nbt(title: &str) -> Result<Vec<u8>, mc_protocol::CodecError> {
    let mut out = Vec::new();
    mc_nbt::write_network(
        &mut out,
        &Tag::Compound(vec![("text".to_string(), Tag::String(title.to_string()))]),
    )?;
    Ok(out)
}

pub(super) fn next_container_id(state: &mut InteractionState) -> i32 {
    let id = state.next_container_id;
    state.next_container_id += 1;
    if state.next_container_id > FURNACE_CONTAINER_ID_MAX {
        state.next_container_id = FURNACE_CONTAINER_ID_MIN;
    }
    id
}

pub(super) async fn store_active_container(
    state: &mut InteractionState,
    player_pose: PlayerPose,
) -> Result<(), ConnectionError> {
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
            if let Err(error) = settle_player_inventory_returns(
                state,
                None,
                Some(&window.input),
                true,
                false,
                false,
                player_pose,
            )
            .await
            {
                state.active_container = Some(ActiveContainer::CraftingTable(window));
                return Err(error);
            }
        }
        Some(ActiveContainer::EnchantingTable(window)) => {
            if let Err(error) = settle_player_inventory_returns(
                state,
                Some(&window.inputs),
                None,
                false,
                false,
                false,
                player_pose,
            )
            .await
            {
                state.active_container = Some(ActiveContainer::EnchantingTable(window));
                return Err(error);
            }
        }
        Some(ActiveContainer::Stonecutter(window)) => {
            let input = stonecutter_input_array(&window.input);
            if let Err(error) = settle_player_inventory_returns(
                state,
                None,
                Some(&input),
                true,
                false,
                false,
                player_pose,
            )
            .await
            {
                state.active_container = Some(ActiveContainer::Stonecutter(window));
                return Err(error);
            }
        }
        Some(ActiveContainer::Script(_)) => {}
        None => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use mc_data::Identifier;

    use super::is_fuel_item_id;

    #[test]
    fn generated_wood_items_are_playable_furnace_fuel() {
        let items = mc_data::items::solaris_required_items();
        let birch_planks = items
            .id_of(&Identifier::parse("minecraft:birch_planks").unwrap())
            .expect("birch planks item");
        let birch_log = items
            .id_of(&Identifier::parse("minecraft:birch_log").unwrap())
            .expect("birch log item");
        let wooden_pickaxe = items
            .id_of(&Identifier::parse("minecraft:wooden_pickaxe").unwrap())
            .expect("wooden pickaxe item");

        assert!(is_fuel_item_id(&items, birch_planks));
        assert!(is_fuel_item_id(&items, birch_log));
        assert!(is_fuel_item_id(&items, wooden_pickaxe));
    }
}
