use mc_data::ItemStack;
use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_world::{BlockPos, ChestBlockEntity};

use super::furnace::{furnace_slot_to_stack, stack_to_furnace_slot};
use super::quickcraft::{
    QUICKCRAFT_TYPE_CHARITABLE, QUICKCRAFT_TYPE_GREEDY, QuickCraftClick, QuickCraftState,
    QuickCraftStep, quickcraft_distribution_count,
};
use crate::play::inventory::{PlayerInventory, can_stack, item_max_stack};

pub(in crate::play) const CHEST_MENU_TYPE_ID: i32 = 2;
pub(in crate::play) const DOUBLE_CHEST_MENU_TYPE_ID: i32 = 5;
pub(in crate::play) const SINGLE_CHEST_STORAGE_SLOTS: usize = 27;
pub(in crate::play) const PLAYER_CONTAINER_STORAGE_SLOTS: usize = 36;

#[derive(Debug, Clone)]
pub(in crate::play) struct ChestWindow {
    pub(in crate::play) container_id: i32,
    pub(in crate::play) positions: Vec<BlockPos>,
    pub(in crate::play) state_id: i32,
    pub(in crate::play) quickcraft: QuickCraftState,
}

impl ChestWindow {
    pub(in crate::play) fn new(mut positions: Vec<BlockPos>, container_id: i32) -> Self {
        positions.sort_by_key(|pos| (pos.x, pos.y, pos.z));
        positions.dedup();
        debug_assert!(!positions.is_empty());
        debug_assert!(positions.len() <= 2);
        Self {
            container_id,
            positions,
            state_id: 1,
            quickcraft: QuickCraftState::default(),
        }
    }

    pub(in crate::play) fn position(&self) -> BlockPos {
        self.positions[0]
    }

    pub(in crate::play) fn menu_type(&self) -> i32 {
        if self.positions.len() == 2 {
            DOUBLE_CHEST_MENU_TYPE_ID
        } else {
            CHEST_MENU_TYPE_ID
        }
    }
}

#[derive(Debug, Clone)]
pub(in crate::play) struct ChestView {
    pub(in crate::play) chests: Vec<ChestBlockEntity>,
}

impl ChestView {
    pub(in crate::play) fn storage_slots(&self) -> usize {
        self.chests.len() * SINGLE_CHEST_STORAGE_SLOTS
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::play) enum ChestClickAction {
    Pickup { slot: usize, button: i8 },
    OutsidePickup { button: i8 },
    QuickMove { slot: usize },
    Swap { slot: usize, button: i8 },
    Throw { slot: usize, button: i8 },
    QuickCraft(QuickCraftClick),
    Unsupported,
}

pub(in crate::play) struct ChestClickInput<'a> {
    pub(in crate::play) items: &'a ItemRegistry,
    pub(in crate::play) item_facts: &'a ItemFactsTable,
    pub(in crate::play) window: ChestWindow,
    pub(in crate::play) view: ChestView,
    pub(in crate::play) inventory: PlayerInventory,
    pub(in crate::play) carried_item: ItemStack,
    pub(in crate::play) action: ChestClickAction,
}

pub(in crate::play) struct ChestClickPlan {
    pub(in crate::play) window: ChestWindow,
    pub(in crate::play) view: ChestView,
    pub(in crate::play) inventory: PlayerInventory,
    pub(in crate::play) carried_item: ItemStack,
    pub(in crate::play) dropped: Option<ItemStack>,
    pub(in crate::play) changed: bool,
}

pub(in crate::play) fn plan_click(input: ChestClickInput<'_>) -> ChestClickPlan {
    let ChestClickInput {
        items,
        item_facts,
        mut window,
        mut view,
        mut inventory,
        mut carried_item,
        action,
    } = input;
    let mut dropped = None;

    if !matches!(action, ChestClickAction::QuickCraft(_)) {
        window.quickcraft.reset();
    }
    let changed = match action {
        ChestClickAction::Pickup { slot, button } => apply_pickup_click(
            items,
            item_facts,
            &mut view,
            &mut inventory,
            &mut carried_item,
            slot,
            button,
        ),
        ChestClickAction::OutsidePickup { button } => {
            dropped = apply_outside_pickup_click(&mut carried_item, button);
            dropped.is_some()
        }
        ChestClickAction::QuickMove { slot } => {
            apply_quick_move_click(items, item_facts, &mut view, &mut inventory, slot)
        }
        ChestClickAction::Swap { slot, button } => {
            apply_swap_click(&mut view, &mut inventory, slot, button)
        }
        ChestClickAction::Throw { slot, button } => {
            dropped = apply_throw_click(&mut view, &mut inventory, slot, button);
            dropped.is_some()
        }
        ChestClickAction::QuickCraft(click) => apply_quickcraft_click(
            items,
            item_facts,
            &mut view,
            &mut inventory,
            &mut carried_item,
            &mut window,
            click,
        ),
        ChestClickAction::Unsupported => false,
    };

    ChestClickPlan {
        window,
        view,
        inventory,
        carried_item,
        dropped,
        changed,
    }
}

pub(in crate::play) fn chest_menu_state_change_count(
    before_view: &ChestView,
    after_view: &ChestView,
    before_inventory: &PlayerInventory,
    after_inventory: &PlayerInventory,
    before_carried: &ItemStack,
    after_carried: &ItemStack,
) -> i32 {
    let chest_changes = chest_slot_stacks(before_view)
        .into_iter()
        .zip(chest_slot_stacks(after_view))
        .filter(|(before, after)| before != after)
        .count();
    let inventory_changes = (9..=44)
        .filter(|slot| before_inventory.slots[*slot] != after_inventory.slots[*slot])
        .count();
    let carried_changes = usize::from(before_carried != after_carried);
    i32::try_from(chest_changes + inventory_changes + carried_changes)
        .unwrap_or(i32::MAX)
        .max(1)
}

pub(in crate::play) fn chest_wire_items(
    view: &ChestView,
    inventory: &PlayerInventory,
) -> Vec<ItemStack> {
    let mut items = Vec::with_capacity(view.storage_slots() + PLAYER_CONTAINER_STORAGE_SLOTS);
    for chest in &view.chests {
        items.extend(chest.slots.iter().map(furnace_slot_to_stack));
    }
    items.extend((9..=35).map(|slot| inventory.slots[slot].clone()));
    items.extend((36..=44).map(|slot| inventory.slots[slot].clone()));
    items
}

pub(in crate::play) fn chest_slot_stacks(view: &ChestView) -> Vec<ItemStack> {
    view.chests
        .iter()
        .flat_map(|chest| chest.slots.iter().map(furnace_slot_to_stack))
        .collect()
}

pub(in crate::play) fn adjacent_chest_positions(position: BlockPos) -> [BlockPos; 4] {
    [
        BlockPos {
            x: position.x - 1,
            y: position.y,
            z: position.z,
        },
        BlockPos {
            x: position.x + 1,
            y: position.y,
            z: position.z,
        },
        BlockPos {
            x: position.x,
            y: position.y,
            z: position.z - 1,
        },
        BlockPos {
            x: position.x,
            y: position.y,
            z: position.z + 1,
        },
    ]
}

pub(in crate::play) fn chest_player_slot(storage_slots: usize, menu_slot: usize) -> Option<usize> {
    let main_end = storage_slots + 26;
    let hotbar_start = storage_slots + 27;
    let hotbar_end = storage_slots + 35;
    match menu_slot {
        slot if (storage_slots..=main_end).contains(&slot) => Some(9 + (slot - storage_slots)),
        slot if (hotbar_start..=hotbar_end).contains(&slot) => Some(36 + (slot - hotbar_start)),
        _ => None,
    }
}

fn chest_menu_stack(
    view: &ChestView,
    inventory: &PlayerInventory,
    menu_slot: usize,
) -> Option<ItemStack> {
    let storage_slots = view.storage_slots();
    if menu_slot < storage_slots {
        let chest = menu_slot / SINGLE_CHEST_STORAGE_SLOTS;
        let slot = menu_slot % SINGLE_CHEST_STORAGE_SLOTS;
        return Some(furnace_slot_to_stack(&view.chests[chest].slots[slot]));
    }
    chest_player_slot(storage_slots, menu_slot).map(|slot| inventory.slots[slot].clone())
}

pub(in crate::play) fn set_chest_menu_stack(
    view: &mut ChestView,
    inventory: &mut PlayerInventory,
    menu_slot: usize,
    stack: ItemStack,
) -> bool {
    let storage_slots = view.storage_slots();
    if menu_slot < storage_slots {
        let chest = menu_slot / SINGLE_CHEST_STORAGE_SLOTS;
        let slot = menu_slot % SINGLE_CHEST_STORAGE_SLOTS;
        view.chests[chest].slots[slot] = stack_to_furnace_slot(&stack);
        return true;
    }
    let Some(slot) = chest_player_slot(storage_slots, menu_slot) else {
        return false;
    };
    inventory.slots[slot] = stack;
    true
}

fn valid_chest_menu_slot(view: &ChestView, menu_slot: usize) -> bool {
    menu_slot < view.storage_slots() + PLAYER_CONTAINER_STORAGE_SLOTS
}

fn apply_pickup_click(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    view: &mut ChestView,
    inventory: &mut PlayerInventory,
    carried_item: &mut ItemStack,
    menu_slot: usize,
    button: i8,
) -> bool {
    if !valid_chest_menu_slot(view, menu_slot) || !(button == 0 || button == 1) {
        return false;
    }
    let Some(slot_stack) = chest_menu_stack(view, inventory, menu_slot) else {
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
    let Some(new_slot) = apply_regular_pickup_slot(carried_item, slot_stack, button, max_stack)
    else {
        return false;
    };
    set_chest_menu_stack(view, inventory, menu_slot, new_slot)
}

fn can_quickcraft_replace(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    view: &ChestView,
    inventory: &PlayerInventory,
    menu_slot: usize,
    stack: &ItemStack,
) -> bool {
    if !valid_chest_menu_slot(view, menu_slot) {
        return false;
    }
    let Some(slot_stack) = chest_menu_stack(view, inventory, menu_slot) else {
        return false;
    };
    slot_stack.is_empty()
        || can_stack(&slot_stack, stack)
            && slot_stack.count <= item_max_stack(item_facts, items, stack)
}

#[allow(clippy::too_many_arguments)]
fn apply_quickcraft_click(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    view: &mut ChestView,
    inventory: &mut PlayerInventory,
    carried_item: &mut ItemStack,
    window: &mut ChestWindow,
    click: QuickCraftClick,
) -> bool {
    match window.quickcraft.advance(carried_item.is_empty(), click) {
        QuickCraftStep::Started | QuickCraftStep::Rejected => false,
        QuickCraftStep::Continued { slot } => {
            let Some(menu_slot) = slot else {
                window.quickcraft.reset();
                return false;
            };
            if can_quickcraft_replace(items, item_facts, view, inventory, menu_slot, carried_item)
                && carried_item.count > window.quickcraft.selected_slot_count() as i32
            {
                window.quickcraft.add_slot(menu_slot);
            }
            false
        }
        QuickCraftStep::Finished => {
            let quickcraft = window.quickcraft.finish();
            let quickcraft_kind = quickcraft.kind;
            let quickcraft_slots = quickcraft.slots;
            if quickcraft_slots.is_empty()
                || !matches!(
                    quickcraft_kind,
                    QUICKCRAFT_TYPE_CHARITABLE | QUICKCRAFT_TYPE_GREEDY
                )
            {
                return false;
            }
            if quickcraft_slots.len() == 1 {
                return apply_pickup_click(
                    items,
                    item_facts,
                    view,
                    inventory,
                    carried_item,
                    quickcraft_slots[0],
                    quickcraft_kind,
                );
            }
            let source = carried_item.clone();
            if source.is_empty() || source.count < quickcraft_slots.len() as i32 {
                return false;
            }
            let place_count = quickcraft_distribution_count(
                source.count,
                quickcraft_slots.len(),
                quickcraft_kind,
            );
            if place_count <= 0 {
                return false;
            }
            let max_stack = item_max_stack(item_facts, items, &source);
            let mut remaining = source.count;
            let mut changed = false;
            for menu_slot in quickcraft_slots {
                if !can_quickcraft_replace(items, item_facts, view, inventory, menu_slot, &source) {
                    continue;
                }
                let Some(slot_stack) = chest_menu_stack(view, inventory, menu_slot) else {
                    continue;
                };
                let carry = if slot_stack.is_empty() {
                    0
                } else {
                    slot_stack.count
                };
                let new_count = (place_count + carry).min(max_stack);
                let moved = new_count - carry;
                if moved <= 0 {
                    continue;
                }
                let mut new_stack = source.clone();
                new_stack.count = new_count;
                if set_chest_menu_stack(view, inventory, menu_slot, new_stack) {
                    remaining -= moved;
                    changed = true;
                }
            }
            let mut cursor = source;
            cursor.count = remaining;
            *carried_item = if cursor.count <= 0 {
                ItemStack::EMPTY
            } else {
                cursor
            };
            changed
        }
    }
}

fn merge_stack_into_chest(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    view: &mut ChestView,
    mut stack: ItemStack,
) -> ItemStack {
    let max_stack = item_max_stack(item_facts, items, &stack);
    for chest in &mut view.chests {
        for slot in &mut chest.slots {
            if can_stack(&furnace_slot_to_stack(slot), &stack) && slot.count < max_stack {
                let moved = (max_stack - slot.count).min(stack.count);
                slot.count += moved;
                stack.count -= moved;
                if stack.count <= 0 {
                    return ItemStack::EMPTY;
                }
            }
        }
    }
    for chest in &mut view.chests {
        for slot in &mut chest.slots {
            if !slot.is_empty() {
                continue;
            }
            let moved = stack.count.min(max_stack);
            let mut moved_stack = stack.clone();
            moved_stack.count = moved;
            *slot = stack_to_furnace_slot(&moved_stack);
            stack.count -= moved;
            if stack.count <= 0 {
                return ItemStack::EMPTY;
            }
        }
    }
    stack
}

pub(in crate::play) fn apply_quick_move_click(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    view: &mut ChestView,
    inventory: &mut PlayerInventory,
    menu_slot: usize,
) -> bool {
    let storage_slots = view.storage_slots();
    if !valid_chest_menu_slot(view, menu_slot) {
        return false;
    }
    if menu_slot < storage_slots {
        let chest_idx = menu_slot / SINGLE_CHEST_STORAGE_SLOTS;
        let local_slot = menu_slot % SINGLE_CHEST_STORAGE_SLOTS;
        let original = furnace_slot_to_stack(&view.chests[chest_idx].slots[local_slot]);
        if original.is_empty() {
            return false;
        }
        let max_stack = item_max_stack(item_facts, items, &original);
        let remaining =
            inventory.merge_stack_into_ranges_reversed(original.clone(), &[9..=44], max_stack);
        view.chests[chest_idx].slots[local_slot] = stack_to_furnace_slot(&remaining);
        remaining != original
    } else {
        let Some(player_slot) = chest_player_slot(storage_slots, menu_slot) else {
            return false;
        };
        let original = inventory.slots[player_slot].clone();
        if original.is_empty() {
            return false;
        }
        inventory.slots[player_slot] = ItemStack::EMPTY;
        let remaining = merge_stack_into_chest(items, item_facts, view, original.clone());
        inventory.slots[player_slot] = remaining;
        inventory.slots[player_slot] != original
    }
}

pub(in crate::play) fn apply_swap_click(
    view: &mut ChestView,
    inventory: &mut PlayerInventory,
    menu_slot: usize,
    button: i8,
) -> bool {
    if !valid_chest_menu_slot(view, menu_slot) {
        return false;
    }
    let Some(player_slot) = hotbar_swap_slot(button) else {
        return false;
    };
    if chest_player_slot(view.storage_slots(), menu_slot) == Some(player_slot) {
        return false;
    }
    let Some(clicked) = chest_menu_stack(view, inventory, menu_slot) else {
        return false;
    };
    let swap = inventory.slots[player_slot].clone();
    if !set_chest_menu_stack(view, inventory, menu_slot, swap) {
        return false;
    }
    inventory.slots[player_slot] = clicked;
    true
}

pub(in crate::play) fn apply_throw_click(
    view: &mut ChestView,
    inventory: &mut PlayerInventory,
    menu_slot: usize,
    button: i8,
) -> Option<ItemStack> {
    if !valid_chest_menu_slot(view, menu_slot) {
        return None;
    }
    let mut stack = chest_menu_stack(view, inventory, menu_slot)?;
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
    set_chest_menu_stack(view, inventory, menu_slot, stack).then_some(dropped)
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

fn hotbar_swap_slot(button: i8) -> Option<usize> {
    (0..=8)
        .contains(&button)
        .then_some(PlayerInventory::HOTBAR_BASE + button as usize)
}

fn decrement_stack(stack: &mut ItemStack) {
    stack.count -= 1;
    if stack.count <= 0 {
        *stack = ItemStack::EMPTY;
    }
}
