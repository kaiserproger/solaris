use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_entity::Vec3;
use mc_protocol::packets::play::{
    GameMode, InteractionHand, ItemStack, LIVING_ENTITY_FLAG_OFF_HAND,
    LIVING_ENTITY_FLAG_USING_ITEM,
};

const PLAYER_BASE_ATTACK_SPEED: f32 = 4.0;
const ATTACK_RECHARGE_TICKS_PER_SECOND: f32 = 20.0;
pub(in crate::play) const SHIELD_ACTIVATION_DELAY_TICKS: u64 = 5;
pub(in crate::play) const SHIELD_FRONT_ARC_DOT_MIN: f64 = 0.0;
pub(in crate::play) const SHIELD_FALLBACK_MAX_DAMAGE: i32 = 336;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::play) struct ShieldUseState {
    pub(in crate::play) hand: InteractionHand,
    pub(in crate::play) started_tick: u64,
    pub(in crate::play) slot: usize,
    pub(in crate::play) stack: ItemStack,
}

pub(in crate::play) fn held_attack_speed(
    item_facts: &ItemFactsTable,
    items: &ItemRegistry,
    held: &ItemStack,
) -> f32 {
    let modifier = held_item_id(held)
        .and_then(|item_id| items.name_of(item_id))
        .and_then(|item| item_facts.get(item))
        .and_then(|facts| facts.attack_speed_modifier)
        .unwrap_or(0.0);
    (PLAYER_BASE_ATTACK_SPEED + modifier).max(0.0)
}

pub(in crate::play) fn held_attack_damage(
    item_facts: &ItemFactsTable,
    items: &ItemRegistry,
    held: &ItemStack,
) -> f32 {
    let base = attack_damage_for_item(
        item_facts,
        items,
        (!held.is_empty()).then_some(held.item_id),
    );
    base + sharpness_damage_bonus(held)
}

pub(in crate::play) fn held_attack_damage_at_tick(
    item_facts: &ItemFactsTable,
    items: &ItemRegistry,
    held: &ItemStack,
    last_tick: Option<u64>,
    current_tick: u64,
) -> f32 {
    let full_damage = held_attack_damage(item_facts, items, held);
    let Some(last_tick) = last_tick else {
        return full_damage;
    };
    let elapsed_ticks = current_tick.saturating_sub(last_tick) as f32;
    let strength = ((elapsed_ticks + 0.5) * held_attack_speed(item_facts, items, held)
        / ATTACK_RECHARGE_TICKS_PER_SECOND)
        .clamp(0.0, 1.0);
    let base_damage = attack_damage_for_item(item_facts, items, held_item_id(held));
    let enchantment_damage = full_damage - base_damage;
    base_damage * (0.2 + strength * strength * 0.8) + enchantment_damage * strength
}

pub(in crate::play) fn begin_player_attack_attempt(
    item_facts: &ItemFactsTable,
    items: &ItemRegistry,
    held: &ItemStack,
    game_mode: GameMode,
    last_tick: Option<u64>,
    current_tick: u64,
) -> Option<f32> {
    if game_mode == GameMode::Spectator {
        return None;
    }
    Some(held_attack_damage_at_tick(
        item_facts,
        items,
        held,
        last_tick,
        current_tick,
    ))
}

pub(in crate::play) fn attack_damage_for_item(
    item_facts: &ItemFactsTable,
    items: &ItemRegistry,
    item_id: Option<u32>,
) -> f32 {
    let Some(item) = item_id.and_then(|item_id| items.name_of(item_id)) else {
        return 1.0;
    };
    if let Some(modifier) = item_facts
        .get(item)
        .and_then(|facts| facts.attack_damage_modifier)
    {
        return (1.0 + modifier).max(0.0);
    }
    let path = item.path();
    if path.ends_with("_sword") {
        match path.strip_suffix("_sword").unwrap_or_default() {
            "wooden" | "golden" => 4.0,
            "stone" => 5.0,
            "iron" => 6.0,
            "diamond" => 7.0,
            "netherite" => 8.0,
            _ => 2.0,
        }
    } else if path.ends_with("_axe") {
        7.0
    } else if path.ends_with("_pickaxe") || path.ends_with("_shovel") {
        4.0
    } else {
        1.0
    }
}

fn sharpness_damage_bonus(stack: &ItemStack) -> f32 {
    let level = stack
        .enchantments
        .iter()
        .find(|enchantment| enchantment.id.as_str() == "minecraft:sharpness")
        .map_or(0, |enchantment| enchantment.level);
    if level <= 0 {
        return 0.0;
    }
    1.0 + (level - 1) as f32 * 0.5
}

pub(in crate::play) fn damage_held_weapon_stack(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    held: &mut ItemStack,
) -> Option<ItemStack> {
    let (max_damage, damage_per_attack) = {
        if held.is_empty() {
            return None;
        }
        let item = items.name_of(held.item_id)?;
        let facts = item_facts.get(item);
        let is_weapon = facts.is_some_and(|facts| facts.weapon);
        let is_tool = is_durability_tool_path(item.path());
        if !is_weapon && !is_tool {
            return None;
        }
        let max_damage = facts
            .and_then(|facts| facts.max_damage)
            .and_then(|value| i32::try_from(value).ok())
            .or_else(|| max_tool_damage_for_path(item.path()))?;
        let damage_per_attack = facts
            .and_then(|facts| facts.weapon_damage_per_attack)
            .unwrap_or(1)
            .max(1);
        (max_damage, damage_per_attack)
    };

    let damage_per_attack = i32::try_from(damage_per_attack).unwrap_or(i32::MAX);
    let new_damage = held.damage.unwrap_or(0).saturating_add(damage_per_attack);
    if new_damage >= max_damage {
        *held = ItemStack::EMPTY;
    } else {
        held.damage = Some(new_damage);
    }
    Some(held.clone())
}

pub(in crate::play) fn weapon_attacks_damage_held_item(game_mode: GameMode) -> bool {
    game_mode == GameMode::Survival
}

pub(in crate::play) fn is_durability_tool_path(path: &str) -> bool {
    path.ends_with("_axe")
        || path.ends_with("_hoe")
        || path.ends_with("_pickaxe")
        || path.ends_with("_shovel")
        || path.ends_with("_sword")
}

pub(in crate::play) fn max_tool_damage_for_path(path: &str) -> Option<i32> {
    if !is_durability_tool_path(path) {
        return None;
    }
    let max = if path.starts_with("wooden_") {
        59
    } else if path.starts_with("stone_") {
        131
    } else if path.starts_with("iron_") {
        250
    } else if path.starts_with("diamond_") {
        1561
    } else if path.starts_with("golden_") {
        32
    } else if path.starts_with("netherite_") {
        2031
    } else {
        return None;
    };
    Some(max)
}

pub(in crate::play) fn player_horizontal_look_direction(yaw: f32) -> Vec3 {
    let yaw = f64::from(yaw).to_radians();
    Vec3::new(-yaw.sin(), 0.0, yaw.cos())
}

pub(in crate::play) fn shield_hand_slot(
    hand: InteractionHand,
    main_hand_slot: usize,
    off_hand_slot: usize,
) -> usize {
    match hand {
        InteractionHand::MainHand => main_hand_slot,
        InteractionHand::OffHand => off_hand_slot,
    }
}

pub(in crate::play) fn stack_is_shield(items: &ItemRegistry, stack: &ItemStack) -> bool {
    !stack.is_empty()
        && items
            .name_of(stack.item_id)
            .is_some_and(|item| item.as_str() == "minecraft:shield")
}

pub(in crate::play) fn shield_use_flags(shield_use: Option<&ShieldUseState>) -> i8 {
    let Some(shield_use) = shield_use else {
        return 0;
    };
    let mut flags = LIVING_ENTITY_FLAG_USING_ITEM;
    if shield_use.hand == InteractionHand::OffHand {
        flags |= LIVING_ENTITY_FLAG_OFF_HAND;
    }
    flags
}

pub(in crate::play) fn shield_use_from_stack(
    hand: InteractionHand,
    slot: usize,
    stack: ItemStack,
    current_tick: u64,
    is_shield: bool,
) -> Option<ShieldUseState> {
    is_shield.then_some(ShieldUseState {
        hand,
        started_tick: current_tick,
        slot,
        stack,
    })
}

pub(in crate::play) fn shield_use_matches(
    shield_use: &ShieldUseState,
    current_hand_slot: usize,
    slots: &[ItemStack],
    items: &ItemRegistry,
) -> bool {
    shield_use_matches_slot(
        shield_use.slot,
        current_hand_slot,
        &shield_use.stack,
        slots,
        items,
    )
}

pub(in crate::play) fn shield_use_matches_slot(
    shield_slot: usize,
    current_hand_slot: usize,
    expected_stack: &ItemStack,
    slots: &[ItemStack],
    items: &ItemRegistry,
) -> bool {
    current_hand_slot == shield_slot
        && slots.get(shield_slot) == Some(expected_stack)
        && stack_is_shield(items, expected_stack)
}

pub(in crate::play) fn damage_active_shield_slots(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    slots: &mut [ItemStack],
    shield_use: &mut ShieldUseState,
    blocked_damage: f32,
) -> Option<(usize, ItemStack, bool)> {
    let result = damage_active_shield_slot(
        items,
        item_facts,
        slots,
        shield_use.slot,
        &shield_use.stack,
        blocked_damage,
    )?;
    if !result.2 {
        shield_use.stack = result.1.clone();
    }
    Some(result)
}

pub(in crate::play) fn damage_active_shield_slot(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    slots: &mut [ItemStack],
    shield_slot: usize,
    expected_stack: &ItemStack,
    blocked_damage: f32,
) -> Option<(usize, ItemStack, bool)> {
    if !shield_use_matches_slot(shield_slot, shield_slot, expected_stack, slots, items) {
        return None;
    }
    let max_damage = items
        .name_of(slots.get(shield_slot)?.item_id)
        .and_then(|item| item_facts.get(item))
        .and_then(|facts| facts.max_damage)
        .and_then(|value| i32::try_from(value).ok())
        .unwrap_or(SHIELD_FALLBACK_MAX_DAMAGE)
        .max(1);
    let durability_damage = shield_durability_damage(blocked_damage);
    if durability_damage <= 0 {
        return None;
    }

    let stack = &mut slots[shield_slot];
    let next_damage = stack.damage.unwrap_or(0).saturating_add(durability_damage);
    let broken = next_damage >= max_damage;
    if broken {
        *stack = ItemStack::EMPTY;
    } else {
        stack.damage = Some(next_damage);
    }
    Some((shield_slot, stack.clone(), broken))
}

pub(in crate::play) fn shield_durability_damage(blocked_damage: f32) -> i32 {
    if blocked_damage < 3.0 {
        return 0;
    }
    if !blocked_damage.is_finite() {
        return i32::MAX;
    }
    let scaled = blocked_damage.max(0.0).floor();
    if scaled >= (i32::MAX - 1) as f32 {
        i32::MAX
    } else {
        (scaled as i32).saturating_add(1).max(1)
    }
}

pub(in crate::play) fn shield_disable_ticks(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    attacker_stack: &ItemStack,
    shield_stack: &ItemStack,
) -> Option<u64> {
    let attacker = items.name_of(attacker_stack.item_id)?;
    let shield = items.name_of(shield_stack.item_id)?;
    let seconds = item_facts
        .get(attacker)
        .and_then(|facts| facts.weapon_disable_blocking_seconds)?;
    let scale = item_facts
        .get(shield)
        .and_then(|facts| facts.blocks_attacks_disable_cooldown_scale)?;
    if !seconds.is_finite() || !scale.is_finite() || seconds <= 0.0 || scale <= 0.0 {
        return None;
    }
    let ticks = (seconds * scale * 20.0).round();
    (ticks > 0.0 && ticks <= u64::MAX as f32).then_some(ticks as u64)
}

pub(in crate::play) fn shield_blocks_damage(
    player_position: Vec3,
    player_yaw: f32,
    source_origin: Option<Vec3>,
    current_tick: u64,
    shield_use: Option<&ShieldUseState>,
) -> bool {
    let Some(shield_use) = shield_use else {
        return false;
    };
    shield_blocks_damage_since(
        player_position,
        player_yaw,
        source_origin,
        current_tick,
        shield_use.started_tick,
    )
}

pub(in crate::play) fn shield_blocks_damage_since(
    player_position: Vec3,
    player_yaw: f32,
    source_origin: Option<Vec3>,
    current_tick: u64,
    started_tick: u64,
) -> bool {
    if current_tick.saturating_sub(started_tick) < SHIELD_ACTIVATION_DELAY_TICKS {
        return false;
    }
    let Some(source_origin) = source_origin else {
        return false;
    };
    let incoming = Vec3::new(
        source_origin.x - player_position.x,
        0.0,
        source_origin.z - player_position.z,
    );
    let incoming_len = (incoming.x * incoming.x + incoming.z * incoming.z).sqrt();
    if incoming_len <= f64::EPSILON {
        return false;
    }
    let look = player_horizontal_look_direction(player_yaw);
    let dot = (look.x * incoming.x + look.z * incoming.z) / incoming_len;
    dot >= SHIELD_FRONT_ARC_DOT_MIN
}

fn held_item_id(held: &ItemStack) -> Option<u32> {
    (!held.is_empty()).then_some(held.item_id)
}

#[cfg(test)]
mod tests {
    use mc_data::Identifier;
    use mc_data::item_components::{ItemFacts, ItemFactsTable};
    use mc_data::items::{ItemRegistry, ItemReport};
    use mc_protocol::packets::play::{GameMode, ItemStack};

    use super::{begin_player_attack_attempt, shield_disable_ticks, shield_durability_damage};

    #[test]
    fn spectator_attack_attempt_is_rejected_without_mutating_state() {
        assert_eq!(
            begin_player_attack_attempt(
                &mc_data::item_components::ItemFactsTable::default(),
                &mc_data::items::solaris_required_items(),
                &ItemStack::EMPTY,
                GameMode::Spectator,
                Some(100),
                106,
            ),
            None,
        );
    }

    #[test]
    fn shield_disable_duration_uses_exact_weapon_and_blocking_components() {
        let axe = Identifier::parse("minecraft:iron_axe").unwrap();
        let sword = Identifier::parse("minecraft:iron_sword").unwrap();
        let shield = Identifier::parse("minecraft:shield").unwrap();
        let items = ItemRegistry::from_report(&[
            ItemReport {
                id: axe.clone(),
                protocol_id: 1,
            },
            ItemReport {
                id: sword.clone(),
                protocol_id: 2,
            },
            ItemReport {
                id: shield.clone(),
                protocol_id: 3,
            },
        ]);
        let facts = ItemFactsTable::from_entries([
            (
                axe,
                ItemFacts {
                    weapon_disable_blocking_seconds: Some(5.0),
                    ..ItemFacts::default()
                },
            ),
            (sword, ItemFacts::default()),
            (
                shield,
                ItemFacts {
                    blocks_attacks_disable_cooldown_scale: Some(1.0),
                    ..ItemFacts::default()
                },
            ),
        ]);

        assert_eq!(
            shield_disable_ticks(&items, &facts, &ItemStack::new(1, 1), &ItemStack::new(3, 1),),
            Some(100),
        );
        assert_eq!(
            shield_disable_ticks(&items, &facts, &ItemStack::new(2, 1), &ItemStack::new(3, 1),),
            None,
        );
    }

    #[test]
    fn non_finite_shield_damage_saturates_without_overflow() {
        assert_eq!(shield_durability_damage(f32::INFINITY), i32::MAX);
        assert_eq!(shield_durability_damage(f32::NAN), i32::MAX);
    }
}
