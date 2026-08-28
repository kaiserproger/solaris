//! Protocol-neutral ItemStack transaction semantics for Java Edition 26.1.2.

use crate::{ItemEnchantment, ItemStack};

#[must_use]
pub fn canonical_stack(mut stack: ItemStack) -> ItemStack {
    canonicalize_empty(&mut stack);
    stack
}

pub fn canonicalize_empty(stack: &mut ItemStack) {
    if stack.is_empty() {
        *stack = ItemStack::EMPTY;
    }
}

#[must_use]
pub fn can_stack(left: &ItemStack, right: &ItemStack) -> bool {
    !left.is_empty()
        && !right.is_empty()
        && left.item_id == right.item_id
        && left.damage == right.damage
        && left.custom_name == right.custom_name
        && left.item_model == right.item_model
        && enchantments_equal_in_canonical_order(&left.enchantments, &right.enchantments)
}

fn enchantments_equal_in_canonical_order(
    left: &[ItemEnchantment],
    right: &[ItemEnchantment],
) -> bool {
    if left == right {
        return true;
    }
    if left.len() != right.len() {
        return false;
    }
    let mut left = left.iter().collect::<Vec<_>>();
    let mut right = right.iter().collect::<Vec<_>>();
    left.sort_unstable();
    right.sort_unstable();
    left == right
}

pub fn take_from_stack(slot: &mut ItemStack, count: i32) -> ItemStack {
    canonicalize_empty(slot);
    if slot.is_empty() || count <= 0 {
        return ItemStack::EMPTY;
    }
    let moved = slot.count.min(count);
    let mut out = slot.clone();
    out.count = moved;
    slot.count = slot.count.checked_sub(moved).unwrap_or_default();
    canonicalize_empty(slot);
    out
}

pub fn decrement_stack(stack: &mut ItemStack) {
    canonicalize_empty(stack);
    if stack.count <= 1 {
        *stack = ItemStack::EMPTY;
        return;
    }
    stack.count = stack.count.checked_sub(1).unwrap_or_default();
}

pub fn take_throw_stack(slot: &mut ItemStack, button: i8) -> Option<ItemStack> {
    canonicalize_empty(slot);
    match button {
        0 => (!slot.is_empty()).then(|| take_from_stack(slot, 1)),
        1 => (!slot.is_empty()).then(|| std::mem::take(slot)),
        _ => None,
    }
}

pub fn apply_regular_pickup_slot(
    carried_item: &mut ItemStack,
    mut slot_stack: ItemStack,
    button: i8,
    max_stack: i32,
    can_place_carried: bool,
) -> Option<ItemStack> {
    canonicalize_empty(carried_item);
    canonicalize_empty(&mut slot_stack);
    if !(button == 0 || button == 1) || max_stack <= 0 {
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
            let moved = max_stack.checked_sub(slot_stack.count)?.min(cursor.count);
            if moved <= 0 {
                return None;
            }
            let mut new_slot = slot_stack;
            new_slot.count = new_slot.count.checked_add(moved)?;
            carried_item.count = carried_item.count.checked_sub(moved)?;
            canonicalize_empty(carried_item);
            return Some(new_slot);
        }
        *carried_item = slot_stack;
        return Some(cursor);
    }

    if cursor.is_empty() {
        if slot_stack.is_empty() {
            return None;
        }
        let moved = slot_stack.count / 2 + (slot_stack.count & 1);
        let mut new_cursor = slot_stack.clone();
        new_cursor.count = moved;
        let mut remaining = slot_stack;
        remaining.count = remaining.count.checked_sub(moved)?;
        canonicalize_empty(&mut remaining);
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
        new_slot.count = new_slot.count.checked_add(1)?;
        decrement_stack(carried_item);
        return Some(new_slot);
    }
    None
}

#[must_use]
pub fn apply_regular_swap_slot(
    clicked: ItemStack,
    swap: ItemStack,
    can_place_swap: bool,
    can_place_clicked: bool,
) -> Option<(ItemStack, ItemStack)> {
    (can_place_swap && can_place_clicked).then_some((swap, clicked))
}

pub fn apply_regular_throw_slot(
    slot_stack: ItemStack,
    button: i8,
) -> Option<(ItemStack, ItemStack)> {
    let mut stack = slot_stack;
    let dropped = take_throw_stack(&mut stack, button)?;
    Some((stack, dropped))
}

pub fn apply_outside_pickup_click(carried_item: &mut ItemStack, button: i8) -> Option<ItemStack> {
    canonicalize_empty(carried_item);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_transactions_preserve_identity_and_canonical_empty() {
        let mut cursor = ItemStack::new(7, 3);
        let slot = ItemStack::new(7, 63);
        let next = apply_regular_pickup_slot(&mut cursor, slot, 0, 64, true).unwrap();
        assert_eq!(next.count, 64);
        assert_eq!(cursor.count, 2);

        let dropped = apply_outside_pickup_click(&mut cursor, 1).unwrap();
        assert_eq!(dropped.count, 1);
        assert_eq!(cursor.count, 1);
        decrement_stack(&mut cursor);
        assert_eq!(cursor, ItemStack::EMPTY);
    }

    #[test]
    fn stack_compatibility_ignores_enchantment_storage_order() {
        let sharpness = crate::Identifier::parse("minecraft:sharpness").unwrap();
        let unbreaking = crate::Identifier::parse("minecraft:unbreaking").unwrap();
        let mut left = ItemStack::new(7, 1)
            .with_enchantment(sharpness.clone(), 3)
            .with_enchantment(unbreaking.clone(), 2);
        let mut right = ItemStack::new(7, 1)
            .with_enchantment(sharpness, 3)
            .with_enchantment(unbreaking, 2);
        right.enchantments.reverse();
        assert!(can_stack(&left, &right));
        left.damage = Some(1);
        assert!(!can_stack(&left, &right));
    }
}
