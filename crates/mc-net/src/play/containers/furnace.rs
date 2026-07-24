use std::collections::BTreeMap;

use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_data::recipes::{Recipe, RecipeKind};
use mc_data::tags::TagsData;
use mc_protocol::codec::Identifier;
use mc_protocol::packets::play::ItemStack;
use mc_world::{BlockPos, FurnaceBlockEntity, FurnaceSlot};

use crate::play::inventory::{PlayerInventory, can_stack, item_max_stack};
use crate::play::recipes::ingredient_accepts_item;

pub(in crate::play) const FURNACE_MENU_SLOT_COUNT: usize = 39;
pub(in crate::play) const FURNACE_MENU_TYPE_ID: i32 = 14;
pub(in crate::play) const SMOKER_MENU_TYPE_ID: i32 = 22;
pub(in crate::play) const BLAST_FURNACE_MENU_TYPE_ID: i32 = 10;
const DEFAULT_FURNACE_COOK_TICKS: i16 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::play) enum FurnaceKind {
    Furnace,
    Smoker,
    BlastFurnace,
}

impl FurnaceKind {
    pub(in crate::play) fn menu_type(self) -> i32 {
        match self {
            Self::Furnace => FURNACE_MENU_TYPE_ID,
            Self::Smoker => SMOKER_MENU_TYPE_ID,
            Self::BlastFurnace => BLAST_FURNACE_MENU_TYPE_ID,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::play) enum FurnaceClickAction {
    Pickup { slot: usize, button: i8 },
    OutsidePickup { button: i8 },
    QuickMove { slot: usize },
    Swap { slot: usize, button: i8 },
    Throw { slot: usize, button: i8 },
    Unsupported,
}

pub(in crate::play) struct FurnaceClickInput<'a> {
    pub(in crate::play) recipes: &'a [Recipe],
    pub(in crate::play) items: &'a ItemRegistry,
    pub(in crate::play) item_facts: &'a ItemFactsTable,
    pub(in crate::play) tags: &'a TagsData,
    pub(in crate::play) kind: FurnaceKind,
    pub(in crate::play) furnace: FurnaceBlockEntity,
    pub(in crate::play) inventory: PlayerInventory,
    pub(in crate::play) carried_item: ItemStack,
    pub(in crate::play) action: FurnaceClickAction,
    pub(in crate::play) experience_seed: u64,
}

pub(in crate::play) struct FurnaceClickPlan {
    pub(in crate::play) furnace: FurnaceBlockEntity,
    pub(in crate::play) inventory: PlayerInventory,
    pub(in crate::play) carried_item: ItemStack,
    pub(in crate::play) dropped: Option<ItemStack>,
    pub(in crate::play) experience: i32,
}

pub(in crate::play) struct FurnaceTickResult {
    pub(in crate::play) furnace: FurnaceBlockEntity,
    pub(in crate::play) slots_changed: bool,
    pub(in crate::play) data_changed: Vec<(i16, i16)>,
}

pub(in crate::play) fn furnace_kind_for_block_id(id: &str) -> Option<FurnaceKind> {
    match id {
        "minecraft:furnace" => Some(FurnaceKind::Furnace),
        "minecraft:smoker" => Some(FurnaceKind::Smoker),
        "minecraft:blast_furnace" => Some(FurnaceKind::BlastFurnace),
        _ => None,
    }
}

pub(in crate::play) fn furnace_menu_title_for_block_id(id: &str) -> Option<&'static str> {
    match id {
        "minecraft:furnace" => Some("Furnace"),
        "minecraft:smoker" => Some("Smoker"),
        "minecraft:blast_furnace" => Some("Blast Furnace"),
        _ => None,
    }
}

pub(in crate::play) fn find_cooking_recipe_for_item(
    recipes: &[Recipe],
    items: &ItemRegistry,
    tags: &TagsData,
    kind: FurnaceKind,
    item_id: u32,
) -> Option<Recipe> {
    recipes.iter().find_map(|recipe| {
        let cooking = match (&recipe.kind, kind) {
            (RecipeKind::Smelting(cooking), FurnaceKind::Furnace) => cooking,
            (RecipeKind::Smoking(cooking), FurnaceKind::Smoker) => cooking,
            (RecipeKind::Blasting(cooking), FurnaceKind::BlastFurnace) => cooking,
            _ => return None,
        };
        ingredient_accepts_item(items, tags, item_id, &cooking.ingredient).then(|| recipe.clone())
    })
}

pub(in crate::play) fn furnace_fuel_ticks(
    tags: &TagsData,
    kind: FurnaceKind,
    item_id: u32,
) -> Option<i16> {
    let ticks = tags.fuel_values().burn_duration(item_id)?;
    Some(match kind {
        FurnaceKind::Furnace => ticks,
        FurnaceKind::Smoker | FurnaceKind::BlastFurnace => ticks / 2,
    })
}

pub(in crate::play) fn furnace_slot_to_stack(slot: &FurnaceSlot) -> ItemStack {
    if slot.is_empty() {
        ItemStack::EMPTY
    } else {
        ItemStack {
            count: slot.count,
            item_id: slot.item_id,
            damage: slot.damage,
            enchantments: slot.enchantments.clone(),
            custom_name: None,
            item_model: None,
        }
    }
}

pub(in crate::play) fn stack_to_furnace_slot(stack: &ItemStack) -> FurnaceSlot {
    if stack.is_empty() {
        FurnaceSlot::EMPTY
    } else {
        FurnaceSlot {
            count: stack.count,
            item_id: stack.item_id,
            damage: stack.damage,
            enchantments: stack.enchantments.clone(),
        }
    }
}

pub(in crate::play) fn furnace_data_values(furnace: &FurnaceBlockEntity) -> [(i16, i16); 4] {
    [
        (0, furnace.burn_remaining),
        (1, furnace.burn_total),
        (2, furnace.cook_progress),
        (3, furnace.cook_total),
    ]
}

pub(in crate::play) fn furnace_experience_seed(position: BlockPos, simulation_tick: u64) -> u64 {
    let mut seed = simulation_tick;
    seed ^= (position.x as i64 as u64).rotate_left(17);
    seed ^= (position.y as i64 as u64).rotate_left(33);
    seed ^= (position.z as i64 as u64).rotate_left(49);
    splitmix64(seed)
}

pub(in crate::play) fn furnace_experience_award(
    recipes: &[Recipe],
    recipes_used: &BTreeMap<String, i32>,
    seed: u64,
) -> i32 {
    let mut total = 0_u64;
    for (recipe_id, count) in recipes_used {
        let Ok(count) = u64::try_from(*count) else {
            continue;
        };
        let Some(recipe) = recipes
            .iter()
            .find(|recipe| recipe.id.as_str() == recipe_id)
        else {
            continue;
        };
        let experience_milli = match &recipe.kind {
            RecipeKind::Smelting(recipe)
            | RecipeKind::Blasting(recipe)
            | RecipeKind::Smoking(recipe) => recipe.experience_milli,
            RecipeKind::Shaped(_)
            | RecipeKind::Shapeless(_)
            | RecipeKind::CampfireCooking(_)
            | RecipeKind::Stonecutting(_) => continue,
        };
        let scaled = count.saturating_mul(u64::from(experience_milli));
        let mut recipe_award = scaled / 1_000;
        let remainder = scaled % 1_000;
        if remainder > 0 {
            let mut recipe_seed = seed ^ 0xCBF2_9CE4_8422_2325;
            for byte in recipe_id.bytes() {
                recipe_seed ^= u64::from(byte);
                recipe_seed = recipe_seed.wrapping_mul(0x0000_0100_0000_01B3);
            }
            if splitmix64(recipe_seed) % 1_000 < remainder {
                recipe_award = recipe_award.saturating_add(1);
            }
        }
        total = total.saturating_add(recipe_award);
    }
    total.min(i32::MAX as u64) as i32
}

pub(in crate::play) fn plan_click(input: FurnaceClickInput<'_>) -> Option<FurnaceClickPlan> {
    let FurnaceClickInput {
        recipes,
        items,
        item_facts,
        tags,
        kind,
        mut furnace,
        mut inventory,
        mut carried_item,
        action,
        experience_seed,
    } = input;
    let before_furnace = furnace.clone();
    let mut dropped = None;

    let changed = match action {
        FurnaceClickAction::Pickup { slot, button } => apply_pickup_click(
            recipes,
            items,
            item_facts,
            tags,
            kind,
            &mut furnace,
            &mut inventory,
            &mut carried_item,
            slot,
            button,
        ),
        FurnaceClickAction::OutsidePickup { button } => {
            dropped = apply_outside_pickup_click(&mut carried_item, button);
            dropped.is_some()
        }
        FurnaceClickAction::QuickMove { slot } => apply_quick_move_click(
            recipes,
            items,
            item_facts,
            tags,
            kind,
            &mut furnace,
            &mut inventory,
            slot,
        ),
        FurnaceClickAction::Swap { slot, button } => apply_swap_click(
            recipes,
            items,
            tags,
            kind,
            &mut furnace,
            &mut inventory,
            slot,
            button,
        ),
        FurnaceClickAction::Throw { slot, button } => {
            dropped = apply_throw_click(&mut furnace, &mut inventory, slot, button);
            dropped.is_some()
        }
        FurnaceClickAction::Unsupported => false,
    };
    if !changed {
        return None;
    }

    let experience = if furnace_output_was_taken(&before_furnace, &furnace) {
        let experience =
            furnace_experience_award(recipes, &before_furnace.recipes_used, experience_seed);
        furnace.recipes_used.clear();
        experience
    } else {
        0
    };

    Some(FurnaceClickPlan {
        furnace,
        inventory,
        carried_item,
        dropped,
        experience,
    })
}

pub(in crate::play) fn tick(
    recipes: &[Recipe],
    items: &ItemRegistry,
    tags: &TagsData,
    before: &FurnaceBlockEntity,
    kind: FurnaceKind,
) -> FurnaceTickResult {
    let mut furnace = before.clone();
    let before_slots = furnace.slots.clone();
    let before_data = furnace_data_values(&furnace);

    if furnace.burn_remaining > 0 {
        furnace.burn_remaining -= 1;
    }

    let input = furnace_slot_to_stack(&furnace.slots[0]);
    let recipe = (!input.is_empty())
        .then(|| find_cooking_recipe_for_item(recipes, items, tags, kind, input.item_id))
        .flatten();
    let Some(recipe) = recipe else {
        furnace.cook_progress = 0;
        return tick_result(furnace, before_slots, before_data);
    };
    let Some(output_item_id) = items.id_of(&recipe.result.item) else {
        furnace.cook_progress = 0;
        return tick_result(furnace, before_slots, before_data);
    };
    let output_count = i32::try_from(recipe.result.count).unwrap_or(0);
    let cooking_time = match &recipe.kind {
        RecipeKind::Smelting(cooking)
        | RecipeKind::Blasting(cooking)
        | RecipeKind::Smoking(cooking)
        | RecipeKind::CampfireCooking(cooking) => cooking.cooking_time,
        _ => DEFAULT_FURNACE_COOK_TICKS as u32,
    };
    furnace.cook_total = i16::try_from(cooking_time)
        .unwrap_or(DEFAULT_FURNACE_COOK_TICKS)
        .max(1);

    if output_count <= 0 || !furnace_output_room(&furnace, output_item_id, output_count) {
        furnace.cook_progress = 0;
        return tick_result(furnace, before_slots, before_data);
    }

    if furnace.burn_remaining <= 0
        && !furnace.slots[1].is_empty()
        && let Some(fuel_ticks) = furnace_fuel_ticks(tags, kind, furnace.slots[1].item_id)
    {
        consume_furnace_fuel(items, &mut furnace.slots[1]);
        furnace.burn_total = fuel_ticks;
        furnace.burn_remaining = fuel_ticks;
    }

    if furnace.burn_remaining > 0 {
        furnace.cook_progress += 1;
        if furnace.cook_progress >= furnace.cook_total {
            decrement_furnace_slot(&mut furnace.slots[0]);
            add_furnace_output(&mut furnace, output_item_id, output_count);
            furnace
                .recipes_used
                .entry(recipe.id.as_str().to_string())
                .and_modify(|count| *count = count.saturating_add(1))
                .or_insert(1);
            furnace.cook_progress = 0;
        }
    } else if furnace.slots[1].is_empty() {
        furnace.cook_progress = furnace.cook_progress.saturating_sub(2).max(0);
    } else {
        furnace.cook_progress = 0;
    }

    tick_result(furnace, before_slots, before_data)
}

fn tick_result(
    furnace: FurnaceBlockEntity,
    before_slots: [FurnaceSlot; 3],
    before_data: [(i16, i16); 4],
) -> FurnaceTickResult {
    let slots_changed = furnace.slots != before_slots;
    let data_changed = changed_furnace_data(before_data, furnace_data_values(&furnace));
    FurnaceTickResult {
        furnace,
        slots_changed,
        data_changed,
    }
}

fn furnace_player_slot(menu_slot: usize) -> Option<usize> {
    match menu_slot {
        3..=29 => Some(9 + (menu_slot - 3)),
        30..=38 => Some(36 + (menu_slot - 30)),
        _ => None,
    }
}

fn furnace_menu_stack(
    furnace: &FurnaceBlockEntity,
    inventory: &PlayerInventory,
    menu_slot: usize,
) -> Option<ItemStack> {
    match menu_slot {
        0..=2 => Some(furnace_slot_to_stack(&furnace.slots[menu_slot])),
        _ => furnace_player_slot(menu_slot).map(|slot| inventory.slots[slot].clone()),
    }
}

fn set_furnace_menu_stack(
    furnace: &mut FurnaceBlockEntity,
    inventory: &mut PlayerInventory,
    menu_slot: usize,
    stack: ItemStack,
) -> bool {
    match menu_slot {
        0..=2 => {
            furnace.slots[menu_slot] = stack_to_furnace_slot(&stack);
            true
        }
        _ => {
            let Some(slot) = furnace_player_slot(menu_slot) else {
                return false;
            };
            inventory.slots[slot] = stack;
            true
        }
    }
}

fn can_place_in_furnace_menu_slot(
    recipes: &[Recipe],
    items: &ItemRegistry,
    tags: &TagsData,
    kind: FurnaceKind,
    menu_slot: usize,
    stack: &ItemStack,
) -> bool {
    if stack.is_empty() {
        return true;
    }
    match menu_slot {
        0 => find_cooking_recipe_for_item(recipes, items, tags, kind, stack.item_id).is_some(),
        1 => furnace_fuel_ticks(tags, kind, stack.item_id).is_some(),
        2 => false,
        3..=38 => true,
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_pickup_click(
    recipes: &[Recipe],
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    tags: &TagsData,
    kind: FurnaceKind,
    furnace: &mut FurnaceBlockEntity,
    inventory: &mut PlayerInventory,
    carried_item: &mut ItemStack,
    menu_slot: usize,
    button: i8,
) -> bool {
    if menu_slot >= FURNACE_MENU_SLOT_COUNT || !(button == 0 || button == 1) {
        return false;
    }
    let Some(slot_stack) = furnace_menu_stack(furnace, inventory, menu_slot) else {
        return false;
    };
    let cursor = carried_item.clone();
    let max_stack = item_max_stack(
        item_facts,
        items,
        if cursor.is_empty() {
            &slot_stack
        } else {
            &cursor
        },
    );

    if menu_slot == 2 && !slot_stack.is_empty() {
        if cursor.is_empty() {
            *carried_item = slot_stack;
            return set_furnace_menu_stack(furnace, inventory, menu_slot, ItemStack::EMPTY);
        }
        if can_stack(&cursor, &slot_stack) && cursor.count < max_stack {
            let moved = (max_stack - carried_item.count).min(slot_stack.count);
            carried_item.count += moved;
            let mut remaining = slot_stack;
            remaining.count -= moved;
            if remaining.count <= 0 {
                remaining = ItemStack::EMPTY;
            }
            return set_furnace_menu_stack(furnace, inventory, menu_slot, remaining);
        }
        return false;
    }

    let can_place_cursor =
        can_place_in_furnace_menu_slot(recipes, items, tags, kind, menu_slot, &cursor);
    let Some(new_slot) = apply_regular_pickup_slot(
        carried_item,
        slot_stack,
        button,
        max_stack,
        can_place_cursor,
    ) else {
        return false;
    };
    set_furnace_menu_stack(furnace, inventory, menu_slot, new_slot)
}

fn apply_outside_pickup_click(carried_item: &mut ItemStack, button: i8) -> Option<ItemStack> {
    if carried_item.is_empty() {
        return None;
    }
    match button {
        0 => Some(std::mem::take(carried_item)),
        1 => {
            let mut dropped = carried_item.clone();
            dropped.count = 1;
            decrement_stack(carried_item);
            Some(dropped)
        }
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_quick_move_click(
    recipes: &[Recipe],
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    tags: &TagsData,
    kind: FurnaceKind,
    furnace: &mut FurnaceBlockEntity,
    inventory: &mut PlayerInventory,
    menu_slot: usize,
) -> bool {
    if menu_slot >= FURNACE_MENU_SLOT_COUNT {
        return false;
    }
    match menu_slot {
        0..=2 => {
            let original = furnace_slot_to_stack(&furnace.slots[menu_slot]);
            if original.is_empty() {
                return false;
            }
            let max_stack = item_max_stack(item_facts, items, &original);
            let remaining = if menu_slot == 2 {
                inventory.merge_stack_into_ranges_reversed(original.clone(), &[9..=44], max_stack)
            } else {
                inventory.merge_stack(original.clone(), max_stack).0
            };
            furnace.slots[menu_slot] = stack_to_furnace_slot(&remaining);
            remaining != original
        }
        _ => {
            let Some(player_slot) = furnace_player_slot(menu_slot) else {
                return false;
            };
            let original = inventory.slots[player_slot].clone();
            if original.is_empty() {
                return false;
            }
            let target =
                if find_cooking_recipe_for_item(recipes, items, tags, kind, original.item_id)
                    .is_some()
                {
                    Some(0)
                } else if furnace_fuel_ticks(tags, kind, original.item_id).is_some() {
                    Some(1)
                } else {
                    None
                };
            let Some(target) = target else {
                return false;
            };
            inventory.slots[player_slot] = ItemStack::EMPTY;
            let remaining = merge_stack_into_furnace_slot(
                recipes,
                items,
                item_facts,
                tags,
                kind,
                furnace,
                target,
                original.clone(),
            );
            inventory.slots[player_slot] = remaining;
            inventory.slots[player_slot] != original
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn merge_stack_into_furnace_slot(
    recipes: &[Recipe],
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    tags: &TagsData,
    kind: FurnaceKind,
    furnace: &mut FurnaceBlockEntity,
    menu_slot: usize,
    stack: ItemStack,
) -> ItemStack {
    if stack.is_empty()
        || !can_place_in_furnace_menu_slot(recipes, items, tags, kind, menu_slot, &stack)
    {
        return stack;
    }
    let target = &mut furnace.slots[menu_slot];
    let max_stack = item_max_stack(item_facts, items, &stack);
    if target.is_empty() {
        let moved = stack.count.min(max_stack);
        let mut moved_stack = stack.clone();
        moved_stack.count = moved;
        *target = stack_to_furnace_slot(&moved_stack);
        let mut remaining = stack;
        remaining.count -= moved;
        if remaining.count <= 0 {
            ItemStack::EMPTY
        } else {
            remaining
        }
    } else if can_stack(&furnace_slot_to_stack(target), &stack) && target.count < max_stack {
        let moved = (max_stack - target.count).min(stack.count);
        target.count += moved;
        let mut remaining = stack;
        remaining.count -= moved;
        if remaining.count <= 0 {
            ItemStack::EMPTY
        } else {
            remaining
        }
    } else {
        stack
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_swap_click(
    recipes: &[Recipe],
    items: &ItemRegistry,
    tags: &TagsData,
    kind: FurnaceKind,
    furnace: &mut FurnaceBlockEntity,
    inventory: &mut PlayerInventory,
    menu_slot: usize,
    button: i8,
) -> bool {
    if menu_slot >= FURNACE_MENU_SLOT_COUNT || menu_slot == 2 {
        return false;
    }
    let Some(player_slot) = hotbar_swap_slot(button) else {
        return false;
    };
    if furnace_player_slot(menu_slot) == Some(player_slot) {
        return false;
    }
    let Some(clicked) = furnace_menu_stack(furnace, inventory, menu_slot) else {
        return false;
    };
    let swap = inventory.slots[player_slot].clone();
    if !can_place_in_furnace_menu_slot(recipes, items, tags, kind, menu_slot, &swap) {
        return false;
    }
    if !set_furnace_menu_stack(furnace, inventory, menu_slot, swap) {
        return false;
    }
    inventory.slots[player_slot] = clicked;
    true
}

fn apply_throw_click(
    furnace: &mut FurnaceBlockEntity,
    inventory: &mut PlayerInventory,
    menu_slot: usize,
    button: i8,
) -> Option<ItemStack> {
    if menu_slot >= FURNACE_MENU_SLOT_COUNT || menu_slot == 2 {
        return None;
    }
    let mut stack = furnace_menu_stack(furnace, inventory, menu_slot)?;
    let dropped = match button {
        0 if !stack.is_empty() => {
            let mut dropped = stack.clone();
            dropped.count = 1;
            decrement_stack(&mut stack);
            dropped
        }
        1 if !stack.is_empty() => std::mem::take(&mut stack),
        _ => return None,
    };
    set_furnace_menu_stack(furnace, inventory, menu_slot, stack).then_some(dropped)
}

fn apply_regular_pickup_slot(
    carried_item: &mut ItemStack,
    slot_stack: ItemStack,
    button: i8,
    max_stack: i32,
    can_place_carried: bool,
) -> Option<ItemStack> {
    if !(button == 0 || button == 1) {
        return None;
    }

    let cursor = carried_item.clone();
    if button == 0 {
        if cursor.is_empty() {
            if slot_stack.is_empty() {
                return None;
            }
            *carried_item = slot_stack;
            return Some(ItemStack::EMPTY);
        }
        if !can_place_carried {
            return None;
        }
        if slot_stack.is_empty() {
            *carried_item = ItemStack::EMPTY;
            return Some(cursor);
        }
        if can_stack(&slot_stack, &cursor) && slot_stack.count < max_stack {
            let moved = (max_stack - slot_stack.count).min(cursor.count);
            if moved <= 0 {
                return None;
            }
            let mut new_slot = slot_stack;
            new_slot.count += moved;
            carried_item.count -= moved;
            if carried_item.count <= 0 {
                *carried_item = ItemStack::EMPTY;
            }
            return Some(new_slot);
        }
        *carried_item = slot_stack;
        return Some(cursor);
    }

    if cursor.is_empty() {
        if slot_stack.is_empty() {
            return None;
        }
        let moved = (slot_stack.count + 1) / 2;
        let mut new_cursor = slot_stack.clone();
        new_cursor.count = moved;
        let mut remaining = slot_stack;
        remaining.count -= moved;
        if remaining.count <= 0 {
            remaining = ItemStack::EMPTY;
        }
        *carried_item = new_cursor;
        return Some(remaining);
    }
    if !can_place_carried {
        return None;
    }
    if slot_stack.is_empty() {
        let mut one = cursor;
        one.count = 1;
        decrement_stack(carried_item);
        return Some(one);
    }
    if can_stack(&slot_stack, &cursor) && slot_stack.count < max_stack {
        let mut new_slot = slot_stack;
        new_slot.count += 1;
        decrement_stack(carried_item);
        return Some(new_slot);
    }
    None
}

fn hotbar_swap_slot(button: i8) -> Option<usize> {
    (0..=8)
        .contains(&button)
        .then_some(PlayerInventory::HOTBAR_BASE + button as usize)
}

fn furnace_output_room(furnace: &FurnaceBlockEntity, item_id: u32, count: i32) -> bool {
    let output = furnace_slot_to_stack(&furnace.slots[2]);
    output.is_empty()
        || output.item_id == item_id && output.damage.is_none() && output.count + count <= 64
}

pub(in crate::play) fn furnace_output_was_taken(
    before: &FurnaceBlockEntity,
    after: &FurnaceBlockEntity,
) -> bool {
    let before = &before.slots[2];
    let after = &after.slots[2];
    if before.is_empty() {
        return false;
    }
    if after.is_empty() || before.item_id != after.item_id || before.damage != after.damage {
        return true;
    }
    after.count < before.count
}

fn add_furnace_output(furnace: &mut FurnaceBlockEntity, item_id: u32, count: i32) {
    if furnace.slots[2].is_empty() {
        furnace.slots[2] = stack_to_furnace_slot(&ItemStack::new(item_id, count));
    } else {
        furnace.slots[2].count += count;
    }
}

#[cfg(test)]
mod tests;

pub(in crate::play) fn decrement_furnace_slot(stack: &mut FurnaceSlot) {
    stack.count -= 1;
    if stack.count <= 0 {
        *stack = FurnaceSlot::EMPTY;
    }
}

fn consume_furnace_fuel(items: &ItemRegistry, fuel: &mut FurnaceSlot) {
    let fuel_item_id = fuel.item_id;
    decrement_furnace_slot(fuel);
    if !fuel.is_empty() {
        return;
    }
    let is_lava_bucket = items
        .name_of(fuel_item_id)
        .is_some_and(|name| name.as_str() == "minecraft:lava_bucket");
    if !is_lava_bucket {
        return;
    }
    let bucket = Identifier::parse("minecraft:bucket").expect("static identifier");
    if let Some(bucket_item_id) = items.id_of(&bucket) {
        *fuel = stack_to_furnace_slot(&ItemStack::new(bucket_item_id, 1));
    }
}

fn decrement_stack(stack: &mut ItemStack) {
    stack.count -= 1;
    if stack.count <= 0 {
        *stack = ItemStack::EMPTY;
    }
}

fn changed_furnace_data(before: [(i16, i16); 4], after: [(i16, i16); 4]) -> Vec<(i16, i16)> {
    before
        .into_iter()
        .zip(after)
        .filter_map(|(before, after)| (before != after).then_some(after))
        .collect()
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}
