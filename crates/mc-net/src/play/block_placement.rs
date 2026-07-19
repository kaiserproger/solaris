use mc_nbt::{ListTag, Tag};
use mc_protocol::packets::play::Direction;
use mc_world::{BlockPos, BlockRegistry, BlockState, BlockStateId, WorldReadSnapshot};

use super::plants::vertical_plant_can_survive_at;
use super::{
    BlockEdit, BlockEditBatchOutcome, BlockEditPrecondition, PendingSignEdit, PlayerPose,
    sibling_state_with_property,
};

#[derive(Debug, PartialEq, Eq)]
pub(super) struct PlannedBlockPlacement {
    pub(super) edits: Vec<BlockEdit>,
    pub(super) additional_preconditions: Vec<BlockEditPrecondition>,
}

pub(super) fn placement_snapshot_positions(
    blocks: &BlockRegistry,
    placed_state: BlockStateId,
    pos: BlockPos,
) -> Option<Vec<BlockPos>> {
    let placed = blocks.by_id(placed_state)?;
    if is_editable_sign_state(placed) || placed.block.id.path().ends_with("_wall_sign") {
        return Some(Vec::new());
    }
    if placed.block.id.path().ends_with("_door") {
        return Some(vec![BlockPos {
            y: pos.y + 1,
            ..pos
        }]);
    }

    let below = BlockPos {
        y: pos.y - 1,
        ..pos
    };
    let mut positions = vec![pos, below];
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        positions.push(BlockPos {
            x: pos.x + dx,
            z: pos.z + dz,
            ..pos
        });
        positions.push(BlockPos {
            x: below.x + dx,
            z: below.z + dz,
            ..below
        });
    }
    Some(positions)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_block_placement(
    blocks: &BlockRegistry,
    placed_state: BlockStateId,
    snapshot: Option<&WorldReadSnapshot>,
    pos: BlockPos,
    player_pose: PlayerPose,
    direction: Direction,
    target_relative_hit_y: f32,
    air: BlockStateId,
) -> Option<PlannedBlockPlacement> {
    let placed = blocks.by_id(placed_state)?;
    if let Some(new_state) = sign_placement_state(blocks, placed, player_pose, direction) {
        return Some(PlannedBlockPlacement {
            edits: vec![BlockEdit { pos, new_state }],
            additional_preconditions: Vec::new(),
        });
    }
    let placed_state = oriented_stair_or_slab_state(
        blocks,
        placed_state,
        placed,
        player_pose,
        direction,
        target_relative_hit_y,
    );
    let placed = blocks.by_id(placed_state)?;

    let snapshot = snapshot?;
    if !placed.block.id.path().ends_with("_door") {
        if !vertical_plant_can_survive_at(blocks, snapshot, pos, placed_state) {
            return None;
        }
        if placed.block.id.path() == "cactus" && cactus_has_side_neighbor(blocks, snapshot, pos) {
            return None;
        }
        let mut edits = vec![BlockEdit {
            pos,
            new_state: placed_state,
        }];
        append_cactus_side_neighbor_cascades(blocks, snapshot, &mut edits, pos, placed_state, air);
        let mut additional_preconditions = Vec::with_capacity(edits.len().saturating_sub(1));
        for edit in edits.iter().skip(1) {
            let expected_state = snapshot.get_cached_block(edit.pos)?;
            let expected_token = snapshot.block_mutation_token(edit.pos)?;
            additional_preconditions.push(BlockEditPrecondition {
                pos: edit.pos,
                expected_state,
                expected_token,
            });
        }
        return Some(PlannedBlockPlacement {
            edits,
            additional_preconditions,
        });
    }

    let upper_pos = BlockPos {
        y: pos.y + 1,
        ..pos
    };
    let upper_token = match snapshot.get_cached_block(upper_pos) {
        Some(state_id) if state_id == air => snapshot.block_mutation_token(upper_pos),
        Some(_) | None => None,
    }?;
    let facing = horizontal_facing_from_yaw(player_pose.yaw);
    let lower = door_half_state(blocks, placed, "lower", facing)?;
    let upper = door_half_state(blocks, placed, "upper", facing)?;
    Some(PlannedBlockPlacement {
        edits: vec![
            BlockEdit {
                pos,
                new_state: lower,
            },
            BlockEdit {
                pos: upper_pos,
                new_state: upper,
            },
        ],
        additional_preconditions: vec![BlockEditPrecondition {
            pos: upper_pos,
            expected_state: air,
            expected_token: upper_token,
        }],
    })
}

fn oriented_stair_or_slab_state(
    blocks: &BlockRegistry,
    placed_state: BlockStateId,
    state: &BlockState,
    player_pose: PlayerPose,
    direction: Direction,
    target_relative_hit_y: f32,
) -> BlockStateId {
    let path = state.block.id.path();
    let mut properties = state.properties.clone();
    if path.ends_with("_stairs") {
        set_prop_if_present(
            &mut properties,
            "facing",
            horizontal_facing_from_yaw(player_pose.yaw),
        );
        set_prop_if_present(
            &mut properties,
            "half",
            stair_or_slab_half(direction, target_relative_hit_y),
        );
    } else if path.ends_with("_slab") {
        set_prop_if_present(
            &mut properties,
            "type",
            stair_or_slab_half(direction, target_relative_hit_y),
        );
    } else {
        return placed_state;
    }

    blocks
        .by_name_and_props(&state.block.id, &properties)
        .unwrap_or(placed_state)
}

fn stair_or_slab_half(direction: Direction, target_relative_hit_y: f32) -> &'static str {
    if direction == Direction::Down || (direction != Direction::Up && target_relative_hit_y > 0.5) {
        "top"
    } else {
        "bottom"
    }
}

pub(super) fn append_cactus_side_neighbor_cascades(
    blocks: &BlockRegistry,
    snapshot: &WorldReadSnapshot,
    edits: &mut Vec<BlockEdit>,
    placed: BlockPos,
    placed_state: BlockStateId,
    air: BlockStateId,
) {
    if blocks
        .by_id(placed_state)
        .is_none_or(|state| !is_known_cactus_side_obstructor(state.block.id.path()))
    {
        return;
    }

    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        let mut y = placed.y;
        loop {
            let pos = BlockPos {
                x: placed.x + dx,
                y,
                z: placed.z + dz,
            };
            let Some(state_id) = snapshot.get_cached_block(pos) else {
                break;
            };
            let Some(state) = blocks.by_id(state_id) else {
                break;
            };
            if state.block.id.path() != "cactus" {
                break;
            }
            push_unique_block_edit(
                edits,
                BlockEdit {
                    pos,
                    new_state: air,
                },
            );
            y += 1;
        }
    }
}

fn cactus_has_side_neighbor(
    blocks: &BlockRegistry,
    snapshot: &WorldReadSnapshot,
    placed: BlockPos,
) -> bool {
    [(1, 0), (-1, 0), (0, 1), (0, -1)]
        .into_iter()
        .any(|(dx, dz)| {
            let pos = BlockPos {
                x: placed.x + dx,
                y: placed.y,
                z: placed.z + dz,
            };
            let Some(state_id) = snapshot.get_cached_block(pos) else {
                return false;
            };
            blocks
                .by_id(state_id)
                .is_some_and(|state| state.block.id.path() == "cactus")
        })
}

fn is_known_cactus_side_obstructor(path: &str) -> bool {
    matches!(
        path,
        "stone"
            | "granite"
            | "polished_granite"
            | "diorite"
            | "polished_diorite"
            | "andesite"
            | "polished_andesite"
            | "deepslate"
            | "cobbled_deepslate"
            | "tuff"
            | "calcite"
            | "dripstone_block"
            | "grass_block"
            | "dirt"
            | "coarse_dirt"
            | "podzol"
            | "rooted_dirt"
            | "mud"
            | "clay"
            | "sand"
            | "red_sand"
            | "gravel"
            | "cobblestone"
            | "mossy_cobblestone"
            | "obsidian"
            | "crying_obsidian"
            | "bedrock"
            | "netherrack"
            | "basalt"
            | "smooth_basalt"
            | "blackstone"
            | "end_stone"
            | "anvil"
            | "chipped_anvil"
            | "damaged_anvil"
    ) || path.ends_with("_planks")
        || path.ends_with("_log")
        || path.ends_with("_wood")
        || path.ends_with("_stem")
        || path.ends_with("_hyphae")
        || path.ends_with("_leaves")
        || path.ends_with("_wool")
        || path.ends_with("_terracotta")
        || path.ends_with("_concrete")
        || path.ends_with("_concrete_powder")
}

pub(super) fn sign_placement_state(
    blocks: &BlockRegistry,
    state: &BlockState,
    player_pose: PlayerPose,
    direction: Direction,
) -> Option<BlockStateId> {
    let path = state.block.id.path();
    if path.ends_with("_wall_sign") {
        return direction_to_horizontal_facing(direction)
            .and_then(|facing| sibling_state_with_property(blocks, state, "facing", facing));
    }
    if is_editable_sign_state(state) {
        return sibling_state_with_property(
            blocks,
            state,
            "rotation",
            &sign_rotation_from_yaw(player_pose.yaw).to_string(),
        );
    }
    None
}

pub(super) fn placed_sign_edit(
    blocks: &BlockRegistry,
    outcome: &BlockEditBatchOutcome,
) -> Option<PendingSignEdit> {
    outcome.applied.iter().find_map(|edit| {
        blocks
            .by_id(edit.new_state)
            .filter(|state| is_editable_sign_state(state))
            .and_then(|_| {
                outcome
                    .resulting_tokens
                    .get(&edit.pos)
                    .copied()
                    .map(|token| PendingSignEdit {
                        position: edit.pos,
                        state: edit.new_state,
                        token,
                        is_front_text: true,
                    })
            })
    })
}

fn is_editable_sign_state(state: &BlockState) -> bool {
    let path = state.block.id.path();
    path.ends_with("_sign") && !path.ends_with("_hanging_sign")
}

pub(super) fn sign_block_entity_update_nbt(lines: &[String], is_front_text: bool) -> Tag {
    let text = sign_text_nbt(lines);
    let empty = sign_text_nbt(&[]);
    Tag::Compound(vec![
        (
            "front_text".into(),
            if is_front_text {
                text.clone()
            } else {
                empty.clone()
            },
        ),
        ("back_text".into(), if is_front_text { empty } else { text }),
        ("is_waxed".into(), Tag::Byte(0)),
    ])
}

pub(super) fn sign_block_entity_persistent_nbt(pos: BlockPos, update_tag: &Tag) -> Tag {
    let Tag::Compound(fields) = update_tag else {
        unreachable!("sign update tag is always a compound")
    };
    let mut fields = fields.clone();
    fields.extend([
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
        ("id".into(), Tag::String("minecraft:sign".into())),
    ]);
    Tag::Compound(fields)
}

fn sign_text_nbt(lines: &[String]) -> Tag {
    let messages = (0..4)
        .map(|idx| Tag::String(lines.get(idx).cloned().unwrap_or_default()))
        .collect();
    Tag::Compound(vec![
        (
            "messages".into(),
            Tag::List(ListTag {
                element_type: mc_nbt::tag_type::STRING,
                elements: messages,
            }),
        ),
        ("color".into(), Tag::String("black".into())),
        ("has_glowing_text".into(), Tag::Byte(0)),
    ])
}

fn direction_to_horizontal_facing(direction: Direction) -> Option<&'static str> {
    match direction {
        Direction::North => Some("north"),
        Direction::South => Some("south"),
        Direction::West => Some("west"),
        Direction::East => Some("east"),
        Direction::Down | Direction::Up => None,
    }
}

fn sign_rotation_from_yaw(yaw: f32) -> u8 {
    ((yaw.rem_euclid(360.0) / 22.5).round() as i32).rem_euclid(16) as u8
}

pub(super) fn door_half_state(
    blocks: &BlockRegistry,
    state: &BlockState,
    half: &str,
    facing: &str,
) -> Option<BlockStateId> {
    let mut props = state.properties.clone();
    set_prop_if_present(&mut props, "half", half);
    set_prop_if_present(&mut props, "facing", facing);
    set_prop_if_present(&mut props, "open", "false");
    set_prop_if_present(&mut props, "powered", "false");
    blocks.by_name_and_props(&state.block.id, &props)
}

fn set_prop_if_present(props: &mut [(String, String)], name: &str, value: &str) {
    if let Some((_, current)) = props.iter_mut().find(|(key, _)| key == name) {
        *current = value.to_string();
    }
}

pub(super) fn horizontal_facing_from_yaw(yaw: f32) -> &'static str {
    match ((yaw.rem_euclid(360.0) / 90.0).round() as i32).rem_euclid(4) {
        0 => "south",
        1 => "west",
        2 => "north",
        _ => "east",
    }
}

fn push_unique_block_edit(edits: &mut Vec<BlockEdit>, edit: BlockEdit) {
    if edits.iter().any(|existing| existing.pos == edit.pos) {
        return;
    }
    edits.push(edit);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mc_data::Identifier;
    use mc_data::blocks::{BlockReport, BlockStateReport};
    use mc_world::{BlockPos, BlockRegistry, BlockStateId};

    use super::placement_snapshot_positions;

    fn simple_block(id: u32, name: &str) -> BlockReport {
        BlockReport {
            id: Identifier::parse(name).unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id,
                default: true,
                properties: BTreeMap::new(),
            }],
        }
    }

    #[test]
    fn snapshot_positions_preserve_each_placement_category() {
        let blocks = BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:oak_sign"),
            simple_block(2, "minecraft:oak_wall_sign"),
            simple_block(3, "minecraft:oak_door"),
            simple_block(4, "minecraft:stone"),
            simple_block(5, "minecraft:cactus"),
        ])
        .unwrap();
        let pos = BlockPos {
            x: 10,
            y: 64,
            z: -4,
        };
        let below = BlockPos { y: 63, ..pos };
        let ordinary = vec![
            pos,
            below,
            BlockPos { x: 11, ..pos },
            BlockPos { x: 11, ..below },
            BlockPos { x: 9, ..pos },
            BlockPos { x: 9, ..below },
            BlockPos { z: -3, ..pos },
            BlockPos { z: -3, ..below },
            BlockPos { z: -5, ..pos },
            BlockPos { z: -5, ..below },
        ];
        let cases = [
            (BlockStateId(1), Vec::new()),
            (BlockStateId(2), Vec::new()),
            (BlockStateId(3), vec![BlockPos { y: 65, ..pos }]),
            (BlockStateId(4), ordinary.clone()),
            (BlockStateId(5), ordinary),
        ];

        for (state, expected) in cases {
            assert_eq!(
                placement_snapshot_positions(&blocks, state, pos),
                Some(expected)
            );
        }
    }
}
