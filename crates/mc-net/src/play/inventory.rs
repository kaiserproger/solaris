use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_protocol::codec::Identifier;
use mc_protocol::packets::play::ItemStack;

use super::InteractionState;
use super::survival::max_tool_damage_for_path;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ArmorStats {
    pub(crate) armor: f32,
    pub(crate) toughness: f32,
}

/// 46-slot player inventory (window 0).
///
/// Layout (vanilla wire numbering):
///   0       crafting result
///   1..=4   crafting 2x2 input
///   5..=8   armor (head, chest, legs, feet)
///   9..=35  main inventory rows
///   36..=44 hotbar
///   45      offhand
#[derive(Debug, Clone)]
pub(crate) struct PlayerInventory {
    pub(crate) slots: [ItemStack; 46],
}

impl PlayerInventory {
    /// Slot index where the hotbar begins on the wire.
    pub(crate) const HOTBAR_BASE: usize = 36;
    pub(crate) const OFFHAND_SLOT: usize = 45;

    pub(crate) fn empty() -> Self {
        Self {
            slots: std::array::from_fn(|_| ItemStack::EMPTY),
        }
    }

    pub(crate) fn held(&self, hotbar_slot: u8) -> &ItemStack {
        &self.slots[Self::HOTBAR_BASE + hotbar_slot as usize]
    }

    pub(crate) fn held_mut(&mut self, hotbar_slot: u8) -> &mut ItemStack {
        &mut self.slots[Self::HOTBAR_BASE + hotbar_slot as usize]
    }

    pub(crate) fn set_hotbar(&mut self, hotbar_slot: u8, stack: ItemStack) {
        self.slots[Self::HOTBAR_BASE + hotbar_slot as usize] = stack;
    }

    pub(crate) fn merge_stack(
        &mut self,
        mut stack: ItemStack,
        max_stack: i32,
    ) -> (ItemStack, Vec<(usize, ItemStack)>) {
        let mut changed = Vec::new();
        if stack.is_empty() {
            return (ItemStack::EMPTY, changed);
        }

        for slot in 9..=44 {
            let current = &mut self.slots[slot];
            if current.is_empty()
                || current.item_id != stack.item_id
                || current.damage != stack.damage
                || current.count >= max_stack
            {
                continue;
            }
            let moved = (max_stack - current.count).min(stack.count);
            current.count += moved;
            stack.count -= moved;
            changed.push((slot, current.clone()));
            if stack.count <= 0 {
                return (ItemStack::EMPTY, changed);
            }
        }

        for slot in 9..=44 {
            if !self.slots[slot].is_empty() {
                continue;
            }
            let moved = stack.count.min(max_stack);
            let mut moved_stack = stack.clone();
            moved_stack.count = moved;
            self.slots[slot] = moved_stack;
            stack.count -= moved;
            changed.push((slot, self.slots[slot].clone()));
            if stack.count <= 0 {
                return (ItemStack::EMPTY, changed);
            }
        }

        (stack, changed)
    }

    pub(crate) fn merge_pickup_stack(
        &mut self,
        mut stack: ItemStack,
        max_stack: i32,
    ) -> (ItemStack, Vec<(usize, ItemStack)>) {
        let mut changed = Vec::new();
        if stack.is_empty() {
            return (ItemStack::EMPTY, changed);
        }

        for slot in 9..=44 {
            let current = &mut self.slots[slot];
            if current.is_empty()
                || current.item_id != stack.item_id
                || current.damage != stack.damage
                || current.count >= max_stack
            {
                continue;
            }
            let moved = (max_stack - current.count).min(stack.count);
            current.count += moved;
            stack.count -= moved;
            changed.push((slot, current.clone()));
            if stack.count <= 0 {
                return (ItemStack::EMPTY, changed);
            }
        }

        for slot in 36..=44 {
            if !self.slots[slot].is_empty() {
                continue;
            }
            let moved = stack.count.min(max_stack);
            let mut moved_stack = stack.clone();
            moved_stack.count = moved;
            self.slots[slot] = moved_stack;
            stack.count -= moved;
            changed.push((slot, self.slots[slot].clone()));
            if stack.count <= 0 {
                return (ItemStack::EMPTY, changed);
            }
        }

        for slot in 9..=35 {
            if !self.slots[slot].is_empty() {
                continue;
            }
            let moved = stack.count.min(max_stack);
            let mut moved_stack = stack.clone();
            moved_stack.count = moved;
            self.slots[slot] = moved_stack;
            stack.count -= moved;
            changed.push((slot, self.slots[slot].clone()));
            if stack.count <= 0 {
                return (ItemStack::EMPTY, changed);
            }
        }

        (stack, changed)
    }

    pub(crate) fn as_wire_list(&self) -> Vec<ItemStack> {
        self.slots.to_vec()
    }

    pub(crate) fn merge_stack_into_ranges(
        &mut self,
        mut stack: ItemStack,
        ranges: &[std::ops::RangeInclusive<usize>],
        max_stack: i32,
    ) -> ItemStack {
        if stack.is_empty() {
            return ItemStack::EMPTY;
        }

        for range in ranges {
            for slot in range.clone() {
                let current = &mut self.slots[slot];
                if !can_stack(current, &stack) || current.count >= max_stack {
                    continue;
                }
                let moved = (max_stack - current.count).min(stack.count);
                current.count += moved;
                stack.count -= moved;
                if stack.count <= 0 {
                    return ItemStack::EMPTY;
                }
            }
        }

        for range in ranges {
            for slot in range.clone() {
                if !self.slots[slot].is_empty() {
                    continue;
                }
                let moved = stack.count.min(max_stack);
                let mut moved_stack = stack.clone();
                moved_stack.count = moved;
                self.slots[slot] = moved_stack;
                stack.count -= moved;
                if stack.count <= 0 {
                    return ItemStack::EMPTY;
                }
            }
        }

        stack
    }
}

pub(crate) fn can_stack(left: &ItemStack, right: &ItemStack) -> bool {
    !left.is_empty()
        && !right.is_empty()
        && left.item_id == right.item_id
        && left.damage == right.damage
}

pub(crate) fn item_max_stack(
    item_facts: &ItemFactsTable,
    items: &ItemRegistry,
    stack: &ItemStack,
) -> i32 {
    if stack.is_empty() || stack.damage.is_some() {
        return 1;
    }
    let Some(name) = items.name_of(stack.item_id) else {
        return 64;
    };
    if let Some(max_stack) = item_facts
        .get(name)
        .and_then(|facts| facts.max_stack_size)
        .and_then(|value| i32::try_from(value).ok())
    {
        return max_stack.max(1);
    }
    let path = name.path();
    if max_tool_damage_for_path(path).is_some()
        || matches!(
            path,
            "shield"
                | "bow"
                | "crossbow"
                | "trident"
                | "fishing_rod"
                | "shears"
                | "flint_and_steel"
                | "water_bucket"
                | "lava_bucket"
        )
        || path.ends_with("_helmet")
        || path.ends_with("_chestplate")
        || path.ends_with("_leggings")
        || path.ends_with("_boots")
    {
        1
    } else if path == "bucket" {
        16
    } else {
        64
    }
}

pub(crate) fn take_from_slot(slot: &mut ItemStack, count: i32) -> ItemStack {
    if slot.is_empty() || count <= 0 {
        return ItemStack::EMPTY;
    }
    let moved = slot.count.min(count);
    let mut out = slot.clone();
    out.count = moved;
    slot.count -= moved;
    if slot.count <= 0 {
        *slot = ItemStack::EMPTY;
    }
    out
}

pub(crate) fn decrement_cursor(cursor: &mut ItemStack) {
    cursor.count -= 1;
    if cursor.count <= 0 {
        *cursor = ItemStack::EMPTY;
    }
}

pub(crate) fn hotbar_swap_slot(button: i8) -> Option<usize> {
    (0..=8)
        .contains(&button)
        .then_some(PlayerInventory::HOTBAR_BASE + button as usize)
}

pub(crate) fn player_swap_slot(button: i8) -> Option<usize> {
    hotbar_swap_slot(button).or_else(|| (button == 40).then_some(PlayerInventory::OFFHAND_SLOT))
}

pub(crate) fn take_throw_stack(slot: &mut ItemStack, button: i8) -> Option<ItemStack> {
    match button {
        0 => (!slot.is_empty()).then(|| take_from_slot(slot, 1)),
        1 => (!slot.is_empty()).then(|| std::mem::take(slot)),
        _ => None,
    }
}

pub(crate) fn pickup_click_max_stack(
    item_facts: &ItemFactsTable,
    items: &ItemRegistry,
    carried_item: &ItemStack,
    slot_stack: &ItemStack,
) -> i32 {
    let stack = if carried_item.is_empty() {
        slot_stack
    } else {
        carried_item
    };
    item_max_stack(item_facts, items, stack)
}

pub(crate) fn apply_regular_pickup_slot(
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
        decrement_cursor(carried_item);
        return Some(one);
    }
    if can_stack(&slot_stack, &cursor) && slot_stack.count < max_stack {
        let mut new_slot = slot_stack;
        new_slot.count += 1;
        decrement_cursor(carried_item);
        return Some(new_slot);
    }
    None
}

pub(crate) fn apply_regular_swap_slot(
    clicked: ItemStack,
    swap: ItemStack,
    can_place_swap: bool,
    can_place_clicked: bool,
) -> Option<(ItemStack, ItemStack)> {
    (can_place_swap && can_place_clicked).then_some((swap, clicked))
}

pub(crate) fn apply_regular_throw_slot(
    slot_stack: ItemStack,
    button: i8,
) -> Option<(ItemStack, ItemStack)> {
    let mut stack = slot_stack;
    let dropped = take_throw_stack(&mut stack, button)?;
    Some((stack, dropped))
}

pub(crate) fn apply_outside_pickup_click(
    carried_item: &mut ItemStack,
    button: i8,
) -> Option<ItemStack> {
    if carried_item.is_empty() {
        return None;
    }
    match button {
        0 => Some(std::mem::take(carried_item)),
        1 => {
            let mut dropped = carried_item.clone();
            dropped.count = 1;
            decrement_cursor(carried_item);
            Some(dropped)
        }
        _ => None,
    }
}

pub(crate) fn can_place_in_player_slot(
    items: &ItemRegistry,
    slot: usize,
    stack: &ItemStack,
) -> bool {
    if stack.is_empty() {
        return true;
    }
    match slot {
        5..=8 => armor_entry_for_item(items, stack.item_id)
            .is_some_and(|entry| armor_slot_for_kind(entry.slot) == slot),
        _ => true,
    }
}

pub(crate) fn armor_slot_for_kind(kind: mc_data::armor::ArmorSlot) -> usize {
    match kind {
        mc_data::armor::ArmorSlot::Head => 5,
        mc_data::armor::ArmorSlot::Chest => 6,
        mc_data::armor::ArmorSlot::Legs => 7,
        mc_data::armor::ArmorSlot::Feet => 8,
    }
}

pub(crate) fn armor_entry_for_item(
    items: &ItemRegistry,
    item_id: u32,
) -> Option<&'static mc_data::armor::ArmorEntry> {
    items
        .name_of(item_id)
        .and_then(|name| mc_data::armor::builtin().entry(name))
}

fn equipped_armor_stats(items: &ItemRegistry, inventory: &PlayerInventory) -> ArmorStats {
    let mut total = ArmorStats {
        armor: 0.0,
        toughness: 0.0,
    };
    for slot in 5..=8 {
        let stack = &inventory.slots[slot];
        if stack.is_empty() {
            continue;
        }
        if let Some(entry) = armor_entry_for_item(items, stack.item_id) {
            total.armor += entry.armor;
            total.toughness += entry.toughness;
        }
    }
    total
}

fn equipped_protection_points(items: &ItemRegistry, inventory: &PlayerInventory) -> i32 {
    let protection = Identifier::parse("minecraft:protection").expect("static identifier");
    (5..=8)
        .filter_map(|slot| {
            let stack = &inventory.slots[slot];
            (!stack.is_empty() && armor_entry_for_item(items, stack.item_id).is_some())
                .then_some(stack)
        })
        .flat_map(|stack| &stack.enchantments)
        .filter(|enchantment| enchantment.id == protection)
        .map(|enchantment| enchantment.level.max(0))
        .sum()
}

pub(crate) fn armor_reduced_damage(amount: f32, stats: ArmorStats) -> f32 {
    let damage = amount.max(0.0);
    let toughness = 2.0 + stats.toughness / 4.0;
    let real_armor = (stats.armor - damage / toughness)
        .clamp(stats.armor * 0.2, 20.0)
        .max(0.0);
    damage * (1.0 - real_armor / 25.0)
}

pub(crate) fn protection_reduced_damage(amount: f32, protection_points: i32) -> f32 {
    let damage = amount.max(0.0);
    let points = protection_points.clamp(0, 20) as f32;
    damage * (1.0 - points / 25.0)
}

pub(crate) fn survival_damage_after_armor(state: Option<&InteractionState>, amount: f32) -> f32 {
    let Some(state) = state else {
        return amount.max(0.0);
    };
    armor_reduced_damage(amount, equipped_armor_stats(&state.items, &state.inventory))
}

pub(crate) fn inventory_damage_after_armor(
    items: &ItemRegistry,
    inventory: &PlayerInventory,
    amount: f32,
) -> f32 {
    armor_reduced_damage(amount, equipped_armor_stats(items, inventory))
}

pub(crate) fn survival_damage_after_protection(
    state: Option<&InteractionState>,
    amount: f32,
) -> f32 {
    let Some(state) = state else {
        return amount.max(0.0);
    };
    protection_reduced_damage(
        amount,
        equipped_protection_points(&state.items, &state.inventory),
    )
}

pub(crate) fn inventory_damage_after_protection(
    items: &ItemRegistry,
    inventory: &PlayerInventory,
    amount: f32,
) -> f32 {
    protection_reduced_damage(amount, equipped_protection_points(items, inventory))
}

fn armor_durability_damage(incoming_damage: f32) -> i32 {
    if incoming_damage <= 0.0 {
        return 0;
    }
    if !incoming_damage.is_finite() {
        return i32::MAX;
    }
    let scaled = (incoming_damage / 4.0).floor().max(1.0);
    if scaled >= i32::MAX as f32 {
        i32::MAX
    } else {
        scaled as i32
    }
}

pub(crate) fn damage_equipped_armor(
    state: &mut InteractionState,
    incoming_damage: f32,
) -> Vec<(usize, ItemStack)> {
    let damage = armor_durability_damage(incoming_damage);
    if damage <= 0 {
        return Vec::new();
    }
    let mut changed = Vec::new();
    for slot in 5..=8 {
        let stack = &mut state.inventory.slots[slot];
        if stack.is_empty() {
            continue;
        }
        let Some(entry) = armor_entry_for_item(&state.items, stack.item_id) else {
            continue;
        };
        let next_damage = stack.damage.unwrap_or(0).saturating_add(damage);
        if next_damage >= entry.max_damage {
            *stack = ItemStack::EMPTY;
        } else {
            stack.damage = Some(next_damage);
        }
        changed.push((slot, stack.clone()));
    }
    changed
}

pub(crate) fn damage_inventory_armor(
    items: &ItemRegistry,
    inventory: &mut PlayerInventory,
    incoming_damage: f32,
) -> Vec<(usize, ItemStack)> {
    let damage = armor_durability_damage(incoming_damage);
    if damage <= 0 {
        return Vec::new();
    }
    let mut changed = Vec::new();
    for slot in 5..=8 {
        let stack = &mut inventory.slots[slot];
        if stack.is_empty() {
            continue;
        }
        let Some(entry) = armor_entry_for_item(items, stack.item_id) else {
            continue;
        };
        let next_damage = stack.damage.unwrap_or(0).saturating_add(damage);
        if next_damage >= entry.max_damage {
            *stack = ItemStack::EMPTY;
        } else {
            stack.damage = Some(next_damage);
        }
        changed.push((slot, stack.clone()));
    }
    changed
}
