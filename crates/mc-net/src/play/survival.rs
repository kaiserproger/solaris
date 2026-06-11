use super::plants::plant_drop_stacks;
use super::*;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct SurvivalState {
    pub(super) health: f32,
    pub(super) food: i32,
    pub(super) saturation: f32,
    pub(super) exhaustion: f32,
}

impl SurvivalState {
    pub(super) const MAX_HEALTH: f32 = 20.0;
    pub(super) const MAX_FOOD: i32 = 20;
    pub(super) const BLOCK_BREAK_EXHAUSTION: f32 = 0.005;
    pub(super) const ENTITY_ATTACK_EXHAUSTION: f32 = 0.1;
    const EXHAUSTION_STEP: f32 = 4.0;
    const HEALTH_TICK_PERIOD: u32 = 4;

    pub(super) const FULL: Self = Self {
        health: Self::MAX_HEALTH,
        food: Self::MAX_FOOD,
        saturation: 5.0,
        exhaustion: 0.0,
    };

    pub(super) const fn as_packet(self) -> ClientboundSetHealth {
        ClientboundSetHealth {
            health: self.health,
            food: self.food,
            saturation: self.saturation,
        }
    }

    pub(super) fn apply_damage(&mut self, amount: f32) {
        self.health = (self.health - amount.max(0.0)).clamp(0.0, Self::MAX_HEALTH);
    }

    pub(super) fn heal(&mut self, amount: f32) {
        self.health = (self.health + amount.max(0.0)).clamp(0.0, Self::MAX_HEALTH);
    }

    pub(super) fn add_food(&mut self, food: i32, saturation: f32) {
        self.food = (self.food + food).clamp(0, Self::MAX_FOOD);
        self.saturation = (self.saturation + saturation.max(0.0)).clamp(0.0, self.food as f32);
    }

    pub(super) fn is_dead(self) -> bool {
        self.health <= 0.0
    }

    pub(super) fn add_exhaustion(&mut self, amount: f32) -> bool {
        let before_food = self.food;
        let before_saturation = self.saturation;
        self.exhaustion = (self.exhaustion + amount.max(0.0)).max(0.0);
        while self.exhaustion >= Self::EXHAUSTION_STEP {
            self.exhaustion -= Self::EXHAUSTION_STEP;
            if self.saturation > 0.0 {
                self.saturation = (self.saturation - 1.0).max(0.0);
            } else if self.food > 0 {
                self.food -= 1;
            }
        }
        self.food != before_food || self.saturation != before_saturation
    }

    pub(super) fn tick_health(&mut self, tick: u32) -> bool {
        if self.is_dead() || !tick.is_multiple_of(Self::HEALTH_TICK_PERIOD) {
            return false;
        }
        let before = *self;
        if self.food >= 18 && self.health < Self::MAX_HEALTH {
            self.heal(1.0);
            self.add_exhaustion(6.0);
        } else if self.food == 0 {
            self.apply_damage(1.0);
        }
        *self != before
    }
}

#[derive(Debug, Clone)]
pub(super) struct PendingBreak {
    pub(super) position: i64,
    pub(super) direction: Direction,
    pub(super) started_at: Instant,
    pub(super) required_time: Duration,
    pub(super) held_hotbar_slot: u8,
    pub(super) held_item_id: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum UseKind {
    Food(mc_data::food::FoodEntry),
    #[allow(dead_code)]
    Bow,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PendingUse {
    pub(super) started_at: Instant,
    pub(super) required_time: Duration,
    pub(super) held_hotbar_slot: u8,
    pub(super) held_item_id: u32,
    pub(super) kind: UseKind,
}

#[derive(Debug, Clone, Copy)]
struct MiningRule {
    block_path_contains: &'static [&'static str],
    base_time: Duration,
    tool_suffix: Option<&'static str>,
}

const FALLBACK_MINING_RULES: &[MiningRule] = &[
    MiningRule {
        block_path_contains: &["stone", "ore", "deepslate", "brick"],
        base_time: Duration::from_millis(1_500),
        tool_suffix: Some("_pickaxe"),
    },
    MiningRule {
        block_path_contains: &["log", "wood", "planks"],
        base_time: Duration::from_millis(900),
        tool_suffix: Some("_axe"),
    },
    MiningRule {
        block_path_contains: &["dirt", "sand", "gravel", "clay", "snow"],
        base_time: Duration::from_millis(600),
        tool_suffix: Some("_shovel"),
    },
    MiningRule {
        block_path_contains: &["leaves", "grass", "flower", "podzol"],
        base_time: SURVIVAL_MINING_FALLBACK_TIME,
        tool_suffix: None,
    },
];

const UNKNOWN_BLOCK_MINING_RULE: MiningRule = MiningRule {
    block_path_contains: &[],
    base_time: Duration::from_millis(800),
    tool_suffix: None,
};

pub(super) fn held_item_id(state: &InteractionState) -> Option<u32> {
    let held = state.inventory.held(state.selected_hotbar_slot);
    (!held.is_empty()).then_some(held.item_id)
}

pub(super) fn pending_break_matches(
    state: &InteractionState,
    pending: &PendingBreak,
    action: &ServerboundPlayerAction,
) -> bool {
    pending.position == action.position
        && pending.direction == action.direction
        && pending.held_hotbar_slot == state.selected_hotbar_slot
        && pending.held_item_id == held_item_id(state)
}

pub(super) fn pending_break_is_complete(pending: &PendingBreak, now: Instant) -> bool {
    now.duration_since(pending.started_at) >= pending.required_time
}

fn fallback_mining_rule(block_path: &str) -> MiningRule {
    FALLBACK_MINING_RULES
        .iter()
        .copied()
        .find(|rule| {
            rule.block_path_contains
                .iter()
                .any(|needle| block_path.contains(needle))
        })
        .unwrap_or(UNKNOWN_BLOCK_MINING_RULE)
}

fn tool_speed_divisor(tool_path: Option<&str>, required_suffix: Option<&str>) -> u64 {
    let Some(tool_path) = tool_path else {
        return 1;
    };
    if required_suffix.is_some_and(|suffix| !tool_path.ends_with(suffix)) {
        return 1;
    }

    if tool_path.starts_with("golden_") {
        10
    } else if tool_path.starts_with("netherite_") {
        8
    } else if tool_path.starts_with("diamond_") {
        6
    } else if tool_path.starts_with("iron_") {
        4
    } else if tool_path.starts_with("stone_") {
        3
    } else if tool_path.starts_with("wooden_") {
        2
    } else {
        1
    }
}

pub(super) fn fallback_mining_time(block_path: &str, tool_path: Option<&str>) -> Duration {
    let rule = fallback_mining_rule(block_path);
    let divisor = tool_speed_divisor(tool_path, rule.tool_suffix);
    Duration::from_millis((rule.base_time.as_millis() as u64 / divisor).max(100))
}

fn mining_time_for_block(
    blocks: &BlockRegistry,
    items: &ItemRegistry,
    block_state: BlockStateId,
    held_item_id: Option<u32>,
) -> Duration {
    let Some(block_state) = blocks.by_id(block_state) else {
        return UNKNOWN_BLOCK_MINING_RULE.base_time;
    };
    let tool_path = held_item_id
        .and_then(|id| items.name_of(id))
        .map(mc_data::Identifier::path);
    fallback_mining_time(block_state.block.id.path(), tool_path)
}

pub(super) fn block_break_is_denied(blocks: &BlockRegistry, block_state: BlockStateId) -> bool {
    blocks.by_id(block_state).is_some_and(|state| {
        matches!(
            state.block.id.path(),
            "bedrock" | "barrier" | "end_portal_frame"
        )
    })
}

pub(super) async fn mining_time_for_target(state: &InteractionState, position: i64) -> Duration {
    let (x, y, z) = unpack_block_pos(position);
    let block_state = {
        let mut storage = state.world.lock().await;
        storage
            .get_block(mc_world::BlockPos { x, y, z })
            .map_err(|err| {
                warn!(error = %err, x, y, z, "mining target read failed; using fallback timing");
            })
            .ok()
            .flatten()
    };

    block_state.map_or(UNKNOWN_BLOCK_MINING_RULE.base_time, |block_state| {
        mining_time_for_block(
            &state.blocks,
            &state.items,
            block_state,
            held_item_id(state),
        )
    })
}

pub(super) fn item_entity_type_id(entity_types: &EntityTypeRegistry) -> Option<i32> {
    let item = mc_data::Identifier::parse("minecraft:item").expect("static identifier");
    entity_types
        .id_of(&item)
        .and_then(|id| i32::try_from(id).ok())
}

pub(super) fn xp_orb_entity_type_id(entity_types: &EntityTypeRegistry) -> Option<i32> {
    let xp = mc_data::Identifier::parse("minecraft:experience_orb").expect("static identifier");
    entity_types
        .id_of(&xp)
        .and_then(|id| i32::try_from(id).ok())
        .or_else(|| {
            let legacy = mc_data::Identifier::parse("minecraft:xp_orb").expect("static identifier");
            entity_types
                .id_of(&legacy)
                .and_then(|id| i32::try_from(id).ok())
        })
}

pub(super) fn falling_block_entity_type_id(entity_types: &EntityTypeRegistry) -> Option<i32> {
    let falling_block =
        mc_data::Identifier::parse("minecraft:falling_block").expect("static identifier");
    entity_types
        .id_of(&falling_block)
        .and_then(|id| i32::try_from(id).ok())
}

pub(super) fn arrow_entity_type_id(entity_types: &EntityTypeRegistry) -> Option<i32> {
    let arrow = mc_data::Identifier::parse("minecraft:arrow").expect("static identifier");
    entity_types
        .id_of(&arrow)
        .and_then(|id| i32::try_from(id).ok())
}

pub(super) fn is_hostile_entity(entity_type: &str) -> bool {
    mc_data::Identifier::parse(entity_type.to_string())
        .map(|id| mc_data::entity_types::fallback_entity_type_facts(id, 0))
        .is_ok_and(|facts| facts.category.is_hostile())
}

pub(super) fn mob_drop_stack_from(
    loot: &mc_data::loot::LootTables,
    items: &ItemRegistry,
    entity_type: &str,
) -> Option<ItemStack> {
    let entity = Identifier::parse(entity_type.to_string()).ok()?;
    let drop = loot
        .entity_drop_stack(&entity)
        .or_else(|| mc_data::loot::builtin().entity_drop_stack(&entity))?;
    let item_id = items.id_of(&drop.item)?;
    let count = i32::try_from(drop.count).ok()?;
    Some(ItemStack::new(item_id, count))
}

pub(super) fn mob_drop_stack(state: &InteractionState, entity_type: &str) -> Option<ItemStack> {
    mob_drop_stack_from(&state.loot, &state.items, entity_type)
}

pub(super) fn mob_xp_value(entity_type: &str) -> i32 {
    match entity_type {
        "minecraft:zombie" | "minecraft:skeleton" | "minecraft:spider" => 5,
        "minecraft:cow" | "minecraft:pig" | "minecraft:sheep" | "minecraft:chicken" => 1,
        _ => 0,
    }
}

pub(super) fn block_drop_stacks_from(
    loot: &mc_data::loot::LootTables,
    items: &ItemRegistry,
    blocks: &mc_world::BlockRegistry,
    block_state: BlockStateId,
) -> Vec<ItemStack> {
    let Some(block) = blocks.by_id(block_state) else {
        return Vec::new();
    };
    if let Some(crop_drops) = plant_drop_stacks(items, block) {
        return crop_drops;
    }

    let drop = loot
        .block_drop_stack(&block.block.id)
        .or_else(|| mc_data::loot::builtin().block_drop_stack(&block.block.id));
    let item = drop.map_or(&block.block.id, |drop| &drop.item);
    let count = drop
        .and_then(|drop| i32::try_from(drop.count).ok())
        .unwrap_or(1);
    items
        .id_of(item)
        .map(|item_id| vec![ItemStack::new(item_id, count)])
        .unwrap_or_default()
}

pub(super) fn block_drop_stacks(
    state: &InteractionState,
    block_state: BlockStateId,
) -> Vec<ItemStack> {
    block_drop_stacks_from(&state.loot, &state.items, &state.blocks, block_state)
}

pub(super) fn food_rule_for_item(
    item_facts: &ItemFactsTable,
    item: &mc_data::Identifier,
) -> Option<(mc_data::food::FoodEntry, Duration)> {
    if let Some(facts) = item_facts.get(item)
        && let Some(food) = facts.food
    {
        let duration = facts
            .use_duration_ticks
            .map(|ticks| Duration::from_millis(u64::from(ticks) * 50))
            .unwrap_or(DEFAULT_FOOD_USE_DURATION);
        return Some((food, duration));
    }

    mc_data::food::builtin()
        .entry(item)
        .copied()
        .map(|food| (food, DEFAULT_FOOD_USE_DURATION))
}

pub(super) fn held_food_use(
    state: &InteractionState,
) -> Option<(u32, mc_data::food::FoodEntry, Duration)> {
    let held = state.inventory.held(state.selected_hotbar_slot);
    if held.is_empty() {
        return None;
    }
    let (rule, duration) = state
        .items
        .name_of(held.item_id)
        .and_then(|item| food_rule_for_item(&state.item_facts, item))?;
    Some((held.item_id, rule, duration))
}

#[allow(dead_code)]
pub(super) fn is_bow_item(state: &InteractionState) -> bool {
    let held = state.inventory.held(state.selected_hotbar_slot);
    if held.is_empty() {
        return false;
    }
    state
        .items
        .name_of(held.item_id)
        .is_some_and(|item| item.as_str() == "minecraft:bow")
}

#[allow(dead_code)]
pub(super) fn consume_arrow(state: &mut InteractionState) -> Option<usize> {
    // Offhand first (slot 45)
    if !state.inventory.slots[45].is_empty()
        && state
            .items
            .name_of(state.inventory.slots[45].item_id)
            .is_some_and(|item| item.as_str() == "minecraft:arrow")
    {
        let slot = &mut state.inventory.slots[45];
        slot.count = slot.count.saturating_sub(1);
        if slot.count <= 0 {
            *slot = ItemStack::EMPTY;
        }
        return Some(45);
    }
    // Hotbar slots 0-8 (inventory slots 36-44)
    for hotbar_slot in 0..9u8 {
        let inv_slot = PlayerInventory::HOTBAR_BASE + hotbar_slot as usize;
        if !state.inventory.slots[inv_slot].is_empty()
            && state
                .items
                .name_of(state.inventory.slots[inv_slot].item_id)
                .is_some_and(|item| item.as_str() == "minecraft:arrow")
        {
            let slot = &mut state.inventory.slots[inv_slot];
            slot.count = slot.count.saturating_sub(1);
            if slot.count <= 0 {
                *slot = ItemStack::EMPTY;
            }
            return Some(inv_slot);
        }
    }
    None
}

#[allow(dead_code)]
pub(super) fn bow_draw_power(started_at: Instant) -> f64 {
    let elapsed = Instant::now().duration_since(started_at);
    let ticks = (elapsed.as_millis() as f64 / 50.0).min(20.0);
    let power = ticks / 20.0;
    if power < 0.15 {
        0.0
    } else {
        power.clamp(0.0, 1.0)
    }
}

pub(super) fn pending_use_matches(state: &InteractionState, pending: &PendingUse) -> bool {
    pending.held_hotbar_slot == state.selected_hotbar_slot
        && state.inventory.held(pending.held_hotbar_slot).item_id == pending.held_item_id
}

pub(super) fn pending_use_is_complete(pending: &PendingUse, now: Instant) -> bool {
    now.duration_since(pending.started_at) >= pending.required_time
}

pub(super) fn entity_item_stack(stack: ItemStack) -> EntityItemStack {
    EntityItemStack {
        item_id: stack.item_id,
        count: stack.count,
        damage: stack.damage,
    }
}

pub(super) fn held_attack_damage(state: &InteractionState) -> f32 {
    let held = state.inventory.held(state.selected_hotbar_slot);
    attack_damage_for_item(
        &state.item_facts,
        &state.items,
        (!held.is_empty()).then_some(held.item_id),
    )
}

pub(super) fn attack_damage_for_item(
    item_facts: &ItemFactsTable,
    items: &ItemRegistry,
    item_id: Option<u32>,
) -> f32 {
    let Some(item) = item_id.and_then(|item_id| items.name_of(item_id)) else {
        return 2.0;
    };
    if let Some(modifier) = item_facts
        .get(item)
        .and_then(|facts| facts.attack_damage_modifier)
    {
        return (1.0 + modifier).max(0.0);
    }
    let path = item.path();
    if path.ends_with("_sword") {
        8.0
    } else if path.ends_with("_axe") {
        7.0
    } else if path.ends_with("_pickaxe") || path.ends_with("_shovel") {
        4.0
    } else {
        2.0
    }
}

pub(super) fn is_durability_tool_path(path: &str) -> bool {
    path.ends_with("_axe")
        || path.ends_with("_hoe")
        || path.ends_with("_pickaxe")
        || path.ends_with("_shovel")
        || path.ends_with("_sword")
}

pub(super) fn max_tool_damage_for_path(path: &str) -> Option<i32> {
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

fn damage_held_tool_stack(state: &mut InteractionState) -> Option<(usize, ItemStack)> {
    let hotbar_slot = state.selected_hotbar_slot;
    let wire_slot = PlayerInventory::HOTBAR_BASE + hotbar_slot as usize;
    let item_path = {
        let held = state.inventory.held(hotbar_slot);
        if held.is_empty() {
            return None;
        }
        state.items.name_of(held.item_id)?.path().to_owned()
    };
    let max_damage = max_tool_damage_for_path(&item_path)?;

    let held = state.inventory.held_mut(hotbar_slot);
    let new_damage = held.damage.unwrap_or(0).saturating_add(1);
    if new_damage >= max_damage {
        *held = ItemStack::EMPTY;
    } else {
        held.damage = Some(new_damage);
    }
    Some((wire_slot, held.clone()))
}

fn damage_held_weapon_stack(state: &mut InteractionState) -> Option<(usize, ItemStack)> {
    let hotbar_slot = state.selected_hotbar_slot;
    let wire_slot = PlayerInventory::HOTBAR_BASE + hotbar_slot as usize;
    let (max_damage, damage_per_attack) = {
        let held = state.inventory.held(hotbar_slot);
        if held.is_empty() {
            return None;
        }
        let item = state.items.name_of(held.item_id)?;
        let facts = state.item_facts.get(item);
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

    let held = state.inventory.held_mut(hotbar_slot);
    let damage_per_attack = i32::try_from(damage_per_attack).unwrap_or(i32::MAX);
    let new_damage = held.damage.unwrap_or(0).saturating_add(damage_per_attack);
    if new_damage >= max_damage {
        *held = ItemStack::EMPTY;
    } else {
        held.damage = Some(new_damage);
    }
    Some((wire_slot, held.clone()))
}

fn damage_held_bow_stack(state: &mut InteractionState) -> Option<(usize, ItemStack)> {
    let hotbar_slot = state.selected_hotbar_slot;
    let wire_slot = PlayerInventory::HOTBAR_BASE + hotbar_slot as usize;
    {
        let held = state.inventory.held(hotbar_slot);
        if held.is_empty()
            || state
                .items
                .name_of(held.item_id)
                .is_none_or(|item| item.as_str() != "minecraft:bow")
        {
            return None;
        }
    }

    let held = state.inventory.held_mut(hotbar_slot);
    let new_damage = held.damage.unwrap_or(0).saturating_add(1);
    if new_damage >= 384 {
        *held = ItemStack::EMPTY;
    } else {
        held.damage = Some(new_damage);
    }
    Some((wire_slot, held.clone()))
}

pub(super) async fn damage_held_tool_after_mining<W>(
    state: &mut InteractionState,
    writer: &mut W,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if let Some(changed) = damage_held_tool_stack(state) {
        write_inventory_slot_updates(state, writer, vec![changed]).await?;
    }
    Ok(())
}

pub(super) async fn damage_held_weapon_after_attack<W>(
    state: &mut InteractionState,
    writer: &mut W,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if let Some(changed) = damage_held_weapon_stack(state) {
        write_inventory_slot_updates(state, writer, vec![changed]).await?;
    }
    Ok(())
}

pub(super) async fn damage_held_bow_after_shot<W>(
    state: &mut InteractionState,
    writer: &mut W,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if let Some(changed) = damage_held_bow_stack(state) {
        write_inventory_slot_updates(state, writer, vec![changed]).await?;
    }
    Ok(())
}
