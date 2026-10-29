//! Tick-owned composting. Fill level is a block state; maturation uses the
//! native persisted block scheduler rather than a second composter clock.

use mc_data::{Identifier, ItemStack};
use mc_world::{BlockStateId, ScheduledBlockTick};

use crate::boundary::{BlockPos, ChangeSet, EntityId, Hand};
use crate::tick::{TickState, spawn_item_entity};

const FULL: u8 = 7;
const READY: u8 = 8;
const DELAY: u64 = 20;

/// The old-supported compost table, including the pinned 26.1.2 additions.
fn chance(item: &Identifier) -> Option<u8> {
    if item.namespace() != "minecraft" {
        return None;
    }
    Some(match item.path() {
        "acacia_leaves" | "acacia_sapling" | "azalea_leaves" | "beetroot_seeds"
        | "birch_leaves" | "birch_sapling" | "bush" | "cactus_flower" | "cherry_leaves"
        | "cherry_sapling" | "dark_oak_leaves" | "dark_oak_sapling" | "dried_kelp"
        | "firefly_bush" | "glow_berries" | "hanging_roots" | "jungle_leaves"
        | "jungle_sapling" | "kelp" | "leaf_litter" | "mangrove_leaves" | "mangrove_propagule"
        | "mangrove_roots" | "melon_seeds" | "moss_carpet" | "oak_leaves" | "oak_sapling"
        | "pale_hanging_moss" | "pale_moss_carpet" | "pale_oak_leaves" | "pale_oak_sapling"
        | "pink_petals" | "pitcher_pod" | "pumpkin_seeds" | "seagrass" | "short_dry_grass"
        | "short_grass" | "small_dripleaf" | "spruce_leaves" | "spruce_sapling"
        | "sweet_berries" | "tall_dry_grass" | "torchflower_seeds" | "wheat_seeds"
        | "wildflowers" => 30,
        "cactus"
        | "dried_kelp_block"
        | "flowering_azalea_leaves"
        | "glow_lichen"
        | "melon_slice"
        | "nether_sprouts"
        | "sugar_cane"
        | "tall_grass"
        | "twisting_vines"
        | "vine"
        | "weeping_vines" => 50,
        "allium" | "apple" | "azalea" | "azure_bluet" | "beetroot" | "big_dripleaf"
        | "blue_orchid" | "brown_mushroom" | "carrot" | "carved_pumpkin" | "cocoa_beans"
        | "cornflower" | "crimson_fungus" | "crimson_roots" | "dandelion" | "fern"
        | "large_fern" | "lilac" | "lily_of_the_valley" | "lily_pad" | "melon" | "moss_block"
        | "mushroom_stem" | "nether_wart" | "orange_tulip" | "oxeye_daisy" | "pale_moss_block"
        | "peony" | "pink_tulip" | "poppy" | "potato" | "pumpkin" | "red_mushroom"
        | "red_tulip" | "rose_bush" | "sea_pickle" | "shroomlight" | "spore_blossom"
        | "sunflower" | "warped_fungus" | "warped_roots" | "wheat" | "white_tulip"
        | "wither_rose" => 65,
        "baked_potato"
        | "bread"
        | "brown_mushroom_block"
        | "cookie"
        | "flowering_azalea"
        | "hay_block"
        | "nether_wart_block"
        | "pitcher_plant"
        | "red_mushroom_block"
        | "torchflower"
        | "warped_wart_block" => 85,
        "cake" | "pumpkin_pie" => 100,
        _ => return None,
    })
}

fn level(state: &TickState, pos: BlockPos) -> Option<u8> {
    let block = state
        .registry
        .blocks
        .by_id(BlockStateId(state.world.block_at(pos.x, pos.y, pos.z)?))?;
    if block.block.id.as_str() != "minecraft:composter" {
        return None;
    }
    block
        .properties
        .iter()
        .find(|(key, _)| key == "level")?
        .1
        .parse()
        .ok()
}

fn level_state(state: &TickState, pos: BlockPos, level: u8) -> Option<u32> {
    let current = state
        .registry
        .blocks
        .by_id(BlockStateId(state.world.block_at(pos.x, pos.y, pos.z)?))?;
    // Composter has only the level property. Resolve against the existing
    // registry strings without allocating a property list on each use.
    current.block.states.iter().find_map(|id| {
        let block = state.registry.blocks.by_id(*id)?;
        (block
            .properties
            .iter()
            .find(|(key, _)| key == "level")?
            .1
            .parse::<u8>()
            .ok()?
            == level)
            .then_some(id.0)
    })
}

fn fill_seed(pos: BlockPos, sequence: i32) -> u64 {
    let mut seed = (pos.x as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (pos.y as i64 as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9)
        ^ (pos.z as i64 as u64)
        ^ (sequence as i64 as u64).rotate_left(23);
    seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    seed = (seed ^ (seed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    seed = (seed ^ (seed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    seed ^ (seed >> 31)
}

pub(crate) enum UsePlan {
    Pass,
    Direct,
    Collect {
        empty: u32,
        bone_meal: u32,
    },
    Fill {
        current: u32,
        next: u32,
        maturing: bool,
    },
}

/// The same read-only dispatch is used by native use and the hook preview.
pub(crate) fn plan_use(
    state: &TickState,
    entity: EntityId,
    hand: Hand,
    pos: BlockPos,
    sequence: i32,
) -> UsePlan {
    let Some(current) = level(state, pos) else {
        return UsePlan::Pass;
    };
    let Some(player) = state.players.get(&entity) else {
        return UsePlan::Direct;
    };
    if player.spectator() {
        return UsePlan::Direct;
    }
    let held = &player.inventory.slots[crate::tick::held_slot_index(player, hand)];
    if current == READY {
        return level_state(state, pos, 0)
            .zip(
                state
                    .registry
                    .items
                    .id_of(&Identifier::parse("minecraft:bone_meal").expect("static identifier")),
            )
            .map_or(UsePlan::Direct, |(empty, bone_meal)| UsePlan::Collect {
                empty,
                bone_meal,
            });
    }
    if current >= FULL {
        return UsePlan::Direct;
    }
    if held.is_empty() {
        return UsePlan::Pass;
    }
    let Some(chance) = state.registry.items.name_of(held.item_id).and_then(chance) else {
        return UsePlan::Pass;
    };
    let succeeds = current == 0 || fill_seed(pos, sequence) % 100 < u64::from(chance);
    let Some(next) = level_state(state, pos, current + u8::from(succeeds)) else {
        return UsePlan::Direct;
    };
    UsePlan::Fill {
        current: state
            .world
            .block_at(pos.x, pos.y, pos.z)
            .expect("resident composter"),
        next,
        maturing: succeeds && current + 1 == FULL,
    }
}

pub(crate) fn schedule_fill(state: &mut TickState, pos: BlockPos, maturing: bool) {
    if maturing {
        state.block_ticks.schedule(ScheduledBlockTick::new(
            pos,
            Identifier::parse("minecraft:composter").expect("static identifier"),
            state.tick.saturating_add(DELAY),
            0,
        ));
    }
}

/// Whether this stack can feed a composter (vanilla compostable table).
pub(crate) fn compostable(state: &TickState, stack: &ItemStack) -> bool {
    state
        .registry
        .items
        .name_of(stack.item_id)
        .and_then(|id| chance(&Identifier::parse(id.as_str()).ok()?))
        .is_some()
}

/// Ready level constant for automation extraction.
pub(crate) const READY_LEVEL: u8 = READY;

/// Current level for automation gating (None when not a composter).
pub(crate) fn output_level(state: &TickState, pos: BlockPos) -> Option<u8> {
    level(state, pos)
}

/// Empty a harvested composter back to level 0.
pub(crate) fn empty_level(state: &mut TickState, pos: BlockPos) {
    if let Some(empty) = level_state(state, pos, 0) {
        stage_level(state, pos, empty);
    }
}

/// Automation feed: advance the composter level by one compostable layer and
/// schedule maturation. Returns the new level, or None when the composter is
/// full/ready (hopper must retain its item).
pub(crate) fn feed_level(state: &mut TickState, pos: BlockPos) -> Option<u8> {
    let current = level(state, pos)?;
    if current >= FULL {
        return None;
    }
    // Automation always advances one guaranteed layer per item (vanilla
    // hopper-fed composters do not roll the manual chance per item here;
    // the manual path keeps its own chance roll).
    let next = level_state(state, pos, current + 1)?;
    stage_level(state, pos, next);
    schedule_fill(state, pos, current + 1 == FULL);
    Some(current + 1)
}

/// Commits a use serially on the tick owner. Returning true prevents ordinary
/// placement from also debiting the same held stack. Every composting attempt
/// consumes its input, including failed chance rolls; full composters do not.
pub(crate) fn use_on(
    state: &mut TickState,
    entity: EntityId,
    hand: Hand,
    pos: BlockPos,
    sequence: i32,
    changes: &mut ChangeSet,
) -> bool {
    match plan_use(state, entity, hand, pos, sequence) {
        UsePlan::Pass => return false,
        UsePlan::Direct => {}
        UsePlan::Collect { empty, bone_meal } => {
            stage_level(state, pos, empty);
            if crate::command_rules::boolean(state, crate::command_rules::BoolRule::BLOCK_DROPS) {
                let drop = spawn_item_entity(
                    state,
                    [pos.x as f64 + 0.5, pos.y as f64 + 1.01, pos.z as f64 + 0.5],
                    ItemStack::new(bone_meal, 1),
                );
                changes.entities_spawned.push(drop);
            }
        }
        UsePlan::Fill { next, maturing, .. } => {
            let connection = state.players[&entity].connection;
            if crate::tick::held_debit_below_floor(state, entity, hand, 1) {
                return true;
            }
            stage_level(state, pos, next);
            schedule_fill(state, pos, maturing);
            let effects = state
                .players
                .get_mut(&entity)
                .expect("player exists")
                .consume_held(hand, 1);
            for change in effects.changes {
                state.push_player(connection, change);
            }
        }
    }
    true
}

/// Serial uses see the new level immediately. The final cell value also rides
/// the canonical block phase so neighbors, comparators and WAL see one write.
fn stage_level(state: &mut TickState, pos: BlockPos, next: u32) {
    state.world.set_block(pos.x, pos.y, pos.z, next);
    if let Some((_, value)) = state
        .merge_block_writes
        .iter_mut()
        .find(|(cell, _)| *cell == pos)
    {
        *value = next;
    } else {
        state.merge_block_writes.push((pos, next));
    }
}

/// Called only by the native generic scheduler; stale ticks cannot resurrect
/// a broken/replaced composter or produce a second output.
pub(crate) fn mature(
    state: &mut TickState,
    pos: BlockPos,
    writes: &mut Vec<(BlockPos, u32)>,
) -> bool {
    if level(state, pos) != Some(FULL) {
        return false;
    }
    let Some(ready) = level_state(state, pos, READY) else {
        return false;
    };
    state.world.set_block(pos.x, pos.y, pos.z, ready);
    writes.push((pos, ready));
    true
}

#[cfg(test)]
#[path = "composter_tests.rs"]
pub(crate) mod tests;
