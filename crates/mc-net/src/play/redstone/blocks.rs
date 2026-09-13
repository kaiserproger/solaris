//! Block-role classification and state helpers for the redstone model.

use mc_world::{BlockPos, BlockState, BlockStateId};

use super::super::super::block_state_property;

/// Vanilla dust power ceiling.
pub(super) const MAX_DUST_POWER: u8 = 15;

/// Horizontal neighbours in the state property order used by dust states.
pub(super) const HORIZONTAL: [(&str, (i32, i32, i32)); 4] = [
    ("north", (0, 0, -1)),
    ("south", (0, 0, 1)),
    ("west", (-1, 0, 0)),
    ("east", (1, 0, 0)),
];

/// The six axis-aligned neighbour offsets.
pub(super) const NEIGHBOURS: [(i32, i32, i32); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

/// How a block participates in the redstone model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    /// `minecraft:redstone_wire`: carries power 0..=15 with a connection shape.
    Dust,
    /// Standing or wall redstone torch: inverts the power reaching its support.
    Torch,
    /// Block of redstone: emits 15 to every neighbour.
    RedstoneBlock,
    /// Lever or button: the interactive on/off source.
    Switch,
    /// Pressure plate: on/off source driven by entities (plate activation by
    /// entities is out of scope, the `powered` state is honoured as a source).
    PressurePlate,
    /// Piston base; `sticky` selects the sticky variant.
    Piston { sticky: bool },
    /// Piston head or moving piston; never a tracked component.
    PistonHead,
    /// Door / trapdoor / fence gate / redstone lamp: reacts to received power.
    Consumer,
    /// Everything else.
    Other,
}

pub(super) fn role(state: &BlockState) -> Role {
    let path = state.block.id.path();
    match path {
        "redstone_wire" => Role::Dust,
        "redstone_torch" | "redstone_wall_torch" => Role::Torch,
        "redstone_block" => Role::RedstoneBlock,
        "lever" => Role::Switch,
        "piston" => Role::Piston { sticky: false },
        "sticky_piston" => Role::Piston { sticky: true },
        "piston_head" | "moving_piston" => Role::PistonHead,
        _ => {
            if path.ends_with("_button") {
                Role::Switch
            } else if path.ends_with("_pressure_plate") {
                Role::PressurePlate
            } else if is_consumer_path(path) {
                Role::Consumer
            } else {
                Role::Other
            }
        }
    }
}

/// Blocks that open/light when powered. Repeaters, comparators and observers
/// keep their existing non-redstone behaviour in this slice.
fn is_consumer_path(path: &str) -> bool {
    path.ends_with("_door") || path.ends_with("_trapdoor") || path.ends_with("_fence_gate")
}

pub(super) fn is_dust(state: &BlockState) -> bool {
    state.block.id.path() == "redstone_wire"
}

/// Power a state pushes into every adjacent block, or `None` when it is inert.
pub(super) fn emitted_power(state: &BlockState) -> Option<u8> {
    let path = state.block.id.path();
    if path == "redstone_block" {
        return Some(MAX_DUST_POWER);
    }
    if path == "redstone_torch" || path == "redstone_wall_torch" {
        return (block_state_property(state, "lit") == Some("true")).then_some(MAX_DUST_POWER);
    }
    if path == "lever" || path.ends_with("_button") || path.ends_with("_pressure_plate") {
        return (block_state_property(state, "powered") == Some("true")).then_some(MAX_DUST_POWER);
    }
    None
}

/// Stored dust power, `0` when the property is missing.
pub(super) fn dust_power(state: &BlockState) -> u8 {
    block_state_property(state, "power")
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(0)
        .min(MAX_DUST_POWER)
}

/// State for a dust position: connection shape plus recomputed power.
///
/// Returns `None` when the registry has no matching state.
pub(super) fn dust_state(
    blocks: &mc_world::BlockRegistry,
    state: &BlockState,
    power: u8,
    shape: [&'static str; 4],
) -> Option<BlockStateId> {
    let mut properties = state.properties.clone();
    let mut saw_power = false;
    let mut saw_shape = 0usize;
    for (key, value) in properties.iter_mut() {
        if key == "power" {
            *value = power.to_string();
            saw_power = true;
            continue;
        }
        if let Some((index, _)) = HORIZONTAL
            .iter()
            .enumerate()
            .find(|(_, (name, _))| name == key)
        {
            *value = shape[index].to_string();
            saw_shape += 1;
        }
    }
    if !saw_power || saw_shape != HORIZONTAL.len() {
        return None;
    }
    blocks.by_name_and_props(&state.block.id, &properties)
}

/// The dust connection value pointing from a dust state toward `neighbour`.
///
/// `neighbour_has_dust_above` is the riser check: the neighbour itself is not a
/// component but a wire sits on top of it.
pub(super) fn dust_shape_toward(
    neighbour: &BlockState,
    neighbour_has_dust_above: bool,
) -> &'static str {
    match role(neighbour) {
        Role::Dust
        | Role::Torch
        | Role::RedstoneBlock
        | Role::Switch
        | Role::PressurePlate
        | Role::Piston { .. }
        | Role::Consumer => "side",
        role if neighbour_has_dust_above && !is_air(neighbour) && role != Role::PistonHead => "up",
        _ => "none",
    }
}

pub(super) fn is_air(state: &BlockState) -> bool {
    matches!(state.block.id.path(), "air" | "cave_air" | "void_air")
}

/// Offset helper with checked arithmetic; `None` on world-edge overflow.
pub(super) fn offset(pos: BlockPos, delta: (i32, i32, i32)) -> Option<BlockPos> {
    Some(BlockPos {
        x: pos.x.checked_add(delta.0)?,
        y: pos.y.checked_add(delta.1)?,
        z: pos.z.checked_add(delta.2)?,
    })
}

/// Facing vector for the `facing` property of pistons (and torches).
pub(super) fn facing_delta(facing: &str) -> Option<(i32, i32, i32)> {
    match facing {
        "east" => Some((1, 0, 0)),
        "west" => Some((-1, 0, 0)),
        "up" => Some((0, 1, 0)),
        "down" => Some((0, -1, 0)),
        "south" => Some((0, 0, 1)),
        "north" => Some((0, 0, -1)),
        _ => None,
    }
}
