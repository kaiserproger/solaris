use mc_data::Identifier;
use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_data::recipes::Recipe;
use mc_data::tags::TagsData;
use mc_nbt::Tag;
use mc_protocol::packets::play::ItemStack;

use crate::play::inventory::{
    PlayerInventory, apply_regular_pickup_slot, apply_regular_swap_slot, apply_regular_throw_slot,
    armor_entry_for_item, armor_slot_for_kind, can_place_in_player_slot, can_stack,
    hotbar_swap_slot, item_max_stack, pickup_click_max_stack, player_swap_slot,
};
use crate::play::recipes::ingredient_accepts_item;

pub(in crate::play) const CRAFTING_MENU_TYPE_ID: i32 = 12;
pub(in crate::play) const CRAFTING_MENU_SLOT_COUNT: usize = 46;

#[derive(Debug, Clone)]
pub(in crate::play) struct CraftingTableWindow {
    pub(in crate::play) container_id: i32,
    pub(in crate::play) state_id: i32,
    pub(in crate::play) input: [ItemStack; 9],
    pub(in crate::play) result: ItemStack,
}

impl CraftingTableWindow {
    pub(in crate::play) fn new(container_id: i32) -> Self {
        Self {
            container_id,
            state_id: 1,
            input: std::array::from_fn(|_| ItemStack::EMPTY),
            result: ItemStack::EMPTY,
        }
    }
}

pub(in crate::play) fn crafting_menu_title_nbt() -> Result<Vec<u8>, mc_protocol::CodecError> {
    let mut out = Vec::new();
    mc_nbt::write_network(
        &mut out,
        &Tag::Compound(vec![(
            "text".to_string(),
            Tag::String("Crafting".to_string()),
        )]),
    )?;
    Ok(out)
}

pub(in crate::play) fn crafting_player_slot(menu_slot: usize) -> Option<usize> {
    match menu_slot {
        10..=36 => Some(9 + (menu_slot - 10)),
        37..=45 => Some(36 + (menu_slot - 37)),
        _ => None,
    }
}

fn shaped_recipe_matches(
    items: &ItemRegistry,
    tags: &TagsData,
    input: &[ItemStack; 9],
    shaped: &mc_data::recipes::ShapedRecipe,
) -> bool {
    let height = shaped.pattern.len();
    let width = shaped
        .pattern
        .iter()
        .map(|row| row.chars().count())
        .max()
        .unwrap_or(0);
    if height == 0 || width == 0 || height > 3 || width > 3 {
        return false;
    }

    for top in 0..=(3 - height) {
        'left: for left in 0..=(3 - width) {
            for row in 0..3 {
                for col in 0..3 {
                    let stack = &input[row * 3 + col];
                    let ingredient =
                        if row >= top && row < top + height && col >= left && col < left + width {
                            shaped
                                .pattern
                                .get(row - top)
                                .and_then(|pattern_row| pattern_row.chars().nth(col - left))
                                .filter(|ch| *ch != ' ')
                                .and_then(|ch| shaped.key.get(&ch))
                        } else {
                            None
                        };
                    match ingredient {
                        Some(ingredient)
                            if !stack.is_empty()
                                && ingredient_accepts_item(
                                    items,
                                    tags,
                                    stack.item_id,
                                    ingredient,
                                ) => {}
                        None if stack.is_empty() => {}
                        _ => continue 'left,
                    }
                }
            }
            return true;
        }
    }
    false
}

fn shapeless_recipe_matches(
    items: &ItemRegistry,
    tags: &TagsData,
    input: &[ItemStack; 9],
    shapeless: &mc_data::recipes::ShapelessRecipe,
) -> bool {
    let stacks: Vec<_> = input.iter().filter(|stack| !stack.is_empty()).collect();
    if stacks.len() != shapeless.ingredients.len() {
        return false;
    }
    let mut used = vec![false; shapeless.ingredients.len()];
    for stack in stacks {
        let Some((idx, _)) = shapeless
            .ingredients
            .iter()
            .enumerate()
            .find(|(idx, ingredient)| {
                !used[*idx] && ingredient_accepts_item(items, tags, stack.item_id, ingredient)
            })
        else {
            return false;
        };
        used[idx] = true;
    }
    true
}

fn crafting_recipe_matches(
    items: &ItemRegistry,
    tags: &TagsData,
    input: &[ItemStack; 9],
    recipe: &mc_data::recipes::Recipe,
) -> bool {
    match &recipe.kind {
        mc_data::recipes::RecipeKind::Shaped(shaped) => {
            shaped_recipe_matches(items, tags, input, shaped)
        }
        mc_data::recipes::RecipeKind::Shapeless(shapeless) => {
            shapeless_recipe_matches(items, tags, input, shapeless)
        }
        mc_data::recipes::RecipeKind::Smelting(_)
        | mc_data::recipes::RecipeKind::Blasting(_)
        | mc_data::recipes::RecipeKind::Smoking(_)
        | mc_data::recipes::RecipeKind::CampfireCooking(_)
        | mc_data::recipes::RecipeKind::Stonecutting(_) => false,
    }
}

pub(in crate::play) fn repair_item_crafting_result(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    input: &[ItemStack; 9],
) -> Option<ItemStack> {
    let mut occupied = input.iter().filter(|stack| !stack.is_empty());
    let first = occupied.next()?;
    let second = occupied.next()?;
    if occupied.next().is_some()
        || first.item_id != second.item_id
        || first.count != 1
        || second.count != 1
    {
        return None;
    }

    let first_damage = first.damage?;
    let second_damage = second.damage?;
    let item = items.name_of(first.item_id)?;
    let max_damage = item_facts.get(item)?.max_damage?;
    let max_damage = i32::try_from(max_damage).ok()?;
    if max_damage <= 0 {
        return None;
    }

    let remaining = max_damage.saturating_sub(first_damage)
        + max_damage.saturating_sub(second_damage)
        + max_damage / 20;
    Some(ItemStack::new(first.item_id, 1).with_damage((max_damage - remaining).max(0)))
}

pub(in crate::play) fn crafting_result_from_input(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    tags: &TagsData,
    recipes: &[mc_data::recipes::Recipe],
    input: &[ItemStack; 9],
) -> ItemStack {
    if let Some(result) = repair_item_crafting_result(items, item_facts, input) {
        return result;
    }
    recipes
        .iter()
        .find(|recipe| crafting_recipe_matches(items, tags, input, recipe))
        .and_then(|recipe| {
            let item_id = items.id_of(&recipe.result.item)?;
            let count = i32::try_from(recipe.result.count).ok()?;
            (count > 0).then(|| ItemStack::new(item_id, count))
        })
        .unwrap_or(ItemStack::EMPTY)
}

pub(in crate::play) fn refresh_crafting_result(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    tags: &TagsData,
    recipes: &[mc_data::recipes::Recipe],
    window: &mut CraftingTableWindow,
) {
    window.result = crafting_result_from_input(items, item_facts, tags, recipes, &window.input);
}

pub(in crate::play) fn crafting_table_input_projection(
    input: &[ItemStack; 9],
) -> Option<Box<[ItemStack; 9]>> {
    input
        .iter()
        .any(|stack| !stack.is_empty())
        .then(|| Box::new(input.clone()))
}

pub(in crate::play) fn crafting_table_input_from_projection(
    input: Option<Box<[ItemStack; 9]>>,
) -> [ItemStack; 9] {
    input
        .map(|input| *input)
        .unwrap_or_else(|| std::array::from_fn(|_| ItemStack::EMPTY))
}

pub(in crate::play) fn inventory_crafting_input(inventory: &PlayerInventory) -> [ItemStack; 9] {
    let mut input = std::array::from_fn(|_| ItemStack::EMPTY);
    input[0] = inventory.slots[1].clone();
    input[1] = inventory.slots[2].clone();
    input[3] = inventory.slots[3].clone();
    input[4] = inventory.slots[4].clone();
    input
}

pub(in crate::play) fn refresh_inventory_crafting_result(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    tags: &TagsData,
    recipes: &[mc_data::recipes::Recipe],
    inventory: &mut PlayerInventory,
) {
    let input = inventory_crafting_input(inventory);
    inventory.slots[0] = crafting_result_from_input(items, item_facts, tags, recipes, &input);
}

pub(in crate::play) fn crafting_wire_items(
    window: &CraftingTableWindow,
    inventory: &PlayerInventory,
) -> Vec<ItemStack> {
    let mut items = Vec::with_capacity(CRAFTING_MENU_SLOT_COUNT);
    items.push(window.result.clone());
    items.extend(window.input.iter().cloned());
    items.extend((9..=35).map(|slot| inventory.slots[slot].clone()));
    items.extend((36..=44).map(|slot| inventory.slots[slot].clone()));
    items
}

fn crafting_menu_stack(
    window: &CraftingTableWindow,
    inventory: &PlayerInventory,
    menu_slot: usize,
) -> Option<ItemStack> {
    match menu_slot {
        0 => Some(window.result.clone()),
        1..=9 => Some(window.input[menu_slot - 1].clone()),
        _ => crafting_player_slot(menu_slot).map(|slot| inventory.slots[slot].clone()),
    }
}

fn set_crafting_menu_stack(
    window: &mut CraftingTableWindow,
    inventory: &mut PlayerInventory,
    menu_slot: usize,
    stack: ItemStack,
) -> bool {
    match menu_slot {
        1..=9 => {
            window.input[menu_slot - 1] = stack;
            true
        }
        _ => {
            let Some(slot) = crafting_player_slot(menu_slot) else {
                return false;
            };
            inventory.slots[slot] = stack;
            true
        }
    }
}

fn can_place_in_crafting_menu_slot(
    items: &ItemRegistry,
    menu_slot: usize,
    stack: &ItemStack,
) -> bool {
    if stack.is_empty() {
        return true;
    }
    match menu_slot {
        0 => false,
        1..=9 => true,
        _ => crafting_player_slot(menu_slot)
            .is_some_and(|slot| can_place_in_player_slot(items, slot, stack)),
    }
}

fn crafting_remainder_for_item(items: &ItemRegistry, item_id: u32) -> Option<ItemStack> {
    let name = items.name_of(item_id)?;
    let bucket = Identifier::parse("minecraft:bucket").expect("static identifier");
    if name.path().ends_with("_bucket") || name.as_str() == "minecraft:milk_bucket" {
        items
            .id_of(&bucket)
            .map(|bucket_id| ItemStack::new(bucket_id, 1))
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn consume_crafting_ingredients(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    tags: &TagsData,
    recipes: &[Recipe],
    window: &mut CraftingTableWindow,
    inventory: &mut PlayerInventory,
) -> Vec<ItemStack> {
    let consumed: Vec<_> = window
        .input
        .iter()
        .map(|stack| (!stack.is_empty()).then_some(stack.item_id))
        .collect();
    let mut discarded_remainders = Vec::new();
    for (idx, item_id) in consumed.into_iter().enumerate() {
        let Some(item_id) = item_id else {
            continue;
        };
        window.input[idx].count -= 1;
        if window.input[idx].count <= 0 {
            window.input[idx] =
                crafting_remainder_for_item(items, item_id).unwrap_or(ItemStack::EMPTY);
        } else if let Some(remainder) = crafting_remainder_for_item(items, item_id) {
            let max_stack = item_max_stack(item_facts, items, &remainder);
            let (remaining, _) = inventory.merge_stack(remainder, max_stack);
            if !remaining.is_empty() {
                discarded_remainders.push(remaining);
            }
        }
    }
    refresh_crafting_result(items, item_facts, tags, recipes, window);
    discarded_remainders
}

fn consume_inventory_crafting_ingredients(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    tags: &TagsData,
    recipes: &[Recipe],
    inventory: &mut PlayerInventory,
) -> Vec<ItemStack> {
    let mut discarded_remainders = Vec::new();
    for slot in 1..=4 {
        let item_id = (!inventory.slots[slot].is_empty()).then_some(inventory.slots[slot].item_id);
        let Some(item_id) = item_id else {
            continue;
        };
        inventory.slots[slot].count -= 1;
        if inventory.slots[slot].count <= 0 {
            inventory.slots[slot] =
                crafting_remainder_for_item(items, item_id).unwrap_or(ItemStack::EMPTY);
        } else if let Some(remainder) = crafting_remainder_for_item(items, item_id) {
            let max_stack = item_max_stack(item_facts, items, &remainder);
            let (remaining, _) = inventory.merge_stack(remainder, max_stack);
            if !remaining.is_empty() {
                discarded_remainders.push(remaining);
            }
        }
    }
    refresh_inventory_crafting_result(items, item_facts, tags, recipes, inventory);
    discarded_remainders
}

impl CraftingTableWindow {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::play) fn apply_pickup_click(
        &mut self,
        items: &ItemRegistry,
        item_facts: &ItemFactsTable,
        tags: &TagsData,
        recipes: &[Recipe],
        inventory: &mut PlayerInventory,
        carried_item: &mut ItemStack,
        menu_slot: usize,
        button: i8,
    ) -> (bool, Vec<ItemStack>) {
        if menu_slot >= CRAFTING_MENU_SLOT_COUNT || !(button == 0 || button == 1) {
            return (false, Vec::new());
        }
        if menu_slot == 0 {
            return self.take_result(items, item_facts, tags, recipes, inventory, carried_item);
        }
        let Some(slot_stack) = crafting_menu_stack(self, inventory, menu_slot) else {
            return (false, Vec::new());
        };
        let max_stack = pickup_click_max_stack(item_facts, items, carried_item, &slot_stack);
        let can_place_cursor = can_place_in_crafting_menu_slot(items, menu_slot, carried_item);
        let Some(new_slot) = apply_regular_pickup_slot(
            carried_item,
            slot_stack,
            button,
            max_stack,
            can_place_cursor,
        ) else {
            return (false, Vec::new());
        };
        let changed = set_crafting_menu_stack(self, inventory, menu_slot, new_slot);
        if changed {
            refresh_crafting_result(items, item_facts, tags, recipes, self);
        }
        (changed, Vec::new())
    }

    #[allow(clippy::too_many_arguments)]
    fn take_result(
        &mut self,
        items: &ItemRegistry,
        item_facts: &ItemFactsTable,
        tags: &TagsData,
        recipes: &[Recipe],
        inventory: &mut PlayerInventory,
        carried_item: &mut ItemStack,
    ) -> (bool, Vec<ItemStack>) {
        let result = self.result.clone();
        if result.is_empty() {
            return (false, Vec::new());
        }
        let max_stack = item_max_stack(item_facts, items, &result);
        if carried_item.is_empty() {
            *carried_item = result;
        } else if can_stack(carried_item, &result) && carried_item.count + result.count <= max_stack
        {
            carried_item.count += result.count;
        } else {
            return (false, Vec::new());
        }
        let discarded =
            consume_crafting_ingredients(items, item_facts, tags, recipes, self, inventory);
        (true, discarded)
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::play) fn apply_swap_click(
        &mut self,
        items: &ItemRegistry,
        item_facts: &ItemFactsTable,
        tags: &TagsData,
        recipes: &[Recipe],
        inventory: &mut PlayerInventory,
        menu_slot: usize,
        button: i8,
    ) -> bool {
        if menu_slot >= CRAFTING_MENU_SLOT_COUNT || menu_slot == 0 {
            return false;
        }
        let Some(player_slot) = hotbar_swap_slot(button) else {
            return false;
        };
        if crafting_player_slot(menu_slot) == Some(player_slot) {
            return false;
        }
        let Some(clicked) = crafting_menu_stack(self, inventory, menu_slot) else {
            return false;
        };
        let swap = inventory.slots[player_slot].clone();
        let can_place_swap = can_place_in_crafting_menu_slot(items, menu_slot, &swap);
        let can_place_clicked = can_place_in_player_slot(items, player_slot, &clicked);
        let Some((new_clicked, new_swap)) =
            apply_regular_swap_slot(clicked, swap, can_place_swap, can_place_clicked)
        else {
            return false;
        };
        if !set_crafting_menu_stack(self, inventory, menu_slot, new_clicked) {
            return false;
        }
        inventory.slots[player_slot] = new_swap;
        refresh_crafting_result(items, item_facts, tags, recipes, self);
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::play) fn apply_throw_click(
        &mut self,
        items: &ItemRegistry,
        item_facts: &ItemFactsTable,
        tags: &TagsData,
        recipes: &[Recipe],
        inventory: &mut PlayerInventory,
        menu_slot: usize,
        button: i8,
    ) -> Option<ItemStack> {
        if menu_slot >= CRAFTING_MENU_SLOT_COUNT || menu_slot == 0 {
            return None;
        }
        let (stack, dropped) =
            apply_regular_throw_slot(crafting_menu_stack(self, inventory, menu_slot)?, button)?;
        if !set_crafting_menu_stack(self, inventory, menu_slot, stack) {
            return None;
        }
        refresh_crafting_result(items, item_facts, tags, recipes, self);
        Some(dropped)
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::play) fn apply_quick_move_click(
        &mut self,
        items: &ItemRegistry,
        item_facts: &ItemFactsTable,
        tags: &TagsData,
        recipes: &[Recipe],
        inventory: &mut PlayerInventory,
        menu_slot: usize,
    ) -> (bool, Vec<ItemStack>) {
        if menu_slot >= CRAFTING_MENU_SLOT_COUNT {
            return (false, Vec::new());
        }
        if menu_slot == 0 {
            let result = self.result.clone();
            if result.is_empty() {
                return (false, Vec::new());
            }
            let max_stack = item_max_stack(item_facts, items, &result);
            let mut merged = inventory.clone();
            let (remaining, _) = merged.merge_stack(result, max_stack);
            if !remaining.is_empty() {
                return (false, Vec::new());
            }
            *inventory = merged;
            let discarded =
                consume_crafting_ingredients(items, item_facts, tags, recipes, self, inventory);
            return (true, discarded);
        }
        if (1..=9).contains(&menu_slot) {
            let input_slot = menu_slot - 1;
            let original = self.input[input_slot].clone();
            if original.is_empty() {
                return (false, Vec::new());
            }
            let max_stack = item_max_stack(item_facts, items, &original);
            let (remaining, _) = inventory.merge_stack(original.clone(), max_stack);
            self.input[input_slot] = remaining;
            refresh_crafting_result(items, item_facts, tags, recipes, self);
            return (self.input[input_slot] != original, Vec::new());
        }
        let Some(player_slot) = crafting_player_slot(menu_slot) else {
            return (false, Vec::new());
        };
        let original = inventory.slots[player_slot].clone();
        if original.is_empty() {
            return (false, Vec::new());
        }
        inventory.slots[player_slot] = ItemStack::EMPTY;
        let ranges = if (9..=35).contains(&player_slot) {
            [36..=44]
        } else {
            [9..=35]
        };
        let remaining = inventory.merge_stack_into_ranges(
            original.clone(),
            &ranges,
            item_max_stack(item_facts, items, &original),
        );
        inventory.slots[player_slot] = remaining;
        (inventory.slots[player_slot] != original, Vec::new())
    }
}

impl PlayerInventory {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::play) fn apply_crafting_pickup_click(
        &mut self,
        items: &ItemRegistry,
        item_facts: &ItemFactsTable,
        tags: &TagsData,
        recipes: &[Recipe],
        carried_item: &mut ItemStack,
        slot: usize,
        button: i8,
    ) -> (bool, Vec<ItemStack>) {
        if slot >= self.slots.len() || !(button == 0 || button == 1) {
            return (false, Vec::new());
        }
        if slot == 0 {
            return self.take_crafting_result(items, item_facts, tags, recipes, carried_item);
        }

        let slot_stack = self.slots[slot].clone();
        let max_stack = pickup_click_max_stack(item_facts, items, carried_item, &slot_stack);
        let can_place_cursor = can_place_in_player_slot(items, slot, carried_item);
        let Some(new_slot) = apply_regular_pickup_slot(
            carried_item,
            slot_stack,
            button,
            max_stack,
            can_place_cursor,
        ) else {
            return (false, Vec::new());
        };
        self.slots[slot] = new_slot;
        if (1..=4).contains(&slot) {
            refresh_inventory_crafting_result(items, item_facts, tags, recipes, self);
        }
        (true, Vec::new())
    }

    fn take_crafting_result(
        &mut self,
        items: &ItemRegistry,
        item_facts: &ItemFactsTable,
        tags: &TagsData,
        recipes: &[Recipe],
        carried_item: &mut ItemStack,
    ) -> (bool, Vec<ItemStack>) {
        refresh_inventory_crafting_result(items, item_facts, tags, recipes, self);
        let result = self.slots[0].clone();
        if result.is_empty() {
            return (false, Vec::new());
        }
        let max_stack = item_max_stack(item_facts, items, &result);
        if carried_item.is_empty() {
            *carried_item = result;
        } else if can_stack(carried_item, &result) && carried_item.count + result.count <= max_stack
        {
            carried_item.count += result.count;
        } else {
            return (false, Vec::new());
        }
        let discarded =
            consume_inventory_crafting_ingredients(items, item_facts, tags, recipes, self);
        (true, discarded)
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::play) fn apply_crafting_swap_click(
        &mut self,
        items: &ItemRegistry,
        item_facts: &ItemFactsTable,
        tags: &TagsData,
        recipes: &[Recipe],
        slot: usize,
        button: i8,
    ) -> bool {
        if slot == 0 || slot >= self.slots.len() {
            return false;
        }
        let Some(swap_slot) = player_swap_slot(button) else {
            return false;
        };
        if slot == swap_slot {
            return false;
        }
        let clicked = self.slots[slot].clone();
        let swap = self.slots[swap_slot].clone();
        let can_place_swap = can_place_in_player_slot(items, slot, &swap);
        let can_place_clicked = can_place_in_player_slot(items, swap_slot, &clicked);
        let Some((new_clicked, new_swap)) =
            apply_regular_swap_slot(clicked, swap, can_place_swap, can_place_clicked)
        else {
            return false;
        };
        self.slots[slot] = new_clicked;
        self.slots[swap_slot] = new_swap;
        if (1..=4).contains(&slot) || (1..=4).contains(&swap_slot) {
            refresh_inventory_crafting_result(items, item_facts, tags, recipes, self);
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::play) fn apply_crafting_throw_click(
        &mut self,
        items: &ItemRegistry,
        item_facts: &ItemFactsTable,
        tags: &TagsData,
        recipes: &[Recipe],
        slot: usize,
        button: i8,
    ) -> Option<ItemStack> {
        if slot == 0 || slot >= self.slots.len() {
            return None;
        }
        let (new_slot, dropped) = apply_regular_throw_slot(self.slots[slot].clone(), button)?;
        self.slots[slot] = new_slot;
        if (1..=4).contains(&slot) {
            refresh_inventory_crafting_result(items, item_facts, tags, recipes, self);
        }
        Some(dropped)
    }

    pub(in crate::play) fn apply_crafting_quick_move_click(
        &mut self,
        items: &ItemRegistry,
        item_facts: &ItemFactsTable,
        tags: &TagsData,
        recipes: &[Recipe],
        slot: usize,
    ) -> (bool, Vec<ItemStack>) {
        if slot >= self.slots.len() || self.slots[slot].is_empty() {
            return (false, Vec::new());
        }
        if slot == 0 {
            let result = self.slots[0].clone();
            let max_stack = item_max_stack(item_facts, items, &result);
            let mut merged = self.clone();
            let (remaining, _) = merged.merge_stack(result, max_stack);
            if !remaining.is_empty() {
                return (false, Vec::new());
            }
            *self = merged;
            let discarded =
                consume_inventory_crafting_ingredients(items, item_facts, tags, recipes, self);
            return (true, discarded);
        }

        let original = self.slots[slot].clone();
        let max_stack = item_max_stack(item_facts, items, &original);
        if !(5..=8).contains(&slot)
            && let Some(entry) = armor_entry_for_item(items, original.item_id)
        {
            let armor_slot = armor_slot_for_kind(entry.slot);
            if self.slots[armor_slot].is_empty() {
                let mut equipped = original.clone();
                equipped.count = 1;
                self.slots[armor_slot] = equipped;
                if original.count <= 1 {
                    self.slots[slot] = ItemStack::EMPTY;
                } else {
                    self.slots[slot].count -= 1;
                }
                if (1..=4).contains(&slot) {
                    refresh_inventory_crafting_result(items, item_facts, tags, recipes, self);
                }
                return (true, Vec::new());
            }
        }

        self.slots[slot] = ItemStack::EMPTY;
        let remaining = if (36..=44).contains(&slot) {
            self.merge_stack_into_ranges(original.clone(), &[9..=35], max_stack)
        } else if (9..=35).contains(&slot) {
            self.merge_stack_into_ranges(original.clone(), &[36..=44], max_stack)
        } else {
            self.merge_stack_into_ranges(original.clone(), &[36..=44, 9..=35], max_stack)
        };
        self.slots[slot] = remaining;
        if (1..=4).contains(&slot) {
            refresh_inventory_crafting_result(items, item_facts, tags, recipes, self);
        }
        (self.slots[slot] != original, Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use mc_data::Identifier;
    use mc_data::item_components::{ItemFacts, ItemFactsTable};
    use mc_data::items::{ItemRegistry, ItemReport};
    use mc_data::tags::TagsData;
    use mc_protocol::packets::play::ItemStack;

    use super::{CraftingTableWindow, crafting_remainder_for_item, repair_item_crafting_result};
    use crate::play::inventory::PlayerInventory;

    #[test]
    fn repair_combines_remaining_durability_and_five_percent_bonus() {
        let item = Identifier::parse("minecraft:iron_pickaxe").unwrap();
        let items = ItemRegistry::from_report(&[ItemReport {
            id: item.clone(),
            protocol_id: 42,
        }]);
        let facts = ItemFactsTable::from_entries([(
            item,
            ItemFacts {
                max_damage: Some(100),
                ..ItemFacts::default()
            },
        )]);
        let mut input = std::array::from_fn(|_| ItemStack::EMPTY);
        input[0] = ItemStack::new(42, 1).with_damage(80);
        input[8] = ItemStack::new(42, 1).with_damage(80);

        assert_eq!(
            repair_item_crafting_result(&items, &facts, &input),
            Some(ItemStack::new(42, 1).with_damage(55))
        );
    }

    #[test]
    fn inventory_result_quick_move_is_transactional_when_only_part_fits() {
        let items = ItemRegistry::from_report(&[]);
        let item_facts = ItemFactsTable::default();
        let tags = TagsData::default();
        let mut inventory = PlayerInventory::empty();
        inventory.slots[0] = ItemStack::new(42, 4);
        inventory.slots[1] = ItemStack::new(7, 1);
        inventory.slots[9] = ItemStack::new(42, 62);
        for slot in 10..=44 {
            inventory.slots[slot] = ItemStack::new(99, 64);
        }
        let before = inventory.clone();

        let (changed, discarded_remainders) =
            inventory.apply_crafting_quick_move_click(&items, &item_facts, &tags, &[], 0);

        assert!(!changed);
        assert!(discarded_remainders.is_empty());
        assert_eq!(inventory.slots, before.slots);
    }

    #[test]
    fn crafting_table_result_pickup_consumes_the_matching_grid() {
        let items = ItemRegistry::from_report(&[]);
        let item_facts = ItemFactsTable::default();
        let tags = TagsData::default();
        let mut inventory = PlayerInventory::empty();
        let mut carried_item = ItemStack::EMPTY;
        let mut window = CraftingTableWindow::new(7);
        window.input[0] = ItemStack::new(7, 1);
        window.result = ItemStack::new(42, 4);

        let (changed, discarded_remainders) = window.apply_pickup_click(
            &items,
            &item_facts,
            &tags,
            &[],
            &mut inventory,
            &mut carried_item,
            0,
            0,
        );

        assert!(changed);
        assert!(discarded_remainders.is_empty());
        assert_eq!(carried_item, ItemStack::new(42, 4));
        assert!(window.input.iter().all(ItemStack::is_empty));
        assert!(window.result.is_empty());
    }

    #[test]
    fn filled_bucket_crafting_remainder_is_an_empty_bucket() {
        let bucket = Identifier::parse("minecraft:bucket").unwrap();
        let milk_bucket = Identifier::parse("minecraft:milk_bucket").unwrap();
        let items = ItemRegistry::from_report(&[
            ItemReport {
                id: bucket,
                protocol_id: 1,
            },
            ItemReport {
                id: milk_bucket,
                protocol_id: 2,
            },
        ]);

        assert_eq!(
            crafting_remainder_for_item(&items, 2),
            Some(ItemStack::new(1, 1))
        );
    }

    #[test]
    fn crafting_table_player_quick_move_preserves_the_stack() {
        let items = ItemRegistry::from_report(&[]);
        let item_facts = ItemFactsTable::default();
        let tags = TagsData::default();
        let mut inventory = PlayerInventory::empty();
        inventory.slots[9] = ItemStack::new(42, 3);
        let mut window = CraftingTableWindow::new(7);

        let (changed, discarded_remainders) =
            window.apply_quick_move_click(&items, &item_facts, &tags, &[], &mut inventory, 10);

        assert!(changed);
        assert!(discarded_remainders.is_empty());
        assert!(inventory.slots[9].is_empty());
        assert_eq!(inventory.slots[36], ItemStack::new(42, 3));
    }

    #[test]
    fn inventory_main_quick_move_keeps_hotbar_overflow_in_source_slot() {
        let items = ItemRegistry::from_report(&[]);
        let item_facts = ItemFactsTable::default();
        let tags = TagsData::default();
        let mut inventory = PlayerInventory::empty();
        inventory.slots[9] = ItemStack::new(42, 3);
        inventory.slots[36] = ItemStack::new(42, 63);
        for slot in 37..=44 {
            inventory.slots[slot] = ItemStack::new(99, 64);
        }

        let (changed, discarded_remainders) =
            inventory.apply_crafting_quick_move_click(&items, &item_facts, &tags, &[], 9);

        assert!(changed);
        assert!(discarded_remainders.is_empty());
        assert_eq!(inventory.slots[36], ItemStack::new(42, 64));
        assert_eq!(inventory.slots[9], ItemStack::new(42, 2));
        assert!(inventory.slots[10].is_empty());
    }

    #[test]
    fn crafting_table_grid_quick_move_returns_input_to_inventory() {
        let items = ItemRegistry::from_report(&[]);
        let item_facts = ItemFactsTable::default();
        let tags = TagsData::default();
        let mut inventory = PlayerInventory::empty();
        let mut window = CraftingTableWindow::new(7);
        window.input[0] = ItemStack::new(42, 3);

        let (changed, discarded_remainders) =
            window.apply_quick_move_click(&items, &item_facts, &tags, &[], &mut inventory, 1);

        assert!(changed);
        assert!(discarded_remainders.is_empty());
        assert!(window.input[0].is_empty());
        assert_eq!(inventory.slots[9], ItemStack::new(42, 3));
    }
}
