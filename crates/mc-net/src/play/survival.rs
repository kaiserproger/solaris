#[cfg(test)]
pub(super) use super::combat::is_durability_tool_path;
pub(super) use super::combat::max_tool_damage_for_path;
use super::plants::plant_drop_stacks;
use super::*;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct SurvivalState {
    pub(super) health: f32,
    pub(super) food: i32,
    pub(super) saturation: f32,
    pub(super) exhaustion: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum SurvivalHealthTick {
    Unchanged,
    Changed,
    StarvationDamage(f32),
}

impl SurvivalState {
    pub(super) const MAX_HEALTH: f32 = 20.0;
    pub(super) const MAX_FOOD: i32 = 20;
    pub(super) const BLOCK_BREAK_EXHAUSTION: f32 = 0.005;
    pub(super) const ENTITY_ATTACK_EXHAUSTION: f32 = 0.1;
    pub(super) const SPRINT_EXHAUSTION_PER_METER: f32 = 0.1;
    pub(super) const JUMP_EXHAUSTION: f32 = 0.05;
    pub(super) const SPRINT_JUMP_EXHAUSTION: f32 = 0.2;
    pub(super) const EXHAUSTION_STEP: f32 = 4.0;
    const SATURATED_REGEN_TICKS: u32 = 10;
    const HEALTH_TICK_PERIOD: u32 = 80;

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
        self.food = self.food.saturating_add(food).clamp(0, Self::MAX_FOOD);
        let current_saturation = if self.saturation.is_finite() {
            self.saturation.max(0.0)
        } else {
            0.0
        };
        let added_saturation = if saturation.is_finite() {
            saturation.max(0.0)
        } else {
            0.0
        };
        self.saturation = (current_saturation + added_saturation).min(self.food as f32);
    }

    pub(super) fn is_dead(self) -> bool {
        self.health <= 0.0
    }

    pub(super) fn add_exhaustion(&mut self, amount: f32) -> bool {
        let before_food = self.food;
        let before_saturation = self.saturation;
        self.food = self.food.clamp(0, Self::MAX_FOOD);
        self.saturation = if self.saturation.is_finite() {
            self.saturation.clamp(0.0, self.food as f32)
        } else {
            0.0
        };
        let current_exhaustion = if self.exhaustion.is_finite() {
            self.exhaustion.max(0.0)
        } else {
            0.0
        };
        let added_exhaustion = if amount.is_finite() {
            amount.max(0.0)
        } else {
            0.0
        };
        let total = current_exhaustion + added_exhaustion;
        let total = if total.is_finite() { total } else { f32::MAX };
        let resource_steps = (Self::MAX_FOOD * 2) as u32;
        let steps = ((total / Self::EXHAUSTION_STEP).floor() as u32).min(resource_steps);
        self.exhaustion = total % Self::EXHAUSTION_STEP;

        let saturation_steps = steps.min(self.saturation.ceil() as u32);
        self.saturation = (self.saturation - saturation_steps as f32).max(0.0);
        let food_steps = steps - saturation_steps;
        self.food = self.food.saturating_sub(food_steps as i32).max(0);
        self.food != before_food || self.saturation != before_saturation
    }

    pub(super) fn tick_health(&mut self, tick_timer: &mut u32) -> SurvivalHealthTick {
        if self.is_dead() {
            *tick_timer = 0;
            return SurvivalHealthTick::Unchanged;
        }

        if self.saturation > 0.0 && self.food >= Self::MAX_FOOD && self.health < Self::MAX_HEALTH {
            *tick_timer = tick_timer.saturating_add(1);
            if *tick_timer < Self::SATURATED_REGEN_TICKS {
                return SurvivalHealthTick::Unchanged;
            }
            *tick_timer = 0;
            let saturation_spent = self.saturation.min(6.0);
            self.heal(saturation_spent / 6.0);
            self.add_exhaustion(saturation_spent);
            return SurvivalHealthTick::Changed;
        }

        if self.food >= 18 && self.health < Self::MAX_HEALTH {
            *tick_timer = tick_timer.saturating_add(1);
            if *tick_timer < Self::HEALTH_TICK_PERIOD {
                return SurvivalHealthTick::Unchanged;
            }
            *tick_timer = 0;
            self.heal(1.0);
            self.add_exhaustion(6.0);
            return SurvivalHealthTick::Changed;
        }

        if self.food == 0 {
            *tick_timer = tick_timer.saturating_add(1);
            if *tick_timer < Self::HEALTH_TICK_PERIOD {
                return SurvivalHealthTick::Unchanged;
            }
            *tick_timer = 0;
            return SurvivalHealthTick::StarvationDamage(1.0);
        }

        *tick_timer = 0;
        SurvivalHealthTick::Unchanged
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BlockMutationSnapshot {
    pub(super) state: BlockStateId,
    pub(super) token: mc_world::BlockMutationToken,
}

#[derive(Debug, Clone)]
pub(super) struct PendingBreak {
    pub(super) position: i64,
    pub(super) direction: Direction,
    pub(super) started_tick: u64,
    pub(super) started_progress_per_tick: f32,
    pub(super) held_hotbar_slot: u8,
    pub(super) held_item: Option<ItemStack>,
    pub(super) expected_target: Option<BlockMutationSnapshot>,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum UseKind {
    Food(mc_data::food::FoodEntry),
    #[allow(dead_code)]
    Bow,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PendingUse {
    pub(super) started_tick: u64,
    pub(super) required_ticks: u64,
    pub(super) held_hotbar_slot: u8,
    pub(super) held_slot: usize,
    pub(super) held_item_id: u32,
    pub(super) kind: UseKind,
}

const VANILLA_STOP_DESTROY_THRESHOLD: f32 = 0.7;
const VANILLA_SUBMERGED_MINING_SPEED: f32 = 0.2;
const FALLBACK_UNKNOWN_DESTROY_SPEED: f32 = 0.8;

pub(super) fn held_item_id(state: &InteractionState) -> Option<u32> {
    held_item_stack(state).map(|held| held.item_id)
}

pub(super) fn held_item_stack(state: &InteractionState) -> Option<&ItemStack> {
    let held = state.inventory.held(state.selected_hotbar_slot);
    (!held.is_empty()).then_some(held)
}

pub(super) fn pending_break_matches(
    state: &InteractionState,
    pending: &PendingBreak,
    action: &ServerboundPlayerAction,
) -> bool {
    pending.position == action.position
        && pending.direction == action.direction
        && pending.held_hotbar_slot == state.selected_hotbar_slot
        && pending.held_item.as_ref() == held_item_stack(state)
}

pub(super) fn mining_ticks(duration: Duration) -> u64 {
    let ticks = duration.as_nanos().div_ceil(ENTITY_TICK_PERIOD.as_nanos());
    u64::try_from(ticks).unwrap_or(u64::MAX).max(1)
}

pub(super) fn item_use_ticks(duration: Duration) -> u64 {
    mining_ticks(duration)
}

pub(super) fn pending_break_is_complete(
    pending: &PendingBreak,
    current_tick: u64,
    progress_per_tick: f32,
) -> bool {
    let elapsed_with_start = current_tick
        .saturating_sub(pending.started_tick)
        .saturating_add(1);
    progress_per_tick * elapsed_with_start as f32 >= VANILLA_STOP_DESTROY_THRESHOLD
}

fn fallback_mining_facts(block_path: &str) -> mc_data::block_mining::BlockMiningFacts {
    use mc_data::block_mining::BlockMiningFacts;

    let (destroy_speed, requires_correct_tool_for_drops) = match block_path {
        "bedrock" | "barrier" | "end_portal_frame" => (-1.0, false),
        "air"
        | "cave_air"
        | "void_air"
        | "short_grass"
        | "tall_grass"
        | "wheat"
        | "carrots"
        | "potatoes"
        | "beetroots"
        | "nether_wart"
        | "pumpkin_stem"
        | "melon_stem"
        | "attached_pumpkin_stem"
        | "attached_melon_stem"
        | "sweet_berry_bush"
        | "sugar_cane"
        | "kelp"
        | "kelp_plant"
        | "torch"
        | "wall_torch" => (0.0, false),
        "cocoa" => (0.2, false),
        "stone" | "granite" | "diorite" | "andesite" | "calcite" | "tuff" => (1.5, true),
        "cobblestone" | "mossy_cobblestone" => (2.0, true),
        "deepslate" => (3.0, true),
        "cobbled_deepslate" => (3.5, true),
        "obsidian" | "crying_obsidian" => (50.0, true),
        "ancient_debris" => (30.0, true),
        "dirt" | "coarse_dirt" | "rooted_dirt" | "podzol" | "sand" | "red_sand" => (0.5, false),
        "grass_block" | "gravel" | "clay" => (0.6, false),
        "crafting_table" | "chest" | "trapped_chest" => (2.5, false),
        "furnace" | "blast_furnace" | "smoker" => (3.5, true),
        path if path.starts_with("deepslate_") && path.ends_with("_ore") => (4.5, true),
        path if path.ends_with("_ore") => (3.0, true),
        path if path.ends_with("_log")
            || path.ends_with("_wood")
            || path.ends_with("_stem")
            || path.ends_with("_hyphae")
            || path.ends_with("_planks") =>
        {
            (2.0, false)
        }
        _ => (FALLBACK_UNKNOWN_DESTROY_SPEED, false),
    };
    BlockMiningFacts {
        destroy_speed,
        requires_correct_tool_for_drops,
    }
}

fn fallback_tool_suffix_for_path(block_path: &str) -> Option<&'static str> {
    let facts = fallback_mining_facts(block_path);
    if facts.requires_correct_tool_for_drops
        || matches!(block_path, "furnace" | "blast_furnace" | "smoker")
    {
        return Some("_pickaxe");
    }
    if block_path.ends_with("_log")
        || block_path.ends_with("_wood")
        || block_path.ends_with("_stem")
        || block_path.ends_with("_hyphae")
        || block_path.ends_with("_planks")
        || matches!(block_path, "crafting_table" | "chest" | "trapped_chest")
    {
        return Some("_axe");
    }
    if matches!(
        block_path,
        "dirt"
            | "coarse_dirt"
            | "rooted_dirt"
            | "podzol"
            | "grass_block"
            | "sand"
            | "red_sand"
            | "gravel"
            | "clay"
    ) {
        return Some("_shovel");
    }
    None
}

fn fallback_tool_mining_speed(tool_path: Option<&str>, required_suffix: Option<&str>) -> f32 {
    let Some(tool_path) = tool_path else {
        return 1.0;
    };
    if required_suffix.is_some_and(|suffix| !tool_path.ends_with(suffix)) {
        return 1.0;
    }

    for (material, speed) in [
        ("wooden_", 2.0),
        ("stone_", 4.0),
        ("copper_", 5.0),
        ("iron_", 6.0),
        ("diamond_", 8.0),
        ("netherite_", 9.0),
        ("golden_", 12.0),
    ] {
        if tool_path.starts_with(material) {
            return speed;
        }
    }
    1.0
}

fn vanilla_destroy_progress_per_tick(
    destroy_speed: f32,
    mut item_speed: f32,
    has_correct_tool_for_drops: bool,
    on_ground: bool,
    eye_in_water: bool,
) -> f32 {
    if destroy_speed < 0.0 {
        return 0.0;
    }
    if destroy_speed == 0.0 {
        return f32::INFINITY;
    }
    if !item_speed.is_finite() || item_speed < 0.0 {
        item_speed = 1.0;
    }
    if eye_in_water {
        item_speed *= VANILLA_SUBMERGED_MINING_SPEED;
    }
    if !on_ground {
        item_speed /= 5.0;
    }
    let divisor = if has_correct_tool_for_drops {
        30.0
    } else {
        100.0
    };
    item_speed / destroy_speed / divisor
}

#[cfg(test)]
fn duration_to_full_break(progress_per_tick: f32) -> Duration {
    if progress_per_tick >= 1.0 {
        return Duration::ZERO;
    }
    if !progress_per_tick.is_finite() || progress_per_tick <= 0.0 {
        return Duration::MAX;
    }
    let ticks = (1.0 / progress_per_tick).ceil() as u64;
    Duration::from_millis(ticks.max(1).saturating_mul(50))
}

#[cfg(test)]
pub(super) fn fallback_mining_time(block_path: &str, tool_path: Option<&str>) -> Duration {
    let facts = fallback_mining_facts(block_path);
    let item_speed =
        fallback_tool_mining_speed(tool_path, fallback_tool_suffix_for_path(block_path));
    let correct = !facts.requires_correct_tool_for_drops
        || fallback_tool_allows_block_drop(block_path, tool_path);
    duration_to_full_break(vanilla_destroy_progress_per_tick(
        facts.destroy_speed,
        item_speed,
        correct,
        true,
        false,
    ))
}

fn pickaxe_tier(tool_path: &str) -> Option<u8> {
    let material = tool_path.strip_suffix("_pickaxe")?;
    match material {
        "wooden" | "golden" => Some(0),
        "stone" | "copper" => Some(1),
        "iron" => Some(2),
        "diamond" => Some(3),
        "netherite" => Some(4),
        _ => None,
    }
}

fn required_pickaxe_tier_for_drop(block_path: &str) -> Option<u8> {
    let block_path = block_path.strip_prefix("deepslate_").unwrap_or(block_path);
    match block_path {
        "stone" | "cobblestone" | "deepslate" | "cobbled_deepslate" | "coal_ore"
        | "nether_gold_ore" | "nether_quartz_ore" => Some(0),
        "iron_ore" | "copper_ore" | "lapis_ore" => Some(1),
        "diamond_ore" | "emerald_ore" | "gold_ore" | "redstone_ore" => Some(2),
        "obsidian" | "crying_obsidian" | "ancient_debris" => Some(3),
        _ => None,
    }
}

pub(super) fn fallback_tool_allows_block_drop(block_path: &str, tool_path: Option<&str>) -> bool {
    let Some(required_tier) = required_pickaxe_tier_for_drop(block_path) else {
        return true;
    };
    tool_path
        .and_then(pickaxe_tier)
        .is_some_and(|tier| tier >= required_tier)
}

pub(super) fn block_tag_contains(tags: &TagsData, tag: &str, block_raw_id: i32) -> bool {
    let Ok(block_registry) = Identifier::parse("minecraft:block") else {
        return false;
    };
    let Ok(tag) = Identifier::parse(tag.trim_start_matches('#').to_string()) else {
        return false;
    };
    tags.registries
        .get(&block_registry)
        .and_then(|block_tags| block_tags.get(&tag))
        .is_some_and(|entries| entries.binary_search(&block_raw_id).is_ok())
}

fn tool_rule_matches(
    rule: &mc_data::item_components::ToolRuleFacts,
    block: &mc_world::Block,
    tags: &TagsData,
) -> bool {
    rule.blocks.iter().any(|target| {
        if target.starts_with('#') {
            block_tag_contains(tags, target, block.raw_id)
        } else {
            target == block.id.as_str()
        }
    })
}

fn tool_rule_mining_speed(
    tool: &mc_data::item_components::ToolFacts,
    block: &mc_world::Block,
    tags: &TagsData,
) -> f32 {
    tool.rules
        .iter()
        .find_map(|rule| rule.speed.filter(|_| tool_rule_matches(rule, block, tags)))
        .or(tool.default_mining_speed)
        .unwrap_or(1.0)
}

fn tool_rule_is_correct_for_drops(
    tool: &mc_data::item_components::ToolFacts,
    block: &mc_world::Block,
    tags: &TagsData,
) -> bool {
    tool.rules.iter().find_map(|rule| {
        rule.correct_for_drops
            .filter(|_| tool_rule_matches(rule, block, tags))
    }) == Some(true)
}

fn fallback_tool_suffix_for_block(
    block: &mc_world::Block,
    tags: &TagsData,
) -> Option<&'static str> {
    for (tag, suffix) in [
        ("minecraft:mineable/pickaxe", "_pickaxe"),
        ("minecraft:mineable/axe", "_axe"),
        ("minecraft:mineable/shovel", "_shovel"),
        ("minecraft:mineable/hoe", "_hoe"),
    ] {
        if block_tag_contains(tags, tag, block.raw_id) {
            return Some(suffix);
        }
    }
    fallback_tool_suffix_for_path(block.id.path())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn mining_progress_for_block(
    blocks: &BlockRegistry,
    block_facts: &mc_data::block_facts::BlockFactsTable,
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    tags: &TagsData,
    block_state: BlockStateId,
    held_item: Option<&ItemStack>,
    player_pose: PlayerPose,
) -> f32 {
    let Some(block_state) = blocks.by_id(block_state) else {
        return 0.0;
    };
    let mining = block_facts
        .mining(block_state.id.0)
        .unwrap_or_else(|| fallback_mining_facts(block_state.block.id.path()));
    let tool_id = held_item.and_then(|stack| items.name_of(stack.item_id));
    let tool_path = tool_id.map(Identifier::path);
    let tool = tool_id
        .and_then(|id| item_facts.get(id))
        .and_then(|facts| facts.tool.as_ref());
    let (mut item_speed, tool_correct_for_drops) = tool.map_or_else(
        || {
            (
                fallback_tool_mining_speed(
                    tool_path,
                    fallback_tool_suffix_for_block(&block_state.block, tags),
                ),
                fallback_tool_allows_block_drop(block_state.block.id.path(), tool_path),
            )
        },
        |tool| {
            (
                tool_rule_mining_speed(tool, &block_state.block, tags),
                tool_rule_is_correct_for_drops(tool, &block_state.block, tags),
            )
        },
    );
    if item_speed > 1.0
        && let Some(level) = held_item.and_then(efficiency_level)
    {
        item_speed += level.saturating_mul(level).saturating_add(1) as f32;
    }
    let has_correct_tool = !mining.requires_correct_tool_for_drops || tool_correct_for_drops;
    vanilla_destroy_progress_per_tick(
        mining.destroy_speed,
        item_speed,
        has_correct_tool,
        player_pose.flags.on_ground,
        player_pose.eye_in_water,
    )
}

pub(super) fn block_break_is_denied(blocks: &BlockRegistry, block_state: BlockStateId) -> bool {
    blocks.by_id(block_state).is_some_and(|state| {
        matches!(
            state.block.id.path(),
            "bedrock" | "barrier" | "end_portal_frame"
        )
    })
}

pub(super) async fn mining_target_for(
    state: &InteractionState,
    position: i64,
    player_pose: PlayerPose,
) -> (Option<BlockMutationSnapshot>, f32) {
    let (x, y, z) = unpack_block_pos(position);
    let target = state
        .simulation
        .read_block_snapshot(mc_world::BlockPos { x, y, z })
        .await
        .map_err(|err| {
            warn!(error = ?err, x, y, z, "mining target read failed; using fallback timing");
        })
        .ok()
        .flatten();

    let progress_per_tick = target.map_or(0.0, |target| {
        mining_progress_for_block(
            &state.blocks,
            &state.block_facts,
            &state.items,
            &state.item_facts,
            &state.tags,
            target.state,
            held_item_stack(state),
            player_pose,
        )
    });
    (target, progress_per_tick)
}

fn efficiency_level(stack: &ItemStack) -> Option<i32> {
    stack
        .enchantments
        .iter()
        .find(|enchantment| enchantment.id.as_str() == "minecraft:efficiency")
        .map(|enchantment| enchantment.level)
        .filter(|level| *level > 0)
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
    mc_data::entity_types::fallback_entity_category(entity_type).is_hostile()
}

#[cfg(test)]
pub(super) fn mob_drop_stack_from(
    loot: &mc_data::loot::LootTables,
    items: &ItemRegistry,
    entity_type: &str,
) -> Option<ItemStack> {
    let facts = ItemFactsTable::default();
    mob_drop_stacks_from(loot, items, &facts, entity_type)
        .into_iter()
        .next()
}

#[cfg(test)]
pub(super) fn mob_drop_stacks_from(
    loot: &mc_data::loot::LootTables,
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    entity_type: &str,
) -> Vec<ItemStack> {
    mob_drop_stacks_from_seed(loot, items, item_facts, entity_type, 0)
}

pub(super) fn mob_drop_stacks_from_seed(
    loot: &mc_data::loot::LootTables,
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    entity_type: &str,
    seed: u64,
) -> Vec<ItemStack> {
    let Ok(entity) = Identifier::parse(entity_type.to_string()) else {
        return Vec::new();
    };
    let drops = loot
        .entity_drop_stacks(&entity)
        .or_else(|| mc_data::loot::builtin().entity_drop_stacks(&entity));
    let Some(drops) = drops else {
        return Vec::new();
    };
    drops
        .iter()
        .enumerate()
        .flat_map(|(index, drop)| {
            let Some(item_id) = items.id_of(&drop.item) else {
                return Vec::new();
            };
            let roll = splitmix64(seed ^ (index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            split_drop_stack(items, item_facts, item_id, drop.count.sample(roll))
        })
        .collect()
}

pub(super) fn mob_xp_value(entity_type: &str) -> i32 {
    match entity_type {
        "minecraft:zombie" | "minecraft:skeleton" | "minecraft:spider" => 5,
        "minecraft:cow" | "minecraft:pig" | "minecraft:sheep" | "minecraft:chicken" => 1,
        _ => 0,
    }
}

#[cfg(test)]
pub(super) fn block_drop_stacks_from(
    loot: &mc_data::loot::LootTables,
    items: &ItemRegistry,
    blocks: &mc_world::BlockRegistry,
    block_state: BlockStateId,
) -> Vec<ItemStack> {
    let facts = ItemFactsTable::default();
    block_drop_stacks_with_tool_and_facts_from(loot, items, &facts, blocks, block_state, None)
}

pub(super) fn block_drop_stacks_with_facts_from(
    loot: &mc_data::loot::LootTables,
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    blocks: &mc_world::BlockRegistry,
    block_state: BlockStateId,
) -> Vec<ItemStack> {
    block_drop_stacks_with_tool_and_facts_from(loot, items, item_facts, blocks, block_state, None)
}

#[cfg(test)]
fn block_drop_stacks_with_tool_from(
    loot: &mc_data::loot::LootTables,
    items: &ItemRegistry,
    blocks: &mc_world::BlockRegistry,
    block_state: BlockStateId,
    held_item_id: Option<u32>,
) -> Vec<ItemStack> {
    let facts = ItemFactsTable::default();
    block_drop_stacks_with_tool_and_facts_from(
        loot,
        items,
        &facts,
        blocks,
        block_state,
        held_item_id,
    )
}

pub(super) fn block_drop_stacks_with_tool_and_facts_from(
    loot: &mc_data::loot::LootTables,
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    blocks: &mc_world::BlockRegistry,
    block_state: BlockStateId,
    held_item_id: Option<u32>,
) -> Vec<ItemStack> {
    let held = held_item_id.map(|item_id| ItemStack::new(item_id, 1));
    block_drop_stacks_with_tool_and_facts_from_seeded(
        loot,
        items,
        item_facts,
        blocks,
        block_state,
        held.as_ref(),
        0,
    )
}

pub(super) fn block_drop_stacks_with_tool_and_facts_from_seeded(
    loot: &mc_data::loot::LootTables,
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    blocks: &mc_world::BlockRegistry,
    block_state: BlockStateId,
    held_item: Option<&ItemStack>,
    loot_seed: u64,
) -> Vec<ItemStack> {
    let Some(block) = blocks.by_id(block_state) else {
        return Vec::new();
    };
    let held_item = held_item.filter(|stack| !stack.is_empty());
    let tool_path = held_item
        .map(|stack| stack.item_id)
        .and_then(|item_id| items.name_of(item_id))
        .map(mc_data::Identifier::path);
    if !fallback_tool_allows_block_drop(block.block.id.path(), tool_path) {
        return Vec::new();
    }

    let fallback = mc_data::loot::builtin();
    let configured_drops = loot.block_drop_stacks(&block.block.id);
    let (contextual, drops) = if configured_drops.is_some() {
        (loot.block_loot(&block.block.id), configured_drops)
    } else {
        (
            fallback.block_loot(&block.block.id),
            fallback.block_drop_stacks(&block.block.id),
        )
    };
    if let Some(contextual) = contextual {
        let silk_touch = enchantment_level(held_item, "minecraft:silk_touch") > 0;
        let fortune_level = enchantment_level(held_item, "minecraft:fortune");
        return contextual
            .drops_for_context(silk_touch, &block.block.id, &block.properties)
            .into_iter()
            .enumerate()
            .flat_map(|(index, drop)| {
                let Some(item_id) = items.id_of(&drop.drop.item) else {
                    return Vec::new();
                };
                let count_roll = block_drop_roll(loot_seed, index);
                let bonus_roll = splitmix64(count_roll);
                if !drop.passes_random_chance(splitmix64(bonus_roll)) {
                    return Vec::new();
                }
                split_drop_stack(
                    items,
                    item_facts,
                    item_id,
                    drop.sample_count(count_roll, fortune_level, bonus_roll),
                )
            })
            .collect();
    }
    if let Some(crop_drops) = plant_drop_stacks(items, block) {
        return crop_drops
            .into_iter()
            .flat_map(|stack| {
                split_drop_stack(
                    items,
                    item_facts,
                    stack.item_id,
                    u32::try_from(stack.count).unwrap_or(0),
                )
            })
            .collect();
    }
    if let Some(drops) = drops {
        return drops
            .iter()
            .enumerate()
            .flat_map(|(index, drop)| {
                let Some(item_id) = items.id_of(&drop.item) else {
                    return Vec::new();
                };
                let roll = block_drop_roll(loot_seed, index);
                split_drop_stack(items, item_facts, item_id, drop.count.sample(roll))
            })
            .collect();
    }

    items
        .id_of(&block.block.id)
        .map(|item_id| split_drop_stack(items, item_facts, item_id, 1))
        .unwrap_or_default()
}

fn enchantment_level(held_item: Option<&ItemStack>, enchantment_id: &str) -> u32 {
    held_item
        .into_iter()
        .flat_map(|stack| &stack.enchantments)
        .filter(|enchantment| enchantment.id.as_str() == enchantment_id)
        .filter_map(|enchantment| u32::try_from(enchantment.level).ok())
        .max()
        .unwrap_or(0)
}

fn block_drop_roll(loot_seed: u64, pool_index: usize) -> u64 {
    if pool_index == 0 {
        loot_seed
    } else {
        splitmix64(loot_seed ^ (pool_index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
    }
}

fn split_drop_stack(
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    item_id: u32,
    count: u32,
) -> Vec<ItemStack> {
    let max_stack = u32::try_from(item_max_stack(
        item_facts,
        items,
        &ItemStack::new(item_id, 1),
    ))
    .unwrap_or(1)
    .max(1);
    let mut remaining = count;
    let mut stacks = Vec::new();
    while remaining > 0 {
        let stack_count = remaining.min(max_stack);
        stacks.push(ItemStack::new(
            item_id,
            i32::try_from(stack_count).expect("item max stack fits i32"),
        ));
        remaining -= stack_count;
    }
    stacks
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
    held_slot: usize,
) -> Option<(u32, mc_data::food::FoodEntry, Duration)> {
    let held = state.inventory.slots.get(held_slot)?;
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
pub(super) fn is_bow_item(state: &InteractionState, held_slot: usize) -> bool {
    let Some(held) = state.inventory.slots.get(held_slot) else {
        return false;
    };
    if held.is_empty() {
        return false;
    }
    state
        .items
        .name_of(held.item_id)
        .is_some_and(|item| item.as_str() == "minecraft:bow")
}

fn stack_is_arrow(state: &InteractionState, slot: usize) -> bool {
    state
        .inventory
        .slots
        .get(slot)
        .filter(|stack| !stack.is_empty())
        .and_then(|stack| state.items.name_of(stack.item_id))
        .is_some_and(|item| item.as_str() == "minecraft:arrow")
}

#[allow(dead_code)]
pub(super) fn available_arrow_slot(state: &InteractionState) -> Option<usize> {
    if stack_is_arrow(state, PlayerInventory::OFFHAND_SLOT) {
        return Some(PlayerInventory::OFFHAND_SLOT);
    }

    let main_hand_slot = PlayerInventory::HOTBAR_BASE + usize::from(state.selected_hotbar_slot);
    if stack_is_arrow(state, main_hand_slot) {
        return Some(main_hand_slot);
    }

    for hotbar_slot in 0..9u8 {
        let slot = PlayerInventory::HOTBAR_BASE + usize::from(hotbar_slot);
        if slot != main_hand_slot && stack_is_arrow(state, slot) {
            return Some(slot);
        }
    }
    (9..PlayerInventory::HOTBAR_BASE).find(|&slot| stack_is_arrow(state, slot))
}

pub(super) fn held_bow_max_damage(state: &InteractionState, held_slot: usize) -> Option<i32> {
    let held = state.inventory.slots.get(held_slot)?;
    if held.is_empty() {
        return None;
    }
    let item = state.items.name_of(held.item_id)?;
    if item.as_str() != "minecraft:bow" {
        return None;
    }
    Some(
        state
            .item_facts
            .get(item)
            .and_then(|facts| facts.max_damage)
            .and_then(|value| i32::try_from(value).ok())
            .unwrap_or(384)
            .max(1),
    )
}

pub(super) fn bow_draw_power(started_tick: u64, current_tick: u64) -> f64 {
    let ticks = current_tick.saturating_sub(started_tick).min(20) as f64;
    let elapsed = ticks / 20.0;
    let power = (elapsed * elapsed + elapsed * 2.0) / 3.0;
    if power < 0.1 {
        0.0
    } else {
        power.clamp(0.0, 1.0)
    }
}

pub(super) fn pending_use_matches(state: &InteractionState, pending: &PendingUse) -> bool {
    let selected_slot = PlayerInventory::HOTBAR_BASE + usize::from(state.selected_hotbar_slot);
    (pending.held_slot == PlayerInventory::OFFHAND_SLOT
        || (pending.held_hotbar_slot == state.selected_hotbar_slot
            && pending.held_slot == selected_slot))
        && state
            .inventory
            .slots
            .get(pending.held_slot)
            .is_some_and(|stack| stack.item_id == pending.held_item_id)
}

pub(super) fn pending_use_is_complete(pending: &PendingUse, current_tick: u64) -> bool {
    current_tick.saturating_sub(pending.started_tick) >= pending.required_ticks
}

pub(super) fn entity_item_stack(stack: ItemStack) -> EntityItemStack {
    EntityItemStack {
        item_id: stack.item_id,
        count: stack.count,
        damage: stack.damage,
        enchantments: stack.enchantments,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use mc_data::blocks::{BlockReport, BlockStateReport};
    use mc_data::items::ItemReport;

    use crate::play::combat::attack_damage_for_item;

    #[test]
    fn empty_hand_uses_vanilla_player_base_attack_damage() {
        assert_eq!(
            attack_damage_for_item(
                &ItemFactsTable::default(),
                &mc_data::items::solaris_required_items(),
                None,
            ),
            1.0
        );
    }

    #[test]
    fn ordinary_item_without_modifier_uses_vanilla_player_base_attack_damage() {
        let items = mc_data::items::solaris_required_items();
        let dirt = items
            .id_of(&Identifier::parse("minecraft:dirt").unwrap())
            .expect("required dirt item");
        assert_eq!(
            attack_damage_for_item(&ItemFactsTable::default(), &items, Some(dirt)),
            1.0
        );
    }

    #[test]
    fn mining_duration_rounds_up_to_simulation_ticks() {
        assert_eq!(mining_ticks(Duration::ZERO), 1);
        assert_eq!(mining_ticks(Duration::from_millis(1)), 1);
        assert_eq!(mining_ticks(Duration::from_millis(50)), 1);
        assert_eq!(mining_ticks(Duration::from_millis(51)), 2);
        assert_eq!(mining_ticks(Duration::from_millis(1_600)), 32);
    }

    #[test]
    fn pending_break_completion_uses_simulation_ticks() {
        let pending = PendingBreak {
            position: 0,
            direction: Direction::Up,
            started_tick: 10,
            started_progress_per_tick: 0.2,
            held_hotbar_slot: 0,
            held_item: None,
            expected_target: None,
        };

        assert!(!pending_break_is_complete(&pending, 12, 0.2));
        assert!(pending_break_is_complete(&pending, 13, 0.2));
    }

    #[test]
    fn fallback_mining_uses_vanilla_261_tool_component_speeds() {
        for (tool, expected_ms) in [
            ("wooden_pickaxe", 1_150),
            ("stone_pickaxe", 600),
            ("copper_pickaxe", 450),
            ("iron_pickaxe", 400),
            ("diamond_pickaxe", 300),
            ("netherite_pickaxe", 250),
            ("golden_pickaxe", 200),
        ] {
            assert_eq!(
                fallback_mining_time("stone", Some(tool)),
                Duration::from_millis(expected_ms),
                "wrong fallback mining speed for {tool}"
            );
        }
    }

    #[test]
    fn fallback_mining_uses_vanilla_hardness_and_correct_tool_divisor() {
        assert_eq!(
            fallback_mining_time("stone", None),
            Duration::from_millis(7_500)
        );
        assert_eq!(
            fallback_mining_time("oak_log", None),
            Duration::from_millis(3_000)
        );
        assert_eq!(
            fallback_mining_time("oak_log", Some("stone_axe")),
            Duration::from_millis(750)
        );
        assert_eq!(
            fallback_mining_time("dirt", None),
            Duration::from_millis(750)
        );
        assert_eq!(
            fallback_mining_time("cocoa", None),
            Duration::from_millis(300)
        );
        assert_eq!(fallback_mining_time("nether_wart", None), Duration::ZERO);
    }

    #[test]
    fn oracle_mining_uses_block_hardness_tool_rules_and_player_state() {
        let block = |id: &str, state_id: u32| BlockReport {
            id: Identifier::parse(id).unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: state_id,
                default: true,
                properties: BTreeMap::new(),
            }],
        };
        let report = vec![
            block("minecraft:air", 0),
            block("minecraft:stone", 1),
            block("minecraft:oak_log", 2),
        ];
        let blocks = BlockRegistry::from_report(&report).unwrap();
        let mining = mc_data::block_mining::BlockMiningTable::from_arrays(
            "26.1.2-test",
            vec![0.0, 1.5, 2.0],
            vec![false, true, false],
        );
        let block_facts = mc_data::block_facts::BlockFactsTable::from_blocks_report_with_mining(
            &report,
            Some(&mining),
        );
        let wooden_pickaxe = Identifier::parse("minecraft:wooden_pickaxe").unwrap();
        let items = ItemRegistry::from_report(&[ItemReport {
            id: wooden_pickaxe.clone(),
            protocol_id: 10,
        }]);
        let item_facts = ItemFactsTable::from_entries([(
            wooden_pickaxe,
            mc_data::item_components::ItemFacts {
                tool: Some(mc_data::item_components::ToolFacts {
                    default_mining_speed: Some(1.0),
                    damage_per_block: Some(1),
                    rules: vec![mc_data::item_components::ToolRuleFacts {
                        blocks: vec!["#minecraft:mineable/pickaxe".to_owned()],
                        speed: Some(2.0),
                        correct_for_drops: Some(true),
                    }],
                }),
                ..Default::default()
            },
        )]);
        let block_registry = Identifier::parse("minecraft:block").unwrap();
        let pickaxe_tag = Identifier::parse("minecraft:mineable/pickaxe").unwrap();
        let tags = TagsData {
            registries: BTreeMap::from([(
                block_registry,
                BTreeMap::from([(pickaxe_tag, vec![1])]),
            )]),
        };
        let mut grounded = PlayerPose::new(0.0, 64.0, 0.0);
        grounded.flags = MovePlayerFlags::new(true, false);

        let progress = |state, held: Option<&ItemStack>, pose| {
            mining_progress_for_block(
                &blocks,
                &block_facts,
                &items,
                &item_facts,
                &tags,
                BlockStateId(state),
                held,
                pose,
            )
        };
        let pickaxe_id = items
            .id_of(&Identifier::parse("minecraft:wooden_pickaxe").unwrap())
            .unwrap();
        let pickaxe = ItemStack::new(pickaxe_id, 1);
        let efficient_pickaxe = pickaxe
            .clone()
            .with_enchantment(Identifier::parse("minecraft:efficiency").unwrap(), 1);
        let stone_with_pickaxe = 2.0 / 1.5 / 30.0;
        assert!((progress(1, Some(&pickaxe), grounded) - stone_with_pickaxe).abs() < f32::EPSILON);
        assert!(
            (progress(1, Some(&efficient_pickaxe), grounded) - (4.0 / 1.5 / 30.0)).abs()
                < f32::EPSILON
        );
        assert!((progress(1, None, grounded) - (1.0 / 1.5 / 100.0)).abs() < f32::EPSILON);
        assert!((progress(2, None, grounded) - (1.0 / 2.0 / 30.0)).abs() < f32::EPSILON);

        let mut submerged = grounded;
        submerged.eye_in_water = true;
        assert!(
            (progress(1, Some(&pickaxe), submerged)
                - stone_with_pickaxe * VANILLA_SUBMERGED_MINING_SPEED)
                .abs()
                < f32::EPSILON
        );

        let mut airborne = grounded;
        airborne.flags = MovePlayerFlags::new(false, false);
        assert!(
            (progress(1, Some(&pickaxe), airborne) - stone_with_pickaxe / 5.0).abs() < f32::EPSILON
        );
    }

    #[test]
    fn common_ore_loot_stacks_require_the_progression_pickaxe() {
        let block = |id: &str, state_id: u32| BlockReport {
            id: mc_data::Identifier::parse(id).unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: state_id,
                default: true,
                properties: BTreeMap::new(),
            }],
        };
        let blocks = mc_world::BlockRegistry::from_report(&[
            block("minecraft:air", 0),
            block("minecraft:iron_ore", 1),
            block("minecraft:diamond_ore", 2),
        ])
        .unwrap();
        let items = ItemRegistry::from_report(&[
            ItemReport {
                id: mc_data::Identifier::parse("minecraft:raw_iron").unwrap(),
                protocol_id: 10,
            },
            ItemReport {
                id: mc_data::Identifier::parse("minecraft:diamond").unwrap(),
                protocol_id: 11,
            },
            ItemReport {
                id: mc_data::Identifier::parse("minecraft:wooden_pickaxe").unwrap(),
                protocol_id: 12,
            },
            ItemReport {
                id: mc_data::Identifier::parse("minecraft:stone_pickaxe").unwrap(),
                protocol_id: 13,
            },
            ItemReport {
                id: mc_data::Identifier::parse("minecraft:iron_pickaxe").unwrap(),
                protocol_id: 14,
            },
        ]);
        let loot = mc_data::loot::LootTables::default();

        assert!(
            block_drop_stacks_with_tool_from(&loot, &items, &blocks, BlockStateId(1), Some(12),)
                .is_empty()
        );
        assert_eq!(
            block_drop_stacks_with_tool_from(&loot, &items, &blocks, BlockStateId(1), Some(13),),
            vec![ItemStack::new(10, 1)]
        );
        assert!(
            block_drop_stacks_with_tool_from(&loot, &items, &blocks, BlockStateId(2), Some(13),)
                .is_empty()
        );
        assert_eq!(
            block_drop_stacks_with_tool_from(&loot, &items, &blocks, BlockStateId(2), Some(14),),
            vec![ItemStack::new(11, 1)]
        );
    }

    #[test]
    fn block_uniform_loot_count_uses_the_break_transaction_seed() {
        let block = |id: &str, state_id: u32| BlockReport {
            id: mc_data::Identifier::parse(id).unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: state_id,
                default: true,
                properties: BTreeMap::new(),
            }],
        };
        let blocks = mc_world::BlockRegistry::from_report(&[
            block("minecraft:air", 0),
            block("minecraft:lapis_ore", 1),
        ])
        .unwrap();
        let lapis = mc_data::Identifier::parse("minecraft:lapis_lazuli").unwrap();
        let items = ItemRegistry::from_report(&[
            ItemReport {
                id: lapis.clone(),
                protocol_id: 10,
            },
            ItemReport {
                id: mc_data::Identifier::parse("minecraft:iron_pickaxe").unwrap(),
                protocol_id: 11,
            },
        ]);
        let loot = mc_data::loot::LootTables::from_drop_maps(
            BTreeMap::new(),
            BTreeMap::from([(
                mc_data::Identifier::parse("minecraft:lapis_ore").unwrap(),
                mc_data::loot::LootDrop::uniform(lapis, 4, 9),
            )]),
        );
        let facts = ItemFactsTable::default();
        let held = ItemStack::new(11, 1);
        let drops_for_seed = |seed| {
            block_drop_stacks_with_tool_and_facts_from_seeded(
                &loot,
                &items,
                &facts,
                &blocks,
                BlockStateId(1),
                Some(&held),
                seed,
            )
        };

        assert_eq!(drops_for_seed(0), vec![ItemStack::new(10, 4)]);
        assert_eq!(drops_for_seed(5), vec![ItemStack::new(10, 9)]);
        assert_eq!(drops_for_seed(6), vec![ItemStack::new(10, 4)]);
        assert_eq!(drops_for_seed(5), drops_for_seed(5));
    }

    fn contextual_diamond_ore_loot() -> mc_data::loot::LootTables {
        let tmp = tempfile::tempdir().unwrap();
        let blocks = tmp.path().join("blocks");
        std::fs::create_dir_all(&blocks).unwrap();
        std::fs::write(
            blocks.join("diamond_ore.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:alternatives",
                  "children": [
                    {
                      "type": "minecraft:item",
                      "conditions": [{
                        "condition": "minecraft:match_tool",
                        "predicate": {
                          "predicates": {
                            "minecraft:enchantments": [{
                              "enchantments": "minecraft:silk_touch",
                              "levels": { "min": 1 }
                            }]
                          }
                        }
                      }],
                      "name": "minecraft:diamond_ore"
                    },
                    {
                      "type": "minecraft:item",
                      "functions": [{
                        "function": "minecraft:set_count",
                        "count": {
                          "type": "minecraft:uniform",
                          "min": 1,
                          "max": 3
                        }
                      }, {
                        "enchantment": "minecraft:fortune",
                        "formula": "minecraft:ore_drops",
                        "function": "minecraft:apply_bonus"
                      }],
                      "name": "minecraft:diamond"
                    }
                  ]
                }]
              }]
            }"#,
        )
        .unwrap();
        mc_data::loot::load_vanilla_subset(tmp.path()).unwrap()
    }

    fn contextual_diamond_ore_data() -> (mc_world::BlockRegistry, ItemRegistry, u32) {
        let block = |id: &str, state_id: u32| BlockReport {
            id: mc_data::Identifier::parse(id).unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: state_id,
                default: true,
                properties: BTreeMap::new(),
            }],
        };
        let blocks = mc_world::BlockRegistry::from_report(&[
            block("minecraft:air", 0),
            block("minecraft:diamond_ore", 1),
        ])
        .unwrap();
        let items = ItemRegistry::from_report(&[
            ItemReport {
                id: mc_data::Identifier::parse("minecraft:diamond").unwrap(),
                protocol_id: 10,
            },
            ItemReport {
                id: mc_data::Identifier::parse("minecraft:diamond_ore").unwrap(),
                protocol_id: 11,
            },
            ItemReport {
                id: mc_data::Identifier::parse("minecraft:iron_pickaxe").unwrap(),
                protocol_id: 12,
            },
        ]);
        (blocks, items, 12)
    }

    #[test]
    fn silk_touch_ore_drops_the_ore_block() {
        let loot = contextual_diamond_ore_loot();
        let (blocks, items, pickaxe_id) = contextual_diamond_ore_data();
        let facts = ItemFactsTable::default();
        let held = ItemStack::new(pickaxe_id, 1)
            .with_enchantment(Identifier::parse("minecraft:silk_touch").unwrap(), 1);

        assert_eq!(
            block_drop_stacks_with_tool_and_facts_from_seeded(
                &loot,
                &items,
                &facts,
                &blocks,
                BlockStateId(1),
                Some(&held),
                3,
            ),
            vec![ItemStack::new(11, 1)]
        );
    }

    #[test]
    fn fortune_level_changes_ore_count_for_the_break_seed() {
        let loot = contextual_diamond_ore_loot();
        let (blocks, items, pickaxe_id) = contextual_diamond_ore_data();
        let facts = ItemFactsTable::default();
        let plain = ItemStack::new(pickaxe_id, 1);
        let fortunate = plain
            .clone()
            .with_enchantment(Identifier::parse("minecraft:fortune").unwrap(), 3);
        let drops = |held| {
            block_drop_stacks_with_tool_and_facts_from_seeded(
                &loot,
                &items,
                &facts,
                &blocks,
                BlockStateId(1),
                Some(held),
                3,
            )
        };

        assert_eq!(drops(&plain), vec![ItemStack::new(10, 1)]);
        assert_eq!(drops(&fortunate), vec![ItemStack::new(10, 3)]);
        assert_eq!(drops(&fortunate), drops(&fortunate));
    }

    #[test]
    fn fortune_level_zero_preserves_current_block_drop_behavior() {
        let loot = contextual_diamond_ore_loot();
        let (blocks, items, pickaxe_id) = contextual_diamond_ore_data();
        let facts = ItemFactsTable::default();
        let plain = ItemStack::new(pickaxe_id, 1);
        let fortune_zero = plain
            .clone()
            .with_enchantment(Identifier::parse("minecraft:fortune").unwrap(), 0);

        for seed in 0..8 {
            let drops = |held| {
                block_drop_stacks_with_tool_and_facts_from_seeded(
                    &loot,
                    &items,
                    &facts,
                    &blocks,
                    BlockStateId(1),
                    Some(held),
                    seed,
                )
            };
            assert_eq!(drops(&fortune_zero), drops(&plain), "seed {seed}");
            assert_eq!(
                drops(&plain),
                vec![ItemStack::new(10, 1 + i32::try_from(seed % 3).unwrap())],
                "seed {seed}"
            );
        }
    }

    #[test]
    fn binomial_fortune_uses_the_break_transaction_seed_deterministically() {
        let tmp = tempfile::tempdir().unwrap();
        let loot_root = tmp.path().join("blocks");
        std::fs::create_dir_all(&loot_root).unwrap();
        std::fs::write(
            loot_root.join("test_crop.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:item",
                  "functions": [{
                    "enchantment": "minecraft:fortune",
                    "formula": "minecraft:binomial_with_bonus_count",
                    "function": "minecraft:apply_bonus",
                    "parameters": { "extra": 3, "probability": 0.5 }
                  }],
                  "name": "minecraft:wheat_seeds"
                }]
              }]
            }"#,
        )
        .unwrap();
        let loot = mc_data::loot::load_vanilla_subset(tmp.path()).unwrap();
        let blocks = mc_world::BlockRegistry::from_report(&[
            BlockReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 0,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:test_crop").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 1,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
        ])
        .unwrap();
        let items = ItemRegistry::from_report(&[
            ItemReport {
                id: Identifier::parse("minecraft:wheat_seeds").unwrap(),
                protocol_id: 10,
            },
            ItemReport {
                id: Identifier::parse("minecraft:iron_hoe").unwrap(),
                protocol_id: 11,
            },
        ]);
        let facts = ItemFactsTable::default();
        let plain = ItemStack::new(11, 1);
        let fortunate = plain
            .clone()
            .with_enchantment(Identifier::parse("minecraft:fortune").unwrap(), 2);
        let drops = |held: &ItemStack, seed| {
            block_drop_stacks_with_tool_and_facts_from_seeded(
                &loot,
                &items,
                &facts,
                &blocks,
                BlockStateId(1),
                Some(held),
                seed,
            )
        };

        let baseline = drops(&plain, 91);
        let seeded = drops(&fortunate, 91);
        assert_eq!(baseline, drops(&plain, 91));
        assert!((1..=4).contains(&baseline[0].count));
        assert!((1..=6).contains(&seeded[0].count));
        assert_eq!(seeded, drops(&fortunate, 91));
        assert!((0..32).any(|seed| drops(&fortunate, seed) != seeded));
    }

    #[test]
    fn crop_state_loot_precedes_the_legacy_plant_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let loot_root = tmp.path().join("blocks");
        std::fs::create_dir_all(&loot_root).unwrap();
        std::fs::write(
            loot_root.join("wheat.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:alternatives",
                  "children": [{
                    "type": "minecraft:item",
                    "conditions": [{
                      "block": "minecraft:wheat",
                      "condition": "minecraft:block_state_property",
                      "properties": { "age": "7" }
                    }],
                    "name": "minecraft:wheat"
                  }, {
                    "type": "minecraft:item",
                    "name": "minecraft:wheat_seeds"
                  }]
                }]
              }, {
                "conditions": [{
                  "block": "minecraft:wheat",
                  "condition": "minecraft:block_state_property",
                  "properties": { "age": "7" }
                }],
                "entries": [{
                  "type": "minecraft:item",
                  "functions": [{
                    "enchantment": "minecraft:fortune",
                    "formula": "minecraft:binomial_with_bonus_count",
                    "function": "minecraft:apply_bonus",
                    "parameters": { "extra": 3, "probability": 1.0 }
                  }],
                  "name": "minecraft:wheat_seeds"
                }]
              }]
            }"#,
        )
        .unwrap();
        let loot = mc_data::loot::load_vanilla_subset(tmp.path()).unwrap();
        let blocks = mc_world::BlockRegistry::from_report(&[
            BlockReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 0,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:wheat").unwrap(),
                properties: BTreeMap::from([(
                    "age".to_string(),
                    vec!["6".to_string(), "7".to_string()],
                )]),
                states: vec![
                    BlockStateReport {
                        id: 1,
                        default: true,
                        properties: BTreeMap::from([("age".to_string(), "6".to_string())]),
                    },
                    BlockStateReport {
                        id: 2,
                        default: false,
                        properties: BTreeMap::from([("age".to_string(), "7".to_string())]),
                    },
                ],
            },
        ])
        .unwrap();
        let items = ItemRegistry::from_report(&[
            ItemReport {
                id: Identifier::parse("minecraft:wheat").unwrap(),
                protocol_id: 10,
            },
            ItemReport {
                id: Identifier::parse("minecraft:wheat_seeds").unwrap(),
                protocol_id: 11,
            },
            ItemReport {
                id: Identifier::parse("minecraft:iron_hoe").unwrap(),
                protocol_id: 12,
            },
        ]);
        let facts = ItemFactsTable::default();
        let plain = ItemStack::new(12, 1);
        let fortune = plain
            .clone()
            .with_enchantment(Identifier::parse("minecraft:fortune").unwrap(), 2);
        let drops = |state, held: &ItemStack| {
            block_drop_stacks_with_tool_and_facts_from_seeded(
                &loot,
                &items,
                &facts,
                &blocks,
                state,
                Some(held),
                0,
            )
        };

        assert_eq!(drops(BlockStateId(1), &plain), vec![ItemStack::new(11, 1)]);
        assert_eq!(
            drops(BlockStateId(2), &plain),
            vec![ItemStack::new(10, 1), ItemStack::new(11, 4)]
        );
        assert_eq!(
            drops(BlockStateId(2), &fortune),
            vec![ItemStack::new(10, 1), ItemStack::new(11, 6)]
        );
    }

    #[test]
    fn crop_random_chance_drop_is_seeded_and_optional() {
        let tmp = tempfile::tempdir().unwrap();
        let loot_root = tmp.path().join("blocks");
        std::fs::create_dir_all(&loot_root).unwrap();
        std::fs::write(
            loot_root.join("potatoes.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:item",
                  "name": "minecraft:potato"
                }]
              }, {
                "conditions": [{
                  "block": "minecraft:potatoes",
                  "condition": "minecraft:block_state_property",
                  "properties": { "age": "7" }
                }],
                "entries": [{
                  "type": "minecraft:item",
                  "conditions": [{
                    "chance": 0.5,
                    "condition": "minecraft:random_chance"
                  }],
                  "name": "minecraft:poisonous_potato"
                }]
              }]
            }"#,
        )
        .unwrap();
        let loot = mc_data::loot::load_vanilla_subset(tmp.path()).unwrap();
        let blocks = mc_world::BlockRegistry::from_report(&[
            BlockReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 0,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:potatoes").unwrap(),
                properties: BTreeMap::from([("age".to_string(), vec!["7".to_string()])]),
                states: vec![BlockStateReport {
                    id: 1,
                    default: true,
                    properties: BTreeMap::from([("age".to_string(), "7".to_string())]),
                }],
            },
        ])
        .unwrap();
        let items = ItemRegistry::from_report(&[
            ItemReport {
                id: Identifier::parse("minecraft:potato").unwrap(),
                protocol_id: 10,
            },
            ItemReport {
                id: Identifier::parse("minecraft:poisonous_potato").unwrap(),
                protocol_id: 11,
            },
            ItemReport {
                id: Identifier::parse("minecraft:iron_hoe").unwrap(),
                protocol_id: 12,
            },
        ]);
        let facts = ItemFactsTable::default();
        let held = ItemStack::new(12, 1);
        let drops = |seed| {
            block_drop_stacks_with_tool_and_facts_from_seeded(
                &loot,
                &items,
                &facts,
                &blocks,
                BlockStateId(1),
                Some(&held),
                seed,
            )
        };

        assert_eq!(drops(17), drops(17));
        assert!((0..64).any(|seed| drops(seed) == vec![ItemStack::new(10, 1)]));
        assert!(
            (0..64)
                .any(|seed| { drops(seed) == vec![ItemStack::new(10, 1), ItemStack::new(11, 1)] })
        );
    }

    #[test]
    fn block_loot_emits_every_independent_pool_drop() {
        let block = |id: &str, state_id: u32| BlockReport {
            id: mc_data::Identifier::parse(id).unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: state_id,
                default: true,
                properties: BTreeMap::new(),
            }],
        };
        let blocks = mc_world::BlockRegistry::from_report(&[
            block("minecraft:air", 0),
            block("minecraft:potted_oak_sapling", 1),
        ])
        .unwrap();
        let flower_pot = mc_data::Identifier::parse("minecraft:flower_pot").unwrap();
        let oak_sapling = mc_data::Identifier::parse("minecraft:oak_sapling").unwrap();
        let items = ItemRegistry::from_report(&[
            ItemReport {
                id: flower_pot.clone(),
                protocol_id: 10,
            },
            ItemReport {
                id: oak_sapling.clone(),
                protocol_id: 11,
            },
        ]);
        let loot = mc_data::loot::LootTables::from_drop_lists(
            BTreeMap::new(),
            BTreeMap::from([(
                mc_data::Identifier::parse("minecraft:potted_oak_sapling").unwrap(),
                vec![
                    mc_data::loot::LootDrop::single(flower_pot),
                    mc_data::loot::LootDrop::single(oak_sapling),
                ],
            )]),
        );

        assert_eq!(
            block_drop_stacks_from(&loot, &items, &blocks, BlockStateId(1)),
            vec![ItemStack::new(10, 1), ItemStack::new(11, 1)]
        );
    }

    #[test]
    fn partial_configured_loot_falls_back_per_missing_key() {
        let blocks = mc_world::BlockRegistry::from_report(&[
            BlockReport {
                id: mc_data::Identifier::parse("minecraft:air").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 0,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: mc_data::Identifier::parse("minecraft:stone").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 1,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
        ])
        .unwrap();
        let items = ItemRegistry::from_report(&[
            ItemReport {
                id: mc_data::Identifier::parse("minecraft:cobblestone").unwrap(),
                protocol_id: 10,
            },
            ItemReport {
                id: mc_data::Identifier::parse("minecraft:beef").unwrap(),
                protocol_id: 11,
            },
            ItemReport {
                id: mc_data::Identifier::parse("minecraft:wooden_pickaxe").unwrap(),
                protocol_id: 12,
            },
        ]);
        let configured = mc_data::loot::LootTables::from_maps(
            BTreeMap::from([(
                mc_data::Identifier::parse("minecraft:zombie").unwrap(),
                mc_data::Identifier::parse("minecraft:rotten_flesh").unwrap(),
            )]),
            BTreeMap::from([(
                mc_data::Identifier::parse("minecraft:dirt").unwrap(),
                mc_data::Identifier::parse("minecraft:dirt").unwrap(),
            )]),
        );

        assert_eq!(
            block_drop_stacks_with_tool_from(
                &configured,
                &items,
                &blocks,
                BlockStateId(1),
                Some(12),
            ),
            vec![ItemStack::new(10, 1)]
        );
        assert_eq!(
            mob_drop_stack_from(&configured, &items, "minecraft:cow"),
            Some(ItemStack::new(11, 1))
        );
    }

    #[test]
    fn configured_loot_count_splits_at_item_max_stack() {
        let blocks = mc_world::BlockRegistry::from_report(&[
            BlockReport {
                id: mc_data::Identifier::parse("minecraft:air").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 0,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: mc_data::Identifier::parse("minecraft:dirt").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 1,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
        ])
        .unwrap();
        let pearl = mc_data::Identifier::parse("minecraft:ender_pearl").unwrap();
        let items = ItemRegistry::from_report(&[ItemReport {
            id: pearl.clone(),
            protocol_id: 10,
        }]);
        let facts = ItemFactsTable::from_entries([(
            pearl.clone(),
            mc_data::item_components::ItemFacts {
                max_stack_size: Some(16),
                ..Default::default()
            },
        )]);
        let configured = mc_data::loot::LootTables::from_drop_maps(
            BTreeMap::from([(
                mc_data::Identifier::parse("minecraft:cow").unwrap(),
                mc_data::loot::LootDrop {
                    item: pearl.clone(),
                    count: mc_data::loot::LootCount::Fixed(33),
                },
            )]),
            BTreeMap::from([(
                mc_data::Identifier::parse("minecraft:dirt").unwrap(),
                mc_data::loot::LootDrop {
                    item: pearl,
                    count: mc_data::loot::LootCount::Fixed(33),
                },
            )]),
        );
        let expected = vec![
            ItemStack::new(10, 16),
            ItemStack::new(10, 16),
            ItemStack::new(10, 1),
        ];

        assert_eq!(
            block_drop_stacks_with_facts_from(
                &configured,
                &items,
                &facts,
                &blocks,
                BlockStateId(1),
            ),
            expected
        );
        assert_eq!(
            mob_drop_stacks_from(&configured, &items, &facts, "minecraft:cow"),
            expected
        );
    }
}
