//! Bounded, deterministic piston move planning.
//!
//! A move is planned as one atomic group: either every edit of the push (or
//! pull) is planned, or nothing is. The planner refuses rather than partially
//! applying, which is what keeps a move from duplicating or destroying blocks
//! and block-entity contents.

use mc_data::Identifier;
use mc_world::{BlockPos, BlockRegistry, BlockState, BlockStateId};

use super::super::super::{BlockPlanningRead, block_state_property};
use super::blocks::{facing_delta, offset};
use super::power::Settle;

/// Vanilla push limit: at most twelve blocks move ahead of the piston.
pub(super) const MAX_PUSH_DISTANCE: usize = 12;

/// Why a piston could not move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Refusal {
    /// The state does not describe a piston this slice can move.
    Unsupported,
    /// A pushed or pulled block is immovable (component, block entity, bedrock).
    Immovable(BlockPos),
    /// More than [`MAX_PUSH_DISTANCE`] blocks would move.
    PushLimit,
    /// The transaction touches a protected position.
    Protected(BlockPos),
    /// A position in the path is not resident, so the move cannot be validated.
    Unloaded(BlockPos),
    /// The registry has no `piston_head` state for the required facing/type.
    MissingHead,
}

/// Plan the extend/retract group for `pos`. Returns `Ok(())` when nothing needs
/// to change.
pub(super) fn plan_move<R: BlockPlanningRead + ?Sized>(
    world: &mut Settle<'_, R>,
    pos: BlockPos,
    state: &BlockState,
    sticky: bool,
    powered: bool,
) -> Result<(), Refusal> {
    let facing = block_state_property(state, "facing").ok_or(Refusal::Unsupported)?;
    let extended = block_state_property(state, "extended") == Some("true");
    if powered == extended {
        return Ok(());
    }
    let step = facing_delta(facing).ok_or(Refusal::Unsupported)?;
    let head = offset(pos, step).ok_or(Refusal::Unloaded(pos))?;
    let base_state = super::super::super::sibling_state_with_bool_property(
        world.blocks,
        state,
        "extended",
        powered,
    )
    .ok_or(Refusal::Unsupported)?;

    if powered {
        plan_extend(world, pos, head, step, base_state, facing, sticky)
    } else {
        plan_retract(world, pos, head, step, base_state, sticky)
    }
}

fn plan_extend<R: BlockPlanningRead + ?Sized>(
    world: &mut Settle<'_, R>,
    pos: BlockPos,
    head: BlockPos,
    step: (i32, i32, i32),
    base_state: BlockStateId,
    facing: &str,
    sticky: bool,
) -> Result<(), Refusal> {
    let mut chain: Vec<(BlockPos, BlockStateId)> = Vec::new();
    let mut cursor = head;
    loop {
        let Some(state_id) = world.block_at(cursor) else {
            return Err(Refusal::Unloaded(cursor));
        };
        let Some(state) = world.blocks.by_id(state_id) else {
            return Err(Refusal::Unloaded(cursor));
        };
        if super::blocks::is_air(state) {
            break;
        }
        if !is_pushable(&state.block.id) {
            return Err(Refusal::Immovable(cursor));
        }
        chain.push((cursor, state_id));
        if chain.len() > MAX_PUSH_DISTANCE {
            return Err(Refusal::PushLimit);
        }
        cursor = offset(cursor, step).ok_or(Refusal::Unloaded(cursor))?;
    }

    // Validate every touched position before planning anything.
    let mut destinations = Vec::with_capacity(chain.len() + 2);
    destinations.push(pos);
    destinations.push(head);
    for (chain_pos, _) in &chain {
        destinations.push(offset(*chain_pos, step).ok_or(Refusal::Unloaded(*chain_pos))?);
    }
    for destination in destinations {
        if !world.protection_allows(destination) {
            return Err(Refusal::Protected(destination));
        }
    }

    let head_state = head_state(world.blocks, facing, sticky).ok_or(Refusal::MissingHead)?;
    world.plan(pos, base_state);
    for (chain_pos, chain_state) in chain.iter().rev() {
        let destination = offset(*chain_pos, step).ok_or(Refusal::Unloaded(*chain_pos))?;
        world.plan(destination, *chain_state);
    }
    world.plan(head, head_state);
    Ok(())
}

fn plan_retract<R: BlockPlanningRead + ?Sized>(
    world: &mut Settle<'_, R>,
    pos: BlockPos,
    head: BlockPos,
    step: (i32, i32, i32),
    base_state: BlockStateId,
    sticky: bool,
) -> Result<(), Refusal> {
    let air = super::super::super::air_state_id(world.blocks);
    // Only a real piston head is cleared/pulled: a base flagged extended whose
    // head block is gone must not delete whatever now occupies that position.
    let head_is_head = world
        .state_at(head)
        .is_some_and(|state| matches!(super::blocks::role(state), super::blocks::Role::PistonHead));
    // Decide the whole transaction before planning any of it.
    let mut pulled: Option<(BlockPos, BlockPos, BlockStateId)> = None;
    if sticky
        && head_is_head
        && let Some(behind) = offset(head, step)
        && let Some(behind_state_id) = world.block_at(behind)
        && let Some(behind_state) = world.blocks.by_id(behind_state_id)
        && !super::blocks::is_air(behind_state)
        && is_pushable(&behind_state.block.id)
    {
        pulled = Some((head, behind, behind_state_id));
    }

    let mut touched = vec![pos];
    if head_is_head {
        touched.push(head);
    }
    if let Some((_, behind, _)) = pulled {
        touched.push(behind);
    }
    for touched_pos in touched {
        if !world.protection_allows(touched_pos) {
            return Err(Refusal::Protected(touched_pos));
        }
    }

    world.plan(pos, base_state);
    match pulled {
        Some((head, behind, behind_state_id)) => {
            world.plan(head, behind_state_id);
            world.plan(behind, air);
        }
        None if head_is_head => {
            world.plan(head, air);
        }
        None => {}
    }
    Ok(())
}

/// Vanilla `piston_head` state for the facing and piston type.
fn head_state(blocks: &BlockRegistry, facing: &str, sticky: bool) -> Option<BlockStateId> {
    let head = Identifier::parse("minecraft:piston_head").ok()?;
    let kind = if sticky { "sticky" } else { "normal" };
    let full = [
        ("facing".to_string(), facing.to_string()),
        ("short".to_string(), "false".to_string()),
        ("type".to_string(), kind.to_string()),
    ];
    if let Some(state) = blocks.by_name_and_props(&head, &full) {
        return Some(state);
    }
    let without_short = [
        ("facing".to_string(), facing.to_string()),
        ("type".to_string(), kind.to_string()),
    ];
    if let Some(state) = blocks.by_name_and_props(&head, &without_short) {
        return Some(state);
    }
    blocks.by_name_and_props(&head, &[("facing".to_string(), facing.to_string())])
}

/// Blocks a piston may not move in this slice.
///
/// `false` means "refuse": redstone components pop off or need a component
/// update this slice does not model, and block-entity blocks must carry their
/// contents, which the edit batch cannot express yet. Refusing is the
/// conservative half of "move the contents or refuse".
pub(super) fn is_pushable(id: &Identifier) -> bool {
    !is_immovable_path(id.path())
}

fn is_immovable_path(path: &str) -> bool {
    if matches!(
        path,
        "redstone_wire"
            | "redstone_torch"
            | "redstone_wall_torch"
            | "lever"
            | "piston"
            | "sticky_piston"
            | "piston_head"
            | "moving_piston"
            | "bedrock"
            | "obsidian"
            | "crying_obsidian"
            | "ancient_debris"
            | "reinforced_deepslate"
            | "enchanting_table"
            | "end_portal_frame"
            | "end_portal"
            | "end_gateway"
            | "respawn_anchor"
            | "barrier"
            | "light"
            | "budding_amethyst"
            | "command_block"
            | "chain_command_block"
            | "repeating_command_block"
            | "structure_block"
            | "jigsaw"
            | "beacon"
            | "conduit"
    ) {
        return true;
    }
    if path.ends_with("_button") || path.ends_with("_pressure_plate") {
        return true;
    }
    if path.ends_with("_shulker_box")
        || path.ends_with("_sign")
        || path.ends_with("_hanging_sign")
        || path.ends_with("_banner")
        || path.ends_with("_skull")
        || path.ends_with("_head")
        || path.ends_with("_bed")
        || path.ends_with("_anvil")
    {
        return true;
    }
    matches!(
        path,
        "chest"
            | "trapped_chest"
            | "barrel"
            | "furnace"
            | "blast_furnace"
            | "smoker"
            | "hopper"
            | "dispenser"
            | "dropper"
            | "brewing_stand"
            | "campfire"
            | "soul_campfire"
            | "lectern"
            | "jukebox"
            | "flower_pot"
            | "decorated_pot"
            | "bell"
            | "mob_spawner"
            | "trial_spawner"
            | "vault"
            | "sculk_sensor"
            | "sculk_shrieker"
            | "daylight_detector"
            | "comparator"
            | "repeater"
            | "observer"
            | "ender_chest"
    )
}
