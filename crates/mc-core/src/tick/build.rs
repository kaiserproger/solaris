//! Client build planning: placement, break, bucket and support checks.
//! Moved verbatim from `tick.rs`.

use super::*;

/// Whether a placement may overwrite the cell: air, fluids and the vanilla
/// `#minecraft:replaceable` blocks (grass, snow layers, vines, ...); never a
/// torch, sign or flower.
pub(crate) fn replaceable(state: &TickState, current: u32) -> bool {
    if current == state.registry.states.air
        || matches!(
            state.registry.materials.classify(current),
            mc_physics::BlockMaterial::Water | mc_physics::BlockMaterial::Lava
        )
    {
        return true;
    }
    state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(current))
        .is_some_and(|block| {
            state.registry.tags.contains_raw_id(
                "minecraft:block",
                "minecraft:replaceable",
                block.block.raw_id,
            )
        })
}

/// Vanilla `updateShape` -> `canSurvive`: around every written cell, an
/// attached block (torch, flower, sign, door, ladder, button) whose support
/// is no longer solid becomes air and drops its loot; a popped door or
/// double plant takes its other half along. Cascades through the pops.
pub(crate) fn pop_unsupported(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &mut Vec<EntityId>,
) {
    let air = state.registry.states.air;
    let mut frontier: Vec<BlockPos> = writes.iter().map(|(pos, _)| *pos).collect();
    while let Some(changed) = frontier.pop() {
        let neighbours = [
            [0, 1, 0],
            [0, -1, 0],
            [1, 0, 0],
            [-1, 0, 0],
            [0, 0, 1],
            [0, 0, -1],
        ]
        .map(|step| BlockPos {
            x: changed.x + step[0],
            y: changed.y + step[1],
            z: changed.z + step[2],
        });
        for pos in neighbours {
            let Some(current) = state.world.block_at(pos.x, pos.y, pos.z) else {
                continue;
            };
            if current == air {
                continue;
            }
            if !crate::placement::support_changed(
                &state.registry.blocks,
                &state.registry.tags,
                pos,
                current,
                changed,
            ) || crate::placement::supported(
                &state.registry.blocks,
                &state.registry.tags,
                pos,
                current,
                &|pos| state.world.block_at(pos.x, pos.y, pos.z),
            ) {
                continue;
            }
            let mut gone = vec![pos];
            if let Some((other, _)) =
                crate::placement::other_half(&state.registry.blocks, pos, current)
                && let Some(other_state) = state.world.block_at(other.x, other.y, other.z)
                && crate::placement::paired(
                    &state.registry.blocks,
                    pos,
                    current,
                    other,
                    other_state,
                )
            {
                gone.push(other);
            }
            if !state.pop_block_loot(pos, current, spawned) {
                continue;
            }
            for cell in gone {
                let old = state
                    .world
                    .block_at(cell.x, cell.y, cell.z)
                    .unwrap_or(current);
                let new = crate::placement::break_state(
                    &state.registry.blocks,
                    old,
                    air,
                    state.registry.states.water,
                );
                state.world.set_block(cell.x, cell.y, cell.z, new);
                writes.retain(|(written, _)| *written != cell);
                writes.push((cell, new));
                frontier.push(cell);
            }
        }
    }
}

/// A `UseItem` with a bucket in `hand`: the fill/empty intent, if the
/// look ray finds a source to take or a cell to pour into.
pub(super) fn bucket_intent(
    state: &mut TickState,
    entity: EntityId,
    slot: EntitySlot,
    hand: crate::boundary::Hand,
    yaw: f32,
    pitch: f32,
) -> Option<Intent> {
    let mut intent = plan_bucket_intent(state, entity, slot, hand, yaw, pitch)?;
    intent.seq = state.next_seq(entity);
    Some(intent)
}

pub(crate) fn plan_bucket_intent(
    state: &TickState,
    entity: EntityId,
    slot: EntitySlot,
    hand: crate::boundary::Hand,
    yaw: f32,
    pitch: f32,
) -> Option<Intent> {
    plan_bucket_intent_at(state, entity, slot, hand, yaw, pitch, None)
}

pub(crate) fn bucket_eye(state: &TickState, slot: EntitySlot) -> [f64; 3] {
    let pos = state.entities.pos(slot);
    [
        pos[0],
        pos[1] + player_eye_height(state.entities.row(slot).appearance.pose),
        pos[2],
    ]
}

pub(super) fn plan_bucket_intent_at(
    state: &TickState,
    entity: EntityId,
    slot: EntitySlot,
    hand: Hand,
    yaw: f32,
    pitch: f32,
    eye: Option<[f64; 3]>,
) -> Option<Intent> {
    use crate::fluids::Fluid;
    let item = |name: &str| {
        mc_data::Identifier::parse(name)
            .ok()
            .and_then(|id| state.registry.items.id_of(&id))
    };
    let (empty, water, lava) = (
        item("minecraft:bucket")?,
        item("minecraft:water_bucket")?,
        item("minecraft:lava_bucket")?,
    );
    let player = state.players.get(&entity)?;
    if player.spectator() {
        return None;
    }
    let held_slot = held_slot_index(player, hand);
    let held = player.inventory.slots.get(held_slot)?;
    let carried = match held.item_id {
        id if held.is_empty() || ![empty, water, lava].contains(&id) => return None,
        id if id == water => Some(Fluid::Water),
        id if id == lava => Some(Fluid::Lava),
        _ => None,
    };
    let eye = eye.unwrap_or_else(|| bucket_eye(state, slot));
    let (target, new) = crate::fluids::bucket_target(state, eye, yaw, pitch, carried)?;
    let current = state.world.block_at(target.x, target.y, target.z)?;
    let result = match carried {
        Some(_) => empty,
        None => match crate::fluids::fluid_at(&state.registry.states, current) {
            Some((Fluid::Lava, _)) => lava,
            _ => water,
        },
    };
    let creative = player.creative();
    let single = held.count == 1;
    Some(Intent {
        actor: entity,
        seq: 0,
        kind: IntentKind::Bucket {
            pos: target,
            give: (!creative && !single).then_some(result),
        },
        footprint: Footprint {
            block_expect: vec![(target, current)],
            block_write: if carried == Some(Fluid::Water) && state.dimension.water_evaporates() {
                Vec::new()
            } else {
                vec![(target, new)]
            },
            inv: vec![InvClaim {
                owner: entity,
                slot: u16::try_from(held_slot).unwrap_or(u16::MAX),
                expect_item: held.item_id,
                expect_count: held.count,
                take: i32::from(!creative),
                put_item: (!creative && single).then_some(result),
                put_count: 1,
            }],
            ..Default::default()
        },
    })
}

pub(super) fn plan_break(
    state: &TickState,
    entity: EntityId,
    position: BlockPos,
    current: u32,
) -> Intent {
    let air = state.registry.states.air;
    let mut block_expect = vec![(position, current)];
    let mut block_write = vec![(
        position,
        crate::placement::break_state(
            &state.registry.blocks,
            current,
            air,
            state.registry.states.water,
        ),
    )];
    let is_ice = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(current))
        .is_some_and(|block| block.block.id.as_str() == "minecraft:ice");
    let melts = state.players.get(&entity).is_some_and(|player| {
        let held = &player.inventory.slots[held_slot_index(player, Hand::Main)];
        !player.creative()
            && !held.enchantments.iter().any(|enchantment| {
                enchantment.id.as_str() == "minecraft:silk_touch" && enchantment.level > 0
            })
    });
    if is_ice && melts && !state.dimension.water_evaporates() {
        let below = BlockPos {
            y: position.y - 1,
            ..position
        };
        if let Some(support) = state.world.block_at(below.x, below.y, below.z) {
            block_expect.push((below, support));
            let tagged = state
                .registry
                .blocks
                .by_id(mc_world::BlockStateId(support))
                .is_some_and(|block| {
                    state.registry.tags.contains_raw_id(
                        "minecraft:block",
                        "minecraft:ice_melts_when_destroyed_above",
                        block.block.raw_id,
                    )
                });
            if tagged || crate::fluids::fluid_at(&state.registry.states, support).is_some() {
                block_write[0].1 = state.registry.states.water;
            }
        }
    }
    let mut broken = position;
    if let Some((other, primary)) =
        crate::placement::other_half(&state.registry.blocks, position, current)
        && let Some(other_state) = state.world.block_at(other.x, other.y, other.z)
        && crate::placement::paired(
            &state.registry.blocks,
            position,
            current,
            other,
            other_state,
        )
    {
        block_expect.push((other, other_state));
        block_write.push((
            other,
            crate::placement::break_state(
                &state.registry.blocks,
                other_state,
                air,
                state.registry.states.water,
            ),
        ));
        broken = primary;
    }
    if let Some((other, other_state, single)) = crate::chests::break_partner(
        &state.registry.blocks,
        position,
        current,
        &|pos: BlockPos| state.world.block_at(pos.x, pos.y, pos.z),
    ) {
        block_expect.push((other, other_state));
        block_write.push((other, single));
    }
    Intent {
        actor: entity,
        seq: 0,
        kind: IntentKind::BreakBlock { pos: broken },
        footprint: Footprint {
            block_expect,
            block_write,
            ..Default::default()
        },
    }
}

pub(super) fn plan_placement(
    state: &TickState,
    entity: EntityId,
    hand: crate::boundary::Hand,
    position: BlockPos,
    face: u8,
    cursor: [f32; 3],
    held: &mc_data::ItemStack,
) -> Option<Intent> {
    let click = placement_click(state, entity, position, face, cursor)?;
    plan_placement_click(state, entity, hand, held, click)
}

pub(crate) fn placement_click(
    state: &TickState,
    entity: EntityId,
    position: BlockPos,
    face: u8,
    cursor: [f32; 3],
) -> Option<crate::placement::Click> {
    let slot = state.entities.slot_of(entity)?;
    let [yaw, pitch, _] = state.entities.rot(slot);
    Some(crate::placement::Click {
        clicked: position,
        face: crate::placement::Face::from_code(face),
        cursor,
        yaw,
        pitch,
        sneaking: state.entities.row(slot).appearance.shared_flags & SNEAKING_FLAG != 0,
    })
}

pub(super) fn plan_placement_click(
    state: &TickState,
    entity: EntityId,
    hand: Hand,
    held: &mc_data::ItemStack,
    click: crate::placement::Click,
) -> Option<Intent> {
    let item_block = block_for_item(state, held)?;
    // A reserved carrier block is only ever placed by its own custom item.
    if state.registry.carrier_blocks.reserves(&item_block.id)
        && !state
            .registry
            .item_facts
            .custom_for_stack(held, &state.registry.items)
            .is_some_and(|custom| state.registry.carrier_blocks.is_block_item(&custom.id))
    {
        return None;
    }
    if crate::command_blocks::is_block(item_block.id.path())
        && !crate::command_blocks::can_place(state, entity)
    {
        return None;
    }
    let dependencies = std::cell::RefCell::new(Vec::new());
    let block_at = |pos: BlockPos| {
        let current = state.world.block_at(pos.x, pos.y, pos.z)?;
        let mut dependencies = dependencies.borrow_mut();
        if !dependencies.iter().any(|(at, _)| *at == pos) {
            dependencies.push((pos, current));
        }
        Some(current)
    };
    let can_replace = |current: u32| replaceable(state, current);
    let reads = crate::placement::Reads {
        block_at: &block_at,
        replaceable: &can_replace,
        water_source: state.registry.states.water,
    };
    let cells = crate::placement::place(&state.registry.blocks, item_block, &click, &reads)?;
    if !crate::farming::can_plant(state, cells[0].1, cells[0].0)
        || !crate::farming::can_stand(state, cells[0].1, cells[0].0)
        || !crate::placement::supported(
            &state.registry.blocks,
            &state.registry.tags,
            cells[0].0,
            cells[0].1,
            &block_at,
        )
        || cells.iter().any(|(pos, placed)| {
            state.registry.materials.classify(*placed) == mc_physics::BlockMaterial::Solid
                && obstructed(state, *pos)
        })
    {
        return None;
    }
    for (pos, _) in &cells {
        block_at(*pos)?;
    }
    let block_expect = dependencies.into_inner();
    let (target, place_state) = cells[0];
    let player = state.players.get(&entity)?;
    let slot = held_slot_index(player, hand);
    Some(Intent {
        actor: entity,
        seq: 0,
        kind: IntentKind::PlaceBlock {
            pos: target,
            state: place_state,
        },
        footprint: Footprint {
            block_expect,
            block_write: cells,
            inv: vec![InvClaim {
                owner: entity,
                slot: u16::try_from(slot).unwrap_or(u16::MAX),
                expect_item: held.item_id,
                expect_count: held.count,
                take: i32::from(!player.creative()),
                put_item: None,
                put_count: 0,
            }],
            ..Default::default()
        },
    })
}

/// Native early refusals precede the hook, just as they precede native use.
/// The hook-enabled gateway returns a per-viewer correction rather than
/// pretending that a prediction repair is a world/WAL mutation.
pub(crate) fn client_build_refused(
    state: &TickState,
    entity: EntityId,
    input: &PlayerInput,
) -> bool {
    match input {
        PlayerInput::Dig {
            position, action, ..
        } if *action != DigAction::Cancel => {
            !within_block_reach(state, entity, *position)
                || crate::spawn_protection::protected(state, entity, *position)
                || !crate::plugin_player::allows_block(state, entity, *position)
                || crate::plugin_inventories::has_reservations_for_block(state, *position)
                || (*action == DigAction::Finish
                    && state
                        .world
                        .block_at(position.x, position.y, position.z)
                        .is_none_or(|current| {
                            !mining::allows(state, entity, *position, current, *action)
                                && !mining::can_finish(state, entity, *position, current)
                        }))
        }
        PlayerInput::UseItemOn { position, face, .. } => {
            if crate::spawn_protection::protected(state, entity, *position) {
                return true;
            }
            // A zone's public-use flag lets a non-member flip a door-like block;
            // the click then never places, so only that block is checked.
            if crate::plugin_player::public_use_override(state, entity, *position) {
                return !within_block_reach(state, entity, *position);
            }
            let step = crate::placement::Face::from_code(*face).offset();
            let adjacent = BlockPos {
                x: position.x.saturating_add(step[0]),
                y: position.y.saturating_add(step[1]),
                z: position.z.saturating_add(step[2]),
            };
            !within_block_reach(state, entity, *position)
                || crate::spawn_protection::protected(state, entity, adjacent)
                || !crate::plugin_player::allows_block(state, entity, *position)
                || !crate::plugin_player::allows_block(state, entity, adjacent)
        }
        _ => false,
    }
}

pub(super) fn use_intercepts_build(
    state: &TickState,
    entity: EntityId,
    hand: Hand,
    pos: BlockPos,
    face: u8,
    held: &mc_data::ItemStack,
) -> bool {
    if crate::leash::would_use_fence(state, entity, pos)
        || crate::shelf::intercepts_placement(state, hand, pos, face)
        || crate::campfire::intercepts_placement(state, pos, held)
    {
        return true;
    }
    let Some(current) = state.world.block_at(pos.x, pos.y, pos.z) else {
        return false;
    };
    let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(current)) else {
        return false;
    };
    let path = block.block.id.path();
    matches!(
        path,
        "jukebox"
            | "vault"
            | "respawn_anchor"
            | "dragon_egg"
            | "decorated_pot"
            | "chiseled_bookshelf"
    ) || (path.ends_with("_bed")
        && (!state.dimension.beds_work()
            || crate::placement::other_half(&state.registry.blocks, pos, current).is_some()))
        || crate::copper::family(path).is_some_and(|(family, _, _)| family == "copper_golem_statue")
}

pub(crate) fn plan_client_build(
    state: &TickState,
    entity: EntityId,
    input: &PlayerInput,
) -> Option<Intent> {
    plan_client_build_at(state, entity, input, state.tick, None)
        .filter(|intent| !intent.footprint.block_write.is_empty())
}

/// Hook planning may approve a started dig before its delayed completion. The
/// normal commit planner still requires sufficient authoritative progress.
pub(crate) fn plan_client_build_preview(
    state: &TickState,
    entity: EntityId,
    input: &PlayerInput,
) -> Option<Intent> {
    if let PlayerInput::Dig {
        position,
        action: DigAction::Finish,
        ..
    } = input
    {
        let block = state.world.block_at(position.x, position.y, position.z)?;
        if block != state.registry.states.air && mining::can_finish(state, entity, *position, block)
        {
            return Some(plan_break(state, entity, *position, block))
                .filter(|intent| !intent.footprint.block_write.is_empty());
        }
    }
    plan_client_build(state, entity, input)
}

pub(crate) fn plan_client_build_at(
    state: &TickState,
    entity: EntityId,
    input: &PlayerInput,
    planning_tick: u64,
    pose: Option<crate::build_precommit::PlanningPose>,
) -> Option<Intent> {
    match input {
        PlayerInput::Dig {
            position, action, ..
        } => {
            let current = state.world.block_at(position.x, position.y, position.z)?;
            if current == state.registry.states.air || !within_block_reach(state, entity, *position)
            {
                return None;
            }
            if !mining::allows(state, entity, *position, current, *action) {
                return None;
            }
            Some(plan_break(state, entity, *position, current))
        }
        PlayerInput::UseItemOn {
            hand,
            position,
            face,
            cursor,
            sequence,
            ..
        } => {
            if !within_block_reach(state, entity, *position) {
                return None;
            }
            let player = state.players.get(&entity)?;
            let held = player.inventory.slots.get(held_slot_index(player, *hand))?;
            if use_intercepts_build(state, entity, *hand, *position, *face, held) {
                return None;
            }
            match crate::composter::plan_use(state, entity, *hand, *position, *sequence) {
                crate::composter::UsePlan::Fill {
                    current,
                    next,
                    maturing,
                } => {
                    return Some(Intent {
                        actor: entity,
                        seq: 0,
                        kind: IntentKind::ComposterFill {
                            pos: *position,
                            maturing,
                        },
                        footprint: Footprint {
                            block_expect: vec![(*position, current)],
                            block_write: vec![(*position, next)],
                            inv: vec![InvClaim {
                                owner: entity,
                                slot: u16::try_from(held_slot_index(player, *hand))
                                    .unwrap_or(u16::MAX),
                                expect_item: held.item_id,
                                expect_count: held.count,
                                take: i32::from(!player.creative()),
                                put_item: None,
                                put_count: 0,
                            }],
                            ..Default::default()
                        },
                    });
                }
                crate::composter::UsePlan::Direct | crate::composter::UsePlan::Collect { .. } => {
                    return None;
                }
                crate::composter::UsePlan::Pass => {}
            }
            if !secondary_use(state, entity)
                && container_menu_for(state, position.x, position.y, position.z).is_some()
            {
                return None;
            }
            let slot = state.entities.slot_of(entity)?;
            let [yaw, _, _] = state.entities.rot(slot);
            if crate::placement::use_block(
                &state.registry.blocks,
                *position,
                yaw,
                &|pos: BlockPos| state.world.block_at(pos.x, pos.y, pos.z),
            )
            .is_some()
            {
                return None;
            }
            if held.is_empty() {
                return None;
            }
            let item = state.registry.items.name_of(held.item_id)?;
            if item.as_str() == "minecraft:bone_meal" {
                return crate::farming::plan_use_intent_at(
                    state,
                    entity,
                    *hand,
                    *position,
                    crate::placement::Face::from_code(*face),
                    held,
                    planning_tick,
                );
            }
            if matches!(
                item.as_str(),
                "minecraft:bucket" | "minecraft:water_bucket" | "minecraft:lava_bucket"
            ) {
                return crate::fluids::plan_use_on(
                    state,
                    entity,
                    *hand,
                    *position,
                    crate::placement::Face::from_code(*face),
                    held,
                );
            }
            let click = match pose {
                Some(crate::build_precommit::PlanningPose::Placement(click)) => Some(click),
                _ => None,
            };
            if crate::signs::intercepts_placement(
                state,
                entity,
                *hand,
                *position,
                click.map(|click| click.sneaking),
            ) {
                return None;
            }
            match click {
                Some(click) => plan_placement_click(state, entity, *hand, held, click),
                None => plan_placement(state, entity, *hand, *position, *face, *cursor, held),
            }
        }
        PlayerInput::UseItem {
            hand, yaw, pitch, ..
        } => {
            let eye = match pose {
                Some(crate::build_precommit::PlanningPose::Ray { eye }) => Some(eye),
                _ => None,
            };
            plan_bucket_intent_at(
                state,
                entity,
                state.entities.slot_of(entity)?,
                *hand,
                *yaw,
                *pitch,
                eye,
            )
        }
        _ => None,
    }
}

/// Held slot index for a hand (mirrors `PlayerState::held_slot`).
pub(crate) fn held_slot_index(
    player: &crate::player::PlayerState,
    hand: crate::boundary::Hand,
) -> usize {
    match hand {
        crate::boundary::Hand::Main => usize::from(player.inventory.selected.min(8)),
        crate::boundary::Hand::Off => crate::inventory::OFF_HAND,
    }
}

/// Admit an actual native held-item consumption, not the enclosing input.
pub(crate) fn held_debit_below_floor(
    state: &TickState,
    entity: EntityId,
    hand: crate::boundary::Hand,
    count: u32,
) -> bool {
    let Some(player) = state.players.get(&entity) else {
        return false;
    };
    if player.creative() {
        return false;
    }
    let slot = held_slot_index(player, hand);
    let held = &player.inventory.slots[slot];
    let debit = count.min(held.count.max(0) as u32);
    crate::plugin_inventories::below_player_debit_floors(state, entity, &[(slot, debit)], None)
}

/// The block the held stack places. Items with no block form (food, tools)
/// return `None`.
pub(super) fn block_for_item<'a>(
    state: &'a TickState,
    held: &mc_data::ItemStack,
) -> Option<&'a mc_world::Block> {
    let name = state.registry.items.name_of(held.item_id)?;
    crate::farming::placed_block(&state.registry.blocks, name)
}

/// Whether a living body (player or mob) overlaps the cell: vanilla
/// `isUnobstructed` refuses a solid block there.
pub(crate) fn obstructed(state: &TickState, pos: BlockPos) -> bool {
    let (x, y, z) = (f64::from(pos.x), f64::from(pos.y), f64::from(pos.z));
    let intersects = |slot| {
        let body = state.entities.body(slot);
        if !body.alive || matches!(body.kind, EntityKind::Item { .. }) {
            return false;
        }
        let at = state.entities.pos(slot);
        at[0] + body.half_width > x
            && at[0] - body.half_width < x + 1.0
            && at[1] + body.height > y
            && at[1] < y + 1.0
            && at[2] + body.half_width > z
            && at[2] - body.half_width < z + 1.0
    };
    // The live mutation-owned chunk indices include bodies moved or spawned by
    // earlier inputs in this batch; a published view or spawn census does not.
    let extent = state.entities.max_half_width();
    let min_x = mc_physics::mth::floor_i32(x - extent).div_euclid(16);
    let max_x = mc_physics::mth::floor_i32(x + 1.0 + extent).div_euclid(16);
    let min_z = mc_physics::mth::floor_i32(z - extent).div_euclid(16);
    let max_z = mc_physics::mth::floor_i32(z + 1.0 + extent).div_euclid(16);
    let cells =
        (i64::from(max_x) - i64::from(min_x) + 1) * (i64::from(max_z) - i64::from(min_z) + 1);
    // Tiny populations or a previously enormous body can make buckets dearer.
    if cells * 2 >= state.entities.len() as i64 {
        return state.entities.slots().any(intersects);
    }
    for cx in min_x..=max_x {
        for cz in min_z..=max_z {
            let chunk = ChunkPos { x: cx, z: cz };
            for index in [
                state.entities.brain_sensor_chunks(),
                state.entities.villager_chunks(),
            ] {
                if index.get(&chunk).is_some_and(|ids| {
                    ids.iter()
                        .filter_map(|id| state.entities.slot_of_any(*id))
                        .any(intersects)
                }) {
                    return true;
                }
            }
        }
    }
    false
}
