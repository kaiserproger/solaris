use std::collections::HashSet;
use std::sync::Arc;

use mc_data::items::ItemRegistry;
use mc_data::tags::TagsData;
use mc_world::{
    BlockRegistry, BlockStateId, ChunkPos, FurnaceBlockEntity, FurnaceSlot, SECTION_DIM,
    ScheduledBlockTick, WorldStorage,
};
use tracing::warn;

use super::campfire::{
    CampfireCookingState, campfire_block_entity_id, campfire_block_entity_persistent_bytes,
    campfire_recipe_result_stack, is_campfire_block,
};
use super::containers::{
    ChestView, FurnaceKind, adjacent_chest_positions, chest_slot_stacks, decrement_furnace_slot,
    find_campfire_recipe_in, find_cooking_recipe_for_item, furnace_kind_for_block_id,
    furnace_slot_to_stack, is_fuel_item_id,
};
use super::random_ticks::next_leaf_distance_state;
use super::session::SessionRegistry;
use super::toggles::extend_adjacent_power_target_edits;
use super::{
    BlockEdit, BlockPlanningRead, ItemStack, SnapshotPlanningWorld, SnapshotReadPrecondition,
    adjacent_block_positions, block_state_property, sibling_state_with_bool_property,
};

#[derive(Debug, Default)]
pub(super) struct ScheduledBlockTickPlan {
    pub(super) edits: Vec<BlockEdit>,
    pub(super) preconditions: Vec<SnapshotReadPrecondition>,
}

pub(super) fn scheduled_block_planning_chunks(ticks: &[ScheduledBlockTick]) -> Vec<ChunkPos> {
    let mut positions = HashSet::new();
    for tick in ticks {
        let centre = ChunkPos {
            x: tick.pos.x.div_euclid(SECTION_DIM as i32),
            z: tick.pos.z.div_euclid(SECTION_DIM as i32),
        };
        for dz in -1..=1 {
            for dx in -1..=1 {
                let (Some(x), Some(z)) = (centre.x.checked_add(dx), centre.z.checked_add(dz))
                else {
                    continue;
                };
                positions.insert(ChunkPos { x, z });
            }
        }
    }
    let mut positions = positions.into_iter().collect::<Vec<_>>();
    positions.sort_unstable_by_key(|position| (position.x, position.z));
    positions
}

pub(super) fn plan_scheduled_block_tick_edits(
    blocks: &BlockRegistry,
    snapshot: &mc_world::WorldReadSnapshot,
    ticks: &[ScheduledBlockTick],
) -> Option<ScheduledBlockTickPlan> {
    let mut world = SnapshotPlanningWorld::new(snapshot);
    let mut plan = ScheduledBlockTickPlan::default();
    for tick in ticks {
        let Some(state_id) = world.get_cached_block(tick.pos) else {
            continue;
        };
        let Some(state) = blocks.by_id(state_id) else {
            continue;
        };
        if state.block.id != tick.block {
            continue;
        }
        if matches!(state.block.id.path(), "hopper" | "comparator") {
            return None;
        }
        let Some(edits) = scheduled_simple_block_tick_edits(blocks, &world, tick.pos, state_id)
        else {
            continue;
        };
        if edits
            .iter()
            .any(|edit| world.get_cached_block(edit.pos).is_none())
        {
            continue;
        }
        for edit in edits {
            if world.apply(edit) {
                plan.edits.push(edit);
            }
        }
    }
    plan.preconditions = world.preconditions();
    Some(plan)
}

pub(super) fn scheduled_block_tick_edits(
    blocks: &BlockRegistry,
    storage: &mut WorldStorage,
    pos: mc_world::BlockPos,
    state_id: BlockStateId,
) -> Option<Vec<BlockEdit>> {
    let state = blocks.by_id(state_id)?;
    if state.block.id.path() == "comparator" {
        return scheduled_comparator_tick_edits(blocks, storage, pos, state);
    }
    scheduled_simple_block_tick_edits(blocks, storage, pos, state_id)
}

pub(super) fn scheduled_simple_block_tick_edits(
    blocks: &BlockRegistry,
    storage: &impl BlockPlanningRead,
    pos: mc_world::BlockPos,
    state_id: BlockStateId,
) -> Option<Vec<BlockEdit>> {
    let state = blocks.by_id(state_id)?;
    if state.block.id.path().ends_with("_leaves") {
        return Some(
            next_leaf_distance_state(blocks, storage, pos, state_id)
                .map(|new_state| vec![BlockEdit { pos, new_state }])
                .unwrap_or_default(),
        );
    }
    if !state.block.id.path().ends_with("_button")
        || block_state_property(state, "powered")? != "true"
    {
        return None;
    }
    let mut edits = vec![BlockEdit {
        pos,
        new_state: sibling_state_with_bool_property(blocks, state, "powered", false)?,
    }];
    extend_adjacent_power_target_edits(blocks, storage, pos, false, &mut edits);
    Some(edits)
}

fn scheduled_comparator_tick_edits(
    blocks: &BlockRegistry,
    storage: &mut WorldStorage,
    pos: mc_world::BlockPos,
    state: &mc_world::BlockState,
) -> Option<Vec<BlockEdit>> {
    let input_signal = comparator_input_signal(blocks, storage, pos, state);
    let new_state = sibling_state_with_bool_property(blocks, state, "powered", input_signal > 0)?;
    if new_state == state.id {
        return Some(Vec::new());
    }
    Some(vec![BlockEdit { pos, new_state }])
}

fn comparator_input_signal(
    blocks: &BlockRegistry,
    storage: &mut WorldStorage,
    pos: mc_world::BlockPos,
    state: &mc_world::BlockState,
) -> i32 {
    let Some(facing) = block_state_property(state, "facing") else {
        return 0;
    };
    let Some(target_pos) = hopper_facing_target(pos, facing) else {
        return 0;
    };
    container_redstone_signal_at(blocks, storage, target_pos)
}

pub(super) fn container_redstone_signal_at(
    blocks: &BlockRegistry,
    storage: &mut WorldStorage,
    pos: mc_world::BlockPos,
) -> i32 {
    if let Some(positions) = cached_storage_chest_like_positions(blocks, storage, pos)
        && let Some(view) = load_hopper_chest_view(storage, &positions)
    {
        let slot_count = view.chests.iter().map(|chest| chest.slots.len()).sum();
        return container_redstone_signal_from_slots(
            view.chests.iter().flat_map(|chest| chest.slots.iter()),
            slot_count,
        );
    }

    if cached_furnace_kind(blocks, storage, pos).is_some()
        && let Ok(Some(furnace)) = storage.furnace_block_entity(pos)
    {
        return container_redstone_signal_from_slots(furnace.slots.iter(), furnace.slots.len());
    }

    let is_hopper = storage
        .get_cached_block(pos)
        .and_then(|state_id| blocks.by_id(state_id))
        .is_some_and(|state| state.block.id.path() == "hopper");
    if is_hopper && let Ok(Some(hopper)) = storage.hopper_block_entity(pos) {
        return container_redstone_signal_from_slots(hopper.slots.iter(), hopper.slots.len());
    }

    0
}

fn container_redstone_signal_from_slots<'a>(
    slots: impl IntoIterator<Item = &'a FurnaceSlot>,
    slot_count: usize,
) -> i32 {
    if slot_count == 0 {
        return 0;
    }
    let total_percent = slots.into_iter().fold(0.0f32, |acc, slot| {
        if slot.is_empty() {
            acc
        } else {
            acc + slot.count as f32 / HOPPER_TRANSFER_MAX_STACK as f32
        }
    }) / slot_count as f32;
    if total_percent <= 0.0 {
        0
    } else {
        (total_percent * 14.0).floor() as i32 + 1
    }
}

pub(super) struct HopperTransferContext<'a> {
    pub(super) blocks: &'a BlockRegistry,
    pub(super) items: &'a ItemRegistry,
    pub(super) tags: &'a TagsData,
    pub(super) recipes: &'a [mc_data::recipes::Recipe],
    pub(super) sessions: &'a SessionRegistry,
}

pub(super) struct ResidentPlannedHopperTransfer {
    pub(super) plan: mc_world::ResidentHopperTransferPlan,
    pub(super) result: HopperTickResult,
}

pub(super) fn resident_hopper_cooldown_plan(
    blocks: &BlockRegistry,
    snapshot: &mc_world::WorldReadSnapshot,
    tick: &ScheduledBlockTick,
    world_tick: u64,
) -> Option<mc_world::ResidentHopperTransferPlan> {
    let chunk_position = ChunkPos {
        x: tick.pos.x.div_euclid(SECTION_DIM as i32),
        z: tick.pos.z.div_euclid(SECTION_DIM as i32),
    };
    let chunk = snapshot.chunk(chunk_position)?;
    let local_x = tick.pos.x.rem_euclid(SECTION_DIM as i32) as u8;
    let local_z = tick.pos.z.rem_euclid(SECTION_DIM as i32) as u8;
    let expected_state = chunk.get_block(local_x, tick.pos.y, local_z)?;
    let state = blocks.by_id(expected_state)?;
    if state.block.id != tick.block || state.block.id.path() != "hopper" {
        return None;
    }
    let expected = chunk.hoppers.get(&tick.pos)?.clone();
    if expected.transfer_cooldown <= 1 {
        return None;
    }
    let mut updated = expected.clone();
    updated.transfer_cooldown -= 1;
    Some(mc_world::ResidentHopperTransferPlan {
        expected_states: vec![(tick.pos, expected_state)],
        hoppers: vec![mc_world::ResidentBlockEntityChange {
            position: tick.pos,
            expected,
            updated,
        }],
        chests: Vec::new(),
        furnaces: Vec::new(),
        scheduled_block_ticks: vec![ScheduledBlockTick::new(
            tick.pos,
            tick.block.clone(),
            world_tick.saturating_add(HOPPER_TICK_DELAY_TICKS),
            0,
        )],
    })
}

pub(super) fn plan_resident_hopper_transfer(
    context: &HopperTransferContext<'_>,
    blocks: &Arc<BlockRegistry>,
    snapshot: &mc_world::WorldReadSnapshot,
    planning_chunks: &[ChunkPos],
    tick: &ScheduledBlockTick,
    world_tick: u64,
) -> Option<ResidentPlannedHopperTransfer> {
    let state_id = snapshot.get_cached_block(tick.pos)?;
    let state = blocks.by_id(state_id)?;
    if state.block.id != tick.block || state.block.id.path() != "hopper" {
        return None;
    }
    let facing = block_state_property(state, "facing")?;
    let target = hopper_facing_target(tick.pos, facing)?;
    if snapshot
        .get_cached_block(target)
        .is_some_and(|target_state| is_campfire_block(blocks, target_state))
    {
        return None;
    }

    let mut staged = WorldStorage::in_memory(Arc::clone(blocks));
    for &position in planning_chunks {
        let Some(chunk) = snapshot.chunk(position) else {
            continue;
        };
        staged
            .insert_generated_chunk(position, (*chunk).clone())
            .ok()?;
    }
    let before_hopper = snapshot
        .chunk(ChunkPos {
            x: tick.pos.x.div_euclid(SECTION_DIM as i32),
            z: tick.pos.z.div_euclid(SECTION_DIM as i32),
        })?
        .hoppers
        .get(&tick.pos)?
        .clone();
    let result = scheduled_hopper_transfer(context, &mut staged, tick.pos, state_id)?;
    for update in &result.updates {
        schedule_comparator_ticks_for_hopper_update(
            blocks,
            &mut staged,
            update,
            world_tick.saturating_add(COMPARATOR_TICK_DELAY_TICKS),
        );
    }
    staged
        .schedule_block_tick(ScheduledBlockTick::new(
            tick.pos,
            tick.block.clone(),
            world_tick.saturating_add(HOPPER_TICK_DELAY_TICKS),
            0,
        ))
        .ok()?;

    let after_hopper = staged.hopper_block_entity(tick.pos).ok()??;
    let mut plan = mc_world::ResidentHopperTransferPlan {
        expected_states: vec![(tick.pos, state_id)],
        hoppers: vec![mc_world::ResidentBlockEntityChange {
            position: tick.pos,
            expected: before_hopper,
            updated: after_hopper,
        }],
        chests: Vec::new(),
        furnaces: Vec::new(),
        scheduled_block_ticks: Vec::new(),
    };
    for &chunk_position in planning_chunks {
        let Some(before) = snapshot.chunk(chunk_position) else {
            continue;
        };
        let Some(after) = staged.cached_chunk_snapshot(chunk_position) else {
            continue;
        };
        for position in before
            .chests
            .keys()
            .chain(after.chests.keys())
            .copied()
            .collect::<HashSet<_>>()
        {
            let (Some(expected), Some(updated)) =
                (before.chests.get(&position), after.chests.get(&position))
            else {
                return None;
            };
            if expected != updated {
                plan.expected_states
                    .push((position, snapshot.get_cached_block(position)?));
                plan.chests.push(mc_world::ResidentBlockEntityChange {
                    position,
                    expected: expected.clone(),
                    updated: updated.clone(),
                });
            }
        }
        for position in before
            .furnaces
            .keys()
            .chain(after.furnaces.keys())
            .copied()
            .collect::<HashSet<_>>()
        {
            let (Some(expected), Some(updated)) = (
                before.furnaces.get(&position),
                after.furnaces.get(&position),
            ) else {
                return None;
            };
            if expected != updated {
                plan.expected_states
                    .push((position, snapshot.get_cached_block(position)?));
                plan.furnaces.push(mc_world::ResidentBlockEntityChange {
                    position,
                    expected: expected.clone(),
                    updated: updated.clone(),
                });
            }
        }
        for scheduled in after.scheduled_block_ticks() {
            if !before.scheduled_block_ticks().contains(scheduled) {
                plan.scheduled_block_ticks.push(scheduled.clone());
            }
        }
    }
    plan.expected_states
        .sort_unstable_by_key(|(position, _)| (position.x, position.y, position.z));
    plan.expected_states.dedup_by_key(|(position, _)| *position);
    Some(ResidentPlannedHopperTransfer { plan, result })
}

pub(super) fn scheduled_hopper_transfer(
    context: &HopperTransferContext<'_>,
    storage: &mut WorldStorage,
    pos: mc_world::BlockPos,
    state_id: BlockStateId,
) -> Option<HopperTickResult> {
    let state = context.blocks.by_id(state_id)?;
    if state.block.id.path() != "hopper" {
        return None;
    }
    let facing = block_state_property(state, "facing")?;
    let enabled = block_state_property(state, "enabled").unwrap_or("true") == "true";
    let source_pos = mc_world::BlockPos {
        y: pos.y + 1,
        ..pos
    };
    let target_pos = hopper_facing_target(pos, facing)?;
    let Ok(Some(mut hopper)) = storage.hopper_block_entity(pos) else {
        return None;
    };
    let before_hopper = hopper.clone();
    hopper.transfer_cooldown = hopper.transfer_cooldown.saturating_sub(1);
    let mut updates = Vec::new();
    let mut moved_items = false;

    if hopper.transfer_cooldown <= 0 {
        hopper.transfer_cooldown = 0;
        if enabled {
            let mut moved = false;
            if hopper.slots.iter().any(|slot| !slot.is_empty()) {
                moved |= eject_items_from_hopper(
                    context,
                    storage,
                    &mut hopper,
                    facing,
                    target_pos,
                    &mut updates,
                );
            }
            if !hopper_inventory_full(&hopper) {
                moved |=
                    suck_items_into_hopper(context, storage, &mut hopper, source_pos, &mut updates);
            }
            if moved {
                moved_items = true;
                hopper.transfer_cooldown = HOPPER_TRANSFER_DELAY_TICKS as i32;
            }
        }
    }

    if hopper != before_hopper
        && !storage
            .set_hopper_block_entity(pos, hopper)
            .unwrap_or(false)
    {
        return None;
    }
    Some(HopperTickResult {
        updates,
        moved: moved_items,
    })
}

pub(super) struct HopperTickResult {
    pub(super) updates: Vec<HopperTransferUpdate>,
    pub(super) moved: bool,
}

fn eject_items_from_hopper(
    context: &HopperTransferContext<'_>,
    storage: &mut WorldStorage,
    hopper: &mut mc_world::HopperBlockEntity,
    facing: &str,
    target_pos: mc_world::BlockPos,
    updates: &mut Vec<HopperTransferUpdate>,
) -> bool {
    let Some(hopper_slot) = first_non_empty_hopper_slot(hopper) else {
        return false;
    };
    let moving = FurnaceSlot {
        count: 1,
        item_id: hopper.slots[hopper_slot].item_id,
        damage: hopper.slots[hopper_slot].damage,
        enchantments: hopper.slots[hopper_slot].enchantments.clone(),
    };

    if let Some(target_positions) =
        cached_storage_chest_like_positions(context.blocks, storage, target_pos)
    {
        let Some(mut target) = load_hopper_chest_view(storage, &target_positions) else {
            return false;
        };
        let target_before = target.clone();
        let Some((target_chest, target_slot)) = target_hopper_insert_slot(&target, &moving) else {
            return false;
        };

        decrement_furnace_slot(&mut hopper.slots[hopper_slot]);
        insert_one_into_furnace_slot(&mut target.chests[target_chest].slots[target_slot], moving);

        if !store_hopper_chest_view(storage, &target_positions, &target) {
            return false;
        }

        if target.chests != target_before.chests {
            updates.push(HopperTransferUpdate::Chest {
                position: target_positions[0],
                slots: chest_slot_stacks(&target),
            });
        }
        return true;
    }

    if let Some(furnace_kind) = cached_furnace_kind(context.blocks, storage, target_pos) {
        let Ok(Some(mut target)) = storage.furnace_block_entity(target_pos) else {
            return false;
        };
        let target_before = target.clone();
        if insert_hopper_stack_into_furnace(
            context.items,
            context.tags,
            context.recipes,
            facing,
            furnace_kind,
            &mut target,
            &moving,
        )
        .is_none()
        {
            return false;
        }

        decrement_furnace_slot(&mut hopper.slots[hopper_slot]);
        if !storage
            .set_furnace_block_entity(target_pos, target.clone())
            .unwrap_or(false)
        {
            return false;
        }

        if target != target_before {
            updates.push(HopperTransferUpdate::Furnace {
                position: target_pos,
                slots: furnace_slot_stacks(&target),
            });
        }
        return true;
    }

    if cached_storage_block_is_campfire(context.blocks, storage, target_pos) {
        let Some(cooking) =
            insert_hopper_stack_into_campfire(context, storage, target_pos, &moving)
        else {
            return false;
        };
        decrement_furnace_slot(&mut hopper.slots[hopper_slot]);
        updates.push(HopperTransferUpdate::Campfire {
            position: target_pos,
            cooking: Box::new(cooking),
        });
        return true;
    }

    false
}

fn suck_items_into_hopper(
    context: &HopperTransferContext<'_>,
    storage: &mut WorldStorage,
    hopper: &mut mc_world::HopperBlockEntity,
    source_pos: mc_world::BlockPos,
    updates: &mut Vec<HopperTransferUpdate>,
) -> bool {
    if let Some(source_positions) =
        cached_storage_chest_like_positions(context.blocks, storage, source_pos)
    {
        let Some(mut source) = load_hopper_chest_view(storage, &source_positions) else {
            return false;
        };
        let source_before = source.clone();
        let Some((source_chest, source_slot)) = first_non_empty_hopper_chest_slot(&source) else {
            return false;
        };
        let moving = FurnaceSlot {
            count: 1,
            item_id: source.chests[source_chest].slots[source_slot].item_id,
            damage: source.chests[source_chest].slots[source_slot].damage,
            enchantments: source.chests[source_chest].slots[source_slot]
                .enchantments
                .clone(),
        };
        let Some(hopper_slot) = target_hopper_inventory_slot(hopper, &moving) else {
            return false;
        };

        decrement_furnace_slot(&mut source.chests[source_chest].slots[source_slot]);
        insert_one_into_furnace_slot(&mut hopper.slots[hopper_slot], moving);

        if !store_hopper_chest_view(storage, &source_positions, &source) {
            return false;
        }
        if source.chests != source_before.chests {
            updates.push(HopperTransferUpdate::Chest {
                position: source_positions[0],
                slots: chest_slot_stacks(&source),
            });
        }
        return true;
    }

    if cached_furnace_kind(context.blocks, storage, source_pos).is_some() {
        let Ok(Some(mut source)) = storage.furnace_block_entity(source_pos) else {
            return false;
        };
        let source_before = source.clone();
        if source.slots[2].is_empty() {
            return false;
        }
        let moving = FurnaceSlot {
            count: 1,
            item_id: source.slots[2].item_id,
            damage: source.slots[2].damage,
            enchantments: source.slots[2].enchantments.clone(),
        };
        let Some(hopper_slot) = target_hopper_inventory_slot(hopper, &moving) else {
            return false;
        };

        decrement_furnace_slot(&mut source.slots[2]);
        insert_one_into_furnace_slot(&mut hopper.slots[hopper_slot], moving);

        if !storage
            .set_furnace_block_entity(source_pos, source.clone())
            .unwrap_or(false)
        {
            return false;
        }
        if source != source_before {
            updates.push(HopperTransferUpdate::Furnace {
                position: source_pos,
                slots: furnace_slot_stacks(&source),
            });
        }
        return true;
    }

    false
}

fn hopper_inventory_full(hopper: &mc_world::HopperBlockEntity) -> bool {
    hopper
        .slots
        .iter()
        .all(|slot| !slot.is_empty() && slot.count >= HOPPER_TRANSFER_MAX_STACK)
}

fn first_non_empty_hopper_slot(hopper: &mc_world::HopperBlockEntity) -> Option<usize> {
    hopper.slots.iter().position(|slot| !slot.is_empty())
}

fn target_hopper_inventory_slot(
    hopper: &mc_world::HopperBlockEntity,
    moving: &FurnaceSlot,
) -> Option<usize> {
    hopper.slots.iter().position(|slot| {
        slot.is_empty()
            || (slot.item_id == moving.item_id
                && slot.damage == moving.damage
                && slot.enchantments == moving.enchantments
                && slot.count < HOPPER_TRANSFER_MAX_STACK)
    })
}

fn insert_one_into_furnace_slot(target: &mut FurnaceSlot, moving: FurnaceSlot) {
    if target.is_empty() {
        *target = moving;
    } else {
        target.count += 1;
    }
}

pub(super) enum HopperTransferUpdate {
    Chest {
        position: mc_world::BlockPos,
        slots: Vec<ItemStack>,
    },
    Furnace {
        position: mc_world::BlockPos,
        slots: [ItemStack; 3],
    },
    Campfire {
        position: mc_world::BlockPos,
        cooking: Box<CampfireCookingState>,
    },
}

pub(super) fn schedule_comparator_ticks_for_hopper_update(
    blocks: &BlockRegistry,
    storage: &mut WorldStorage,
    update: &HopperTransferUpdate,
    trigger_tick: u64,
) {
    match update {
        HopperTransferUpdate::Chest { position, .. } => {
            if let Some(positions) = cached_storage_chest_like_positions(blocks, storage, *position)
            {
                for position in positions {
                    schedule_comparator_ticks_for_container(
                        blocks,
                        storage,
                        position,
                        trigger_tick,
                    );
                }
            } else {
                schedule_comparator_ticks_for_container(blocks, storage, *position, trigger_tick);
            }
        }
        HopperTransferUpdate::Furnace { position, .. } => {
            schedule_comparator_ticks_for_container(blocks, storage, *position, trigger_tick);
        }
        HopperTransferUpdate::Campfire { .. } => {}
    }
}

fn schedule_comparator_ticks_for_container(
    blocks: &BlockRegistry,
    storage: &mut WorldStorage,
    target: mc_world::BlockPos,
    trigger_tick: u64,
) {
    for pos in adjacent_block_positions(target) {
        let Some(state_id) = storage.get_cached_block(pos) else {
            continue;
        };
        let Some(state) = blocks.by_id(state_id) else {
            continue;
        };
        if state.block.id.path() != "comparator" {
            continue;
        }
        let Some(facing) = block_state_property(state, "facing") else {
            continue;
        };
        if hopper_facing_target(pos, facing) != Some(target) {
            continue;
        }
        if let Err(err) = storage.schedule_block_tick(ScheduledBlockTick::new(
            pos,
            state.block.id.clone(),
            trigger_tick,
            0,
        )) {
            warn!(error = %err, ?pos, ?target, "comparator refresh tick scheduling failed");
        }
    }
}

fn hopper_facing_target(pos: mc_world::BlockPos, facing: &str) -> Option<mc_world::BlockPos> {
    Some(match facing {
        "down" => mc_world::BlockPos {
            y: pos.y - 1,
            ..pos
        },
        "north" => mc_world::BlockPos {
            z: pos.z - 1,
            ..pos
        },
        "south" => mc_world::BlockPos {
            z: pos.z + 1,
            ..pos
        },
        "west" => mc_world::BlockPos {
            x: pos.x - 1,
            ..pos
        },
        "east" => mc_world::BlockPos {
            x: pos.x + 1,
            ..pos
        },
        _ => return None,
    })
}

fn cached_storage_chest_like_positions(
    blocks: &BlockRegistry,
    storage: &WorldStorage,
    pos: mc_world::BlockPos,
) -> Option<Vec<mc_world::BlockPos>> {
    let state = storage
        .get_cached_block(pos)
        .and_then(|state_id| blocks.by_id(state_id))
        .filter(|state| matches!(state.block.id.path(), "chest" | "barrel"))?;
    if state.block.id.path() == "barrel" {
        return Some(vec![pos]);
    }

    let mut positions = vec![pos];
    for neighbour in adjacent_chest_positions(pos) {
        let is_chest = storage
            .get_cached_block(neighbour)
            .and_then(|state_id| blocks.by_id(state_id))
            .is_some_and(|state| state.block.id.as_str() == "minecraft:chest");
        if is_chest {
            positions.push(neighbour);
            break;
        }
    }
    positions.sort_by_key(|pos| (pos.x, pos.y, pos.z));
    positions.dedup();
    Some(positions)
}

fn cached_furnace_kind(
    blocks: &BlockRegistry,
    storage: &WorldStorage,
    pos: mc_world::BlockPos,
) -> Option<FurnaceKind> {
    storage
        .get_cached_block(pos)
        .and_then(|state_id| blocks.by_id(state_id))
        .and_then(|state| furnace_kind_for_block_id(state.block.id.as_str()))
}

fn cached_storage_block_is_campfire(
    blocks: &BlockRegistry,
    storage: &WorldStorage,
    pos: mc_world::BlockPos,
) -> bool {
    storage
        .get_cached_block(pos)
        .is_some_and(|state_id| is_campfire_block(blocks, state_id))
}

fn load_hopper_chest_view(
    storage: &mut WorldStorage,
    positions: &[mc_world::BlockPos],
) -> Option<ChestView> {
    let mut chests = Vec::with_capacity(positions.len());
    for &position in positions {
        let Ok(Some(chest)) = storage.chest_block_entity(position) else {
            return None;
        };
        chests.push(chest);
    }
    Some(ChestView { chests })
}

fn store_hopper_chest_view(
    storage: &mut WorldStorage,
    positions: &[mc_world::BlockPos],
    view: &ChestView,
) -> bool {
    positions
        .iter()
        .zip(&view.chests)
        .all(|(&position, chest)| {
            storage
                .set_chest_block_entity(position, chest.clone())
                .unwrap_or(false)
        })
}

fn first_non_empty_hopper_chest_slot(view: &ChestView) -> Option<(usize, usize)> {
    view.chests
        .iter()
        .enumerate()
        .find_map(|(chest, block_entity)| {
            block_entity
                .slots
                .iter()
                .position(|slot| !slot.is_empty())
                .map(|slot| (chest, slot))
        })
}

fn target_hopper_insert_slot(target: &ChestView, moving: &FurnaceSlot) -> Option<(usize, usize)> {
    target
        .chests
        .iter()
        .enumerate()
        .find_map(|(chest, block_entity)| {
            block_entity
                .slots
                .iter()
                .position(|slot| {
                    slot.is_empty()
                        || (slot.item_id == moving.item_id
                            && slot.damage == moving.damage
                            && slot.enchantments == moving.enchantments
                            && slot.count < HOPPER_TRANSFER_MAX_STACK)
                })
                .map(|slot| (chest, slot))
        })
}

fn insert_hopper_input_into_furnace(
    items: &ItemRegistry,
    tags: &TagsData,
    recipes: &[mc_data::recipes::Recipe],
    furnace_kind: FurnaceKind,
    furnace: &mut FurnaceBlockEntity,
    moving: &FurnaceSlot,
) -> Option<()> {
    if moving.is_empty()
        || find_cooking_recipe_for_item(recipes, items, tags, furnace_kind, moving.item_id)
            .is_none()
    {
        return None;
    }
    let target = &mut furnace.slots[0];
    if target.is_empty() {
        *target = FurnaceSlot {
            count: 1,
            item_id: moving.item_id,
            damage: moving.damage,
            enchantments: moving.enchantments.clone(),
        };
        Some(())
    } else if target.item_id == moving.item_id
        && target.damage == moving.damage
        && target.enchantments == moving.enchantments
        && target.count < HOPPER_TRANSFER_MAX_STACK
    {
        target.count += 1;
        Some(())
    } else {
        None
    }
}

pub(super) fn insert_hopper_stack_into_campfire(
    context: &HopperTransferContext<'_>,
    storage: &mut WorldStorage,
    position: mc_world::BlockPos,
    moving: &FurnaceSlot,
) -> Option<CampfireCookingState> {
    if moving.is_empty() {
        return None;
    }
    let recipe =
        find_campfire_recipe_in(context.recipes, context.items, context.tags, moving.item_id)?;
    let result = campfire_recipe_result_stack(context.items, &recipe)?;
    let cooking_time = match &recipe.kind {
        mc_data::recipes::RecipeKind::CampfireCooking(smelting) => smelting.cooking_time,
        _ => return None,
    };
    let input = ItemStack {
        count: 1,
        item_id: moving.item_id,
        damage: moving.damage,
        enchantments: moving.enchantments.clone(),
        custom_name: None,
    };
    context.sessions.commit_campfire_cooking_insert(
        position,
        input,
        result,
        cooking_time,
        |cooking| {
            persist_campfire_block_entity_in_storage(
                storage,
                context.blocks,
                context.items,
                position,
                cooking,
            )
        },
    )
}

fn persist_campfire_block_entity_in_storage(
    storage: &mut WorldStorage,
    blocks: &BlockRegistry,
    items: &ItemRegistry,
    position: mc_world::BlockPos,
    cooking: &CampfireCookingState,
) -> bool {
    let block_entity_id = storage
        .get_cached_block(position)
        .and_then(|block_state| campfire_block_entity_id(blocks, block_state));
    let Some(block_entity_id) = block_entity_id else {
        return false;
    };
    let Some(bytes) =
        campfire_block_entity_persistent_bytes(block_entity_id, position, items, cooking)
    else {
        return false;
    };
    if let Err(err) = storage.set_opaque_block_entity(position, bytes) {
        warn!(error = %err, ?position, "campfire block entity save failed");
        return false;
    }
    true
}

fn insert_hopper_stack_into_furnace(
    items: &ItemRegistry,
    tags: &TagsData,
    recipes: &[mc_data::recipes::Recipe],
    facing: &str,
    furnace_kind: FurnaceKind,
    furnace: &mut FurnaceBlockEntity,
    moving: &FurnaceSlot,
) -> Option<()> {
    if facing == "down" {
        return insert_hopper_input_into_furnace(
            items,
            tags,
            recipes,
            furnace_kind,
            furnace,
            moving,
        );
    }
    insert_hopper_fuel_into_furnace(items, furnace, moving)
}

fn insert_hopper_fuel_into_furnace(
    items: &ItemRegistry,
    furnace: &mut FurnaceBlockEntity,
    moving: &FurnaceSlot,
) -> Option<()> {
    if moving.is_empty() || !is_fuel_item_id(items, moving.item_id) {
        return None;
    }
    let target = &mut furnace.slots[1];
    if target.is_empty() {
        *target = FurnaceSlot {
            count: 1,
            item_id: moving.item_id,
            damage: moving.damage,
            enchantments: moving.enchantments.clone(),
        };
        Some(())
    } else if target.item_id == moving.item_id
        && target.damage == moving.damage
        && target.enchantments == moving.enchantments
        && target.count < HOPPER_TRANSFER_MAX_STACK
    {
        target.count += 1;
        Some(())
    } else {
        None
    }
}

pub(super) fn furnace_slot_stacks(furnace: &FurnaceBlockEntity) -> [ItemStack; 3] {
    std::array::from_fn(|slot| furnace_slot_to_stack(&furnace.slots[slot]))
}

pub(super) fn placed_hopper_ticks(
    blocks: &BlockRegistry,
    edits: &[BlockEdit],
    world_tick: u64,
) -> Vec<ScheduledBlockTick> {
    edits
        .iter()
        .filter_map(|edit| {
            let block_state = blocks.by_id(edit.new_state)?;
            (block_state.block.id.path() == "hopper").then(|| {
                ScheduledBlockTick::new(
                    edit.pos,
                    block_state.block.id.clone(),
                    world_tick.saturating_add(HOPPER_TICK_DELAY_TICKS),
                    0,
                )
            })
        })
        .collect()
}

pub(super) fn backfill_loaded_hopper_ticks(
    storage: &mut WorldStorage,
    blocks: &BlockRegistry,
    loaded_chunks: &[(i32, i32)],
    trigger_tick: u64,
) {
    let mut ticks = Vec::new();
    for &(cx, cz) in loaded_chunks {
        let cpos = ChunkPos { x: cx, z: cz };
        let Some(chunk) = storage.cached_chunk(cpos) else {
            continue;
        };
        for pos in chunk.hoppers.keys().copied() {
            let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
            let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
            let Some(state_id) = chunk.get_block(local_x, pos.y, local_z) else {
                continue;
            };
            let Some(state) = blocks.by_id(state_id) else {
                continue;
            };
            if state.block.id.path() != "hopper"
                || chunk
                    .scheduled_block_ticks()
                    .iter()
                    .any(|tick| tick.pos == pos && tick.block == state.block.id)
            {
                continue;
            }
            ticks.push(ScheduledBlockTick::new(
                pos,
                state.block.id.clone(),
                trigger_tick,
                0,
            ));
        }
    }
    for tick in ticks {
        if let Err(err) = storage.schedule_block_tick(tick) {
            warn!(error = %err, "loaded hopper tick backfill failed");
        }
    }
}

pub(super) const HOPPER_TRANSFER_MAX_STACK: i32 = 64;
// 26.1.2 HopperBlockEntity.MOVE_ITEM_SPEED. Same-tick hopper-chain skip rules
// remain outside this foundation.
pub(super) const HOPPER_TRANSFER_DELAY_TICKS: u64 = 8;
pub(super) const HOPPER_TICK_DELAY_TICKS: u64 = 1;
// 26.1.2 ComparatorBlock#getDelay.
pub(super) const COMPARATOR_TICK_DELAY_TICKS: u64 = 2;
