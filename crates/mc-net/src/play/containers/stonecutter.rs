use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_data::recipes::{Recipe, RecipeKind};
use mc_data::tags::TagsData;
use mc_nbt::Tag;
use mc_protocol::packets::play::ItemStack;
use mc_world::BlockPos;

use crate::play::inventory::{PlayerInventory, can_stack, item_max_stack};
use crate::play::recipes::{ingredient_accepts_item, stonecutter_recipe_entry};

pub(in crate::play) const STONECUTTER_MENU_TYPE_ID: i32 = 24;
pub(in crate::play) const STONECUTTER_MENU_SLOT_COUNT: usize = 38;

#[derive(Debug, Clone)]
pub(in crate::play) struct StonecutterWindow {
    pub(in crate::play) container_id: i32,
    pub(in crate::play) state_id: i32,
    pub(in crate::play) input: ItemStack,
    pub(in crate::play) result: ItemStack,
    pub(in crate::play) selected_recipe: Option<usize>,
}

impl StonecutterWindow {
    pub(in crate::play) fn at_position(container_id: i32, _position: BlockPos) -> Self {
        Self {
            container_id,
            state_id: 1,
            input: ItemStack::EMPTY,
            result: ItemStack::EMPTY,
            selected_recipe: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::play) enum StonecutterClickAction {
    Pickup { slot: usize, button: i8 },
    QuickMove { slot: usize },
    Unsupported,
}

pub(in crate::play) struct StonecutterClickInput<'a> {
    pub(in crate::play) recipes: &'a [Recipe],
    pub(in crate::play) items: &'a ItemRegistry,
    pub(in crate::play) item_facts: &'a ItemFactsTable,
    pub(in crate::play) tags: &'a TagsData,
    pub(in crate::play) window: StonecutterWindow,
    pub(in crate::play) inventory: PlayerInventory,
    pub(in crate::play) carried_item: ItemStack,
    pub(in crate::play) action: StonecutterClickAction,
}

pub(in crate::play) struct StonecutterClickPlan {
    pub(in crate::play) window: StonecutterWindow,
    pub(in crate::play) inventory: PlayerInventory,
    pub(in crate::play) carried_item: ItemStack,
}

pub(in crate::play) fn stonecutter_menu_title_nbt() -> Result<Vec<u8>, mc_protocol::CodecError> {
    let mut out = Vec::new();
    mc_nbt::write_network(
        &mut out,
        &Tag::Compound(vec![(
            "text".to_string(),
            Tag::String("Stonecutter".to_string()),
        )]),
    )?;
    Ok(out)
}

pub(in crate::play) fn stonecutter_input_array(input: &ItemStack) -> [ItemStack; 9] {
    let mut projected = std::array::from_fn(|_| ItemStack::EMPTY);
    projected[0] = input.clone();
    projected
}

pub(in crate::play) fn stonecutter_input_projection(
    input: &ItemStack,
) -> Option<Box<[ItemStack; 9]>> {
    (!input.is_empty()).then(|| Box::new(stonecutter_input_array(input)))
}

pub(in crate::play) fn stonecutter_input_from_projection(
    input: Option<Box<[ItemStack; 9]>>,
) -> ItemStack {
    input
        .map(|input| input[0].clone())
        .unwrap_or(ItemStack::EMPTY)
}

pub(in crate::play) fn stonecutter_player_slot(menu_slot: usize) -> Option<usize> {
    (2..STONECUTTER_MENU_SLOT_COUNT)
        .contains(&menu_slot)
        .then_some(menu_slot + 7)
}

fn recipe_has_advertised_offer(
    recipe: &Recipe,
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    tags: &TagsData,
    input: &ItemStack,
) -> bool {
    if stonecutter_recipe_entry(recipe, items, item_facts).is_none() {
        return false;
    }
    let RecipeKind::Stonecutting(stonecutting) = &recipe.kind else {
        return false;
    };
    ingredient_accepts_item(items, tags, input.item_id, &stonecutting.ingredient)
}

fn stonecutter_recipe_for_selection<'a>(
    recipes: &'a [Recipe],
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    tags: &TagsData,
    input: &ItemStack,
    selection: usize,
) -> Option<&'a Recipe> {
    if input.is_empty() {
        return None;
    }
    recipes
        .iter()
        .filter(|recipe| recipe_has_advertised_offer(recipe, items, item_facts, tags, input))
        .nth(selection)
}

pub(in crate::play) fn refresh_stonecutter_result(
    recipes: &[Recipe],
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    tags: &TagsData,
    window: &mut StonecutterWindow,
) {
    window.result = window
        .selected_recipe
        .and_then(|selection| {
            stonecutter_recipe_for_selection(
                recipes,
                items,
                item_facts,
                tags,
                &window.input,
                selection,
            )
        })
        .and_then(|recipe| {
            let item_id = items.id_of(&recipe.result.item)?;
            let count = i32::try_from(recipe.result.count).ok()?;
            (count > 0).then(|| ItemStack::new(item_id, count))
        })
        .unwrap_or(ItemStack::EMPTY);
}

pub(in crate::play) fn set_stonecutter_input(
    recipes: &[Recipe],
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    tags: &TagsData,
    window: &mut StonecutterWindow,
    input: ItemStack,
) {
    let item_changed =
        window.input.is_empty() || input.is_empty() || window.input.item_id != input.item_id;
    window.input = input;
    if item_changed {
        window.selected_recipe = None;
    }
    refresh_stonecutter_result(recipes, items, item_facts, tags, window);
}

pub(in crate::play) fn select_stonecutter_recipe(
    recipes: &[Recipe],
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    tags: &TagsData,
    window: &mut StonecutterWindow,
    selection: usize,
) -> bool {
    if stonecutter_recipe_for_selection(recipes, items, item_facts, tags, &window.input, selection)
        .is_none()
    {
        window.selected_recipe = None;
        window.result = ItemStack::EMPTY;
        return false;
    }
    window.selected_recipe = Some(selection);
    refresh_stonecutter_result(recipes, items, item_facts, tags, window);
    !window.result.is_empty()
}

pub(in crate::play) fn stonecutter_wire_items(
    window: &StonecutterWindow,
    inventory: &PlayerInventory,
) -> Vec<ItemStack> {
    let mut items = Vec::with_capacity(STONECUTTER_MENU_SLOT_COUNT);
    items.push(window.input.clone());
    items.push(window.result.clone());
    items.extend((9..=35).map(|slot| inventory.slots[slot].clone()));
    items.extend((36..=44).map(|slot| inventory.slots[slot].clone()));
    items
}

pub(in crate::play) fn plan_click(
    input: StonecutterClickInput<'_>,
) -> Option<StonecutterClickPlan> {
    let StonecutterClickInput {
        recipes,
        items,
        item_facts,
        tags,
        mut window,
        mut inventory,
        mut carried_item,
        action,
    } = input;
    let changed = match action {
        StonecutterClickAction::Pickup { slot, button } => apply_pickup_click(
            recipes,
            items,
            item_facts,
            tags,
            &mut window,
            &mut inventory,
            &mut carried_item,
            slot,
            button,
        ),
        StonecutterClickAction::QuickMove { slot } => apply_quick_move_click(
            recipes,
            items,
            item_facts,
            tags,
            &mut window,
            &mut inventory,
            slot,
        ),
        StonecutterClickAction::Unsupported => false,
    };
    changed.then_some(StonecutterClickPlan {
        window,
        inventory,
        carried_item,
    })
}

fn consume_input(
    recipes: &[Recipe],
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    tags: &TagsData,
    window: &mut StonecutterWindow,
    count: i32,
) {
    window.input.count -= count;
    if window.input.count <= 0 {
        window.input = ItemStack::EMPTY;
        window.selected_recipe = None;
    }
    refresh_stonecutter_result(recipes, items, item_facts, tags, window);
}

#[allow(clippy::too_many_arguments)]
fn take_result(
    recipes: &[Recipe],
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    tags: &TagsData,
    window: &mut StonecutterWindow,
    carried_item: &mut ItemStack,
) -> bool {
    let result = window.result.clone();
    if result.is_empty() {
        return false;
    }
    let max_stack = item_max_stack(item_facts, items, &result);
    if carried_item.is_empty() {
        *carried_item = result;
    } else if can_stack(carried_item, &result) && carried_item.count + result.count <= max_stack {
        carried_item.count += result.count;
    } else {
        return false;
    }
    consume_input(recipes, items, item_facts, tags, window, 1);
    true
}

#[allow(clippy::too_many_arguments)]
fn apply_pickup_click(
    recipes: &[Recipe],
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    tags: &TagsData,
    window: &mut StonecutterWindow,
    inventory: &mut PlayerInventory,
    carried_item: &mut ItemStack,
    menu_slot: usize,
    button: i8,
) -> bool {
    if menu_slot >= STONECUTTER_MENU_SLOT_COUNT || !(button == 0 || button == 1) {
        return false;
    }
    if menu_slot == 1 {
        return take_result(recipes, items, item_facts, tags, window, carried_item);
    }
    let slot_stack = if menu_slot == 0 {
        window.input.clone()
    } else {
        let Some(player_slot) = stonecutter_player_slot(menu_slot) else {
            return false;
        };
        inventory.slots[player_slot].clone()
    };
    let stack = if carried_item.is_empty() {
        &slot_stack
    } else {
        &*carried_item
    };
    let max_stack = item_max_stack(item_facts, items, stack);
    let Some(new_slot) = apply_regular_pickup_slot(carried_item, slot_stack, button, max_stack)
    else {
        return false;
    };
    if menu_slot == 0 {
        set_stonecutter_input(recipes, items, item_facts, tags, window, new_slot);
    } else if let Some(player_slot) = stonecutter_player_slot(menu_slot) {
        inventory.slots[player_slot] = new_slot;
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn apply_quick_move_click(
    recipes: &[Recipe],
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    tags: &TagsData,
    window: &mut StonecutterWindow,
    inventory: &mut PlayerInventory,
    menu_slot: usize,
) -> bool {
    if menu_slot >= STONECUTTER_MENU_SLOT_COUNT {
        return false;
    }
    if menu_slot == 1 {
        let result = window.result.clone();
        if result.is_empty() {
            return false;
        }
        let max_stack = item_max_stack(item_facts, items, &result);
        let capacity = inventory.slots[9..=44]
            .iter()
            .map(|slot| {
                if slot.is_empty() {
                    i64::from(max_stack)
                } else if can_stack(slot, &result) {
                    i64::from((max_stack - slot.count).max(0))
                } else {
                    0
                }
            })
            .sum::<i64>();
        let crafts = i64::from(window.input.count.max(0))
            .min(capacity / i64::from(result.count))
            .min(i64::from(i32::MAX / result.count));
        let Ok(crafts) = i32::try_from(crafts) else {
            return false;
        };
        if crafts <= 0 {
            return false;
        }
        let mut output = result;
        output.count *= crafts;
        let mut merged = inventory.clone();
        let (remaining, _) = merged.merge_stack(output, max_stack);
        if !remaining.is_empty() {
            return false;
        }
        *inventory = merged;
        consume_input(recipes, items, item_facts, tags, window, crafts);
        return true;
    }
    if menu_slot == 0 {
        let original = window.input.clone();
        if original.is_empty() {
            return false;
        }
        let remaining = inventory.merge_stack_into_ranges(
            original.clone(),
            &[9..=35, 36..=44],
            item_max_stack(item_facts, items, &original),
        );
        if remaining == original {
            return false;
        }
        set_stonecutter_input(recipes, items, item_facts, tags, window, remaining);
        return true;
    }

    let Some(player_slot) = stonecutter_player_slot(menu_slot) else {
        return false;
    };
    let original = inventory.slots[player_slot].clone();
    if original.is_empty() {
        return false;
    }
    let accepts_input = recipes
        .iter()
        .any(|recipe| recipe_has_advertised_offer(recipe, items, item_facts, tags, &original));
    if accepts_input && (window.input.is_empty() || can_stack(&window.input, &original)) {
        let max_stack = item_max_stack(item_facts, items, &original);
        let capacity = if window.input.is_empty() {
            max_stack
        } else {
            max_stack - window.input.count
        };
        let moved = original.count.min(capacity).max(0);
        if moved == 0 {
            return false;
        }
        let mut moved_stack = original.clone();
        moved_stack.count = moved;
        let mut remaining = original;
        remaining.count -= moved;
        if remaining.count <= 0 {
            remaining = ItemStack::EMPTY;
        }
        let input = if window.input.is_empty() {
            moved_stack
        } else {
            let mut input = window.input.clone();
            input.count += moved;
            input
        };
        inventory.slots[player_slot] = remaining;
        set_stonecutter_input(recipes, items, item_facts, tags, window, input);
        return true;
    }

    let ranges = if (9..=35).contains(&player_slot) {
        [36..=44]
    } else {
        [9..=35]
    };
    inventory.slots[player_slot] = ItemStack::EMPTY;
    let remaining = inventory.merge_stack_into_ranges(
        original.clone(),
        &ranges,
        item_max_stack(item_facts, items, &original),
    );
    inventory.slots[player_slot] = remaining;
    inventory.slots[player_slot] != original
}

fn apply_regular_pickup_slot(
    carried_item: &mut ItemStack,
    slot_stack: ItemStack,
    button: i8,
    max_stack: i32,
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

fn decrement_stack(stack: &mut ItemStack) {
    stack.count -= 1;
    if stack.count <= 0 {
        *stack = ItemStack::EMPTY;
    }
}
