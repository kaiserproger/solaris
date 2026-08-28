use mc_data::ItemStack;
use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_protocol::codec::Identifier;

use super::InteractionState;

static EMPTY_STACK: ItemStack = ItemStack::EMPTY;

pub(crate) type ArmorStats = mc_data::armor::ArmorStats;

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
    const HOTBAR_LEN: usize = 9;
    pub(crate) const FEET_ARMOR_SLOT: usize = 8;
    pub(crate) const OFFHAND_SLOT: usize = 45;

    pub(crate) fn empty() -> Self {
        Self {
            slots: std::array::from_fn(|_| ItemStack::EMPTY),
        }
    }

    pub(crate) fn held(&self, hotbar_slot: u8) -> Option<&ItemStack> {
        let slot = Self::hotbar_index(hotbar_slot)?;
        let stack = &self.slots[slot];
        if stack.is_empty() {
            Some(&EMPTY_STACK)
        } else {
            Some(stack)
        }
    }

    pub(crate) fn held_mut(&mut self, hotbar_slot: u8) -> Option<&mut ItemStack> {
        let slot = Self::hotbar_index(hotbar_slot)?;
        let stack = &mut self.slots[slot];
        canonicalize_empty(stack);
        Some(stack)
    }

    pub(crate) fn set_hotbar(
        &mut self,
        hotbar_slot: u8,
        stack: ItemStack,
    ) -> Result<(), ItemStack> {
        let Some(slot) = Self::hotbar_index(hotbar_slot) else {
            return Err(stack);
        };
        self.slots[slot] = canonical_stack(stack);
        Ok(())
    }

    fn hotbar_index(hotbar_slot: u8) -> Option<usize> {
        (usize::from(hotbar_slot) < Self::HOTBAR_LEN)
            .then_some(Self::HOTBAR_BASE + usize::from(hotbar_slot))
    }

    pub(crate) fn merge_stack(
        &mut self,
        mut stack: ItemStack,
        max_stack: i32,
    ) -> (ItemStack, Vec<(usize, ItemStack)>) {
        let mut changed = Vec::new();
        canonicalize_empty(&mut stack);
        if stack.is_empty() {
            return (ItemStack::EMPTY, changed);
        }
        if max_stack <= 0 {
            return (stack, changed);
        }

        for slot in 9..=44 {
            let current = &mut self.slots[slot];
            canonicalize_empty(current);
            if !can_stack(current, &stack) || current.count >= max_stack {
                continue;
            }
            let Some(capacity) = max_stack.checked_sub(current.count) else {
                continue;
            };
            let moved = capacity.min(stack.count);
            let Some(next_count) = current.count.checked_add(moved) else {
                continue;
            };
            let Some(remaining) = stack.count.checked_sub(moved) else {
                continue;
            };
            current.count = next_count;
            stack.count = remaining;
            changed.push((slot, current.clone()));
            if stack.count <= 0 {
                return (ItemStack::EMPTY, changed);
            }
        }

        for slot in 9..=44 {
            canonicalize_empty(&mut self.slots[slot]);
            if !self.slots[slot].is_empty() {
                continue;
            }
            let moved = stack.count.min(max_stack);
            let mut moved_stack = stack.clone();
            moved_stack.count = moved;
            self.slots[slot] = moved_stack;
            stack.count = stack.count.checked_sub(moved).unwrap_or_default();
            changed.push((slot, self.slots[slot].clone()));
            if stack.count <= 0 {
                return (ItemStack::EMPTY, changed);
            }
        }

        (stack, changed)
    }

    pub(crate) fn merge_pickup_stack(
        &mut self,
        stack: ItemStack,
        max_stack: i32,
        selected_hotbar_slot: u8,
    ) -> Option<(ItemStack, Vec<(usize, ItemStack)>)> {
        let selected_slot = Self::hotbar_index(selected_hotbar_slot)?;
        Some(self.merge_pickup_stack_partial(stack, max_stack, selected_slot))
    }

    fn merge_pickup_stack_partial(
        &mut self,
        mut stack: ItemStack,
        max_stack: i32,
        selected_slot: usize,
    ) -> (ItemStack, Vec<(usize, ItemStack)>) {
        let mut changed = Vec::new();
        canonicalize_empty(&mut stack);
        if stack.is_empty() {
            return (ItemStack::EMPTY, changed);
        }
        if max_stack <= 0 {
            return (stack, changed);
        }

        let mut merge_order = Vec::with_capacity(37);
        merge_order.push(selected_slot);
        merge_order.push(Self::OFFHAND_SLOT);
        merge_order.extend(
            (Self::HOTBAR_BASE..Self::HOTBAR_BASE + Self::HOTBAR_LEN)
                .filter(|slot| *slot != selected_slot),
        );
        merge_order.extend(9..=35);
        for slot in merge_order {
            let current = &mut self.slots[slot];
            canonicalize_empty(current);
            if !can_stack(current, &stack) || current.count >= max_stack {
                continue;
            }
            let Some(capacity) = max_stack.checked_sub(current.count) else {
                continue;
            };
            let moved = capacity.min(stack.count);
            let Some(next_count) = current.count.checked_add(moved) else {
                continue;
            };
            let Some(remaining) = stack.count.checked_sub(moved) else {
                continue;
            };
            current.count = next_count;
            stack.count = remaining;
            changed.push((slot, current.clone()));
            if stack.count <= 0 {
                return (ItemStack::EMPTY, changed);
            }
        }

        let empty_order = std::iter::once(selected_slot)
            .chain(
                (Self::HOTBAR_BASE..Self::HOTBAR_BASE + Self::HOTBAR_LEN)
                    .filter(|slot| *slot != selected_slot),
            )
            .chain(9..=35);
        for slot in empty_order {
            canonicalize_empty(&mut self.slots[slot]);
            if !self.slots[slot].is_empty() {
                continue;
            }
            let moved = stack.count.min(max_stack);
            let mut moved_stack = stack.clone();
            moved_stack.count = moved;
            self.slots[slot] = moved_stack;
            stack.count = stack.count.checked_sub(moved).unwrap_or_default();
            changed.push((slot, self.slots[slot].clone()));
            if stack.count <= 0 {
                return (ItemStack::EMPTY, changed);
            }
        }

        (stack, changed)
    }

    pub(crate) fn as_wire_list(&self) -> Vec<ItemStack> {
        self.slots.iter().cloned().map(canonical_stack).collect()
    }

    pub(crate) fn merge_stack_into_ranges(
        &mut self,
        stack: ItemStack,
        ranges: &[std::ops::RangeInclusive<usize>],
        max_stack: i32,
    ) -> ItemStack {
        self.merge_stack_into_ranges_ordered(stack, ranges, max_stack, false)
    }

    pub(crate) fn merge_stack_into_ranges_reversed(
        &mut self,
        stack: ItemStack,
        ranges: &[std::ops::RangeInclusive<usize>],
        max_stack: i32,
    ) -> ItemStack {
        self.merge_stack_into_ranges_ordered(stack, ranges, max_stack, true)
    }

    fn merge_stack_into_ranges_ordered(
        &mut self,
        mut stack: ItemStack,
        ranges: &[std::ops::RangeInclusive<usize>],
        max_stack: i32,
        reverse: bool,
    ) -> ItemStack {
        canonicalize_empty(&mut stack);
        if stack.is_empty() {
            return ItemStack::EMPTY;
        }
        if max_stack <= 0
            || ranges
                .iter()
                .any(|range| !range.is_empty() && *range.end() >= self.slots.len())
        {
            return stack;
        }

        let mut slots = ranges
            .iter()
            .flat_map(|range| range.clone())
            .collect::<Vec<_>>();
        if reverse {
            slots.reverse();
        }

        for &slot in &slots {
            let current = &mut self.slots[slot];
            canonicalize_empty(current);
            if !can_stack(current, &stack) || current.count >= max_stack {
                continue;
            }
            let Some(capacity) = max_stack.checked_sub(current.count) else {
                continue;
            };
            let moved = capacity.min(stack.count);
            let Some(next_count) = current.count.checked_add(moved) else {
                continue;
            };
            let Some(remaining) = stack.count.checked_sub(moved) else {
                continue;
            };
            current.count = next_count;
            stack.count = remaining;
            if stack.count <= 0 {
                return ItemStack::EMPTY;
            }
        }

        for slot in slots {
            canonicalize_empty(&mut self.slots[slot]);
            if !self.slots[slot].is_empty() {
                continue;
            }
            let moved = stack.count.min(max_stack);
            let mut moved_stack = stack.clone();
            moved_stack.count = moved;
            self.slots[slot] = moved_stack;
            stack.count = stack.count.checked_sub(moved).unwrap_or_default();
            if stack.count <= 0 {
                return ItemStack::EMPTY;
            }
        }

        stack
    }
}

fn canonicalize_empty(stack: &mut ItemStack) {
    mc_data::inventory_semantics_26_1_2::canonicalize_empty(stack);
}

fn canonical_stack(stack: ItemStack) -> ItemStack {
    mc_data::inventory_semantics_26_1_2::canonical_stack(stack)
}

pub(crate) fn can_stack(left: &ItemStack, right: &ItemStack) -> bool {
    mc_data::inventory_semantics_26_1_2::can_stack(left, right)
}

pub(crate) fn item_max_stack(
    item_facts: &ItemFactsTable,
    items: &ItemRegistry,
    stack: &ItemStack,
) -> i32 {
    mc_data::item_semantics_26_1_2::max_stack_for_stack(item_facts, items, stack)
}

#[cfg(test)]
pub(crate) fn take_from_slot(slot: &mut ItemStack, count: i32) -> ItemStack {
    mc_data::inventory_semantics_26_1_2::take_from_stack(slot, count)
}

#[cfg(test)]
pub(crate) fn decrement_cursor(cursor: &mut ItemStack) {
    mc_data::inventory_semantics_26_1_2::decrement_stack(cursor);
}

pub(crate) fn hotbar_swap_slot(button: i8) -> Option<usize> {
    (0..=8)
        .contains(&button)
        .then_some(PlayerInventory::HOTBAR_BASE + button as usize)
}

pub(crate) fn player_swap_slot(button: i8) -> Option<usize> {
    hotbar_swap_slot(button).or_else(|| (button == 40).then_some(PlayerInventory::OFFHAND_SLOT))
}

#[cfg(test)]
pub(crate) fn take_throw_stack(slot: &mut ItemStack, button: i8) -> Option<ItemStack> {
    mc_data::inventory_semantics_26_1_2::take_throw_stack(slot, button)
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
    mc_data::inventory_semantics_26_1_2::apply_regular_pickup_slot(
        carried_item,
        slot_stack,
        button,
        max_stack,
        can_place_carried,
    )
}

pub(crate) fn apply_regular_swap_slot(
    clicked: ItemStack,
    swap: ItemStack,
    can_place_swap: bool,
    can_place_clicked: bool,
) -> Option<(ItemStack, ItemStack)> {
    mc_data::inventory_semantics_26_1_2::apply_regular_swap_slot(
        clicked,
        swap,
        can_place_swap,
        can_place_clicked,
    )
}

pub(crate) fn apply_regular_throw_slot(
    slot_stack: ItemStack,
    button: i8,
) -> Option<(ItemStack, ItemStack)> {
    mc_data::inventory_semantics_26_1_2::apply_regular_throw_slot(slot_stack, button)
}

pub(crate) fn apply_outside_pickup_click(
    carried_item: &mut ItemStack,
    button: i8,
) -> Option<ItemStack> {
    mc_data::inventory_semantics_26_1_2::apply_outside_pickup_click(carried_item, button)
}

pub(crate) fn can_place_in_player_slot(
    item_facts: &ItemFactsTable,
    items: &ItemRegistry,
    slot: usize,
    stack: &ItemStack,
) -> bool {
    if stack.is_empty() {
        return true;
    }
    match slot {
        5..=8 => equippable_slot_for_item(item_facts, items, stack.item_id) == Some(slot),
        _ => true,
    }
}

pub(crate) fn equippable_slot_for_item(
    item_facts: &ItemFactsTable,
    items: &ItemRegistry,
    item_id: u32,
) -> Option<usize> {
    mc_data::item_semantics_26_1_2::equippable_player_slot(item_facts, items, item_id)
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
    mc_data::armor::armor_reduced_damage(amount, stats)
}

pub(crate) fn protection_reduced_damage(amount: f32, protection_points: i32) -> f32 {
    mc_data::armor::protection_reduced_damage(amount, protection_points)
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
