use super::*;

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

pub(crate) fn armor_reduced_damage(amount: f32, stats: ArmorStats) -> f32 {
    let damage = amount.max(0.0);
    let toughness = 2.0 + stats.toughness / 4.0;
    let real_armor = (stats.armor - damage / toughness)
        .clamp(stats.armor * 0.2, 20.0)
        .max(0.0);
    damage * (1.0 - real_armor / 25.0)
}

pub(crate) fn survival_damage_after_armor(state: Option<&InteractionState>, amount: f32) -> f32 {
    let Some(state) = state else {
        return amount.max(0.0);
    };
    armor_reduced_damage(amount, equipped_armor_stats(&state.items, &state.inventory))
}

pub(crate) fn damage_equipped_armor(state: &mut InteractionState) -> Vec<(usize, ItemStack)> {
    let mut changed = Vec::new();
    for slot in 5..=8 {
        let stack = &mut state.inventory.slots[slot];
        if stack.is_empty() {
            continue;
        }
        let Some(entry) = armor_entry_for_item(&state.items, stack.item_id) else {
            continue;
        };
        let next_damage = stack.damage.unwrap_or(0) + 1;
        if next_damage >= entry.max_damage {
            *stack = ItemStack::EMPTY;
        } else {
            stack.damage = Some(next_damage);
        }
        changed.push((slot, stack.clone()));
    }
    changed
}
