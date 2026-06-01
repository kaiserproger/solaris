use mc_data::items::ItemRegistry;
use mc_protocol::codec::Identifier;
use mc_protocol::packets::play::{Direction, ItemStack};
use tracing::warn;

use super::{BlockEdit, block_state_property, named_block_default, sibling_state_with_property};

pub(super) fn vertical_plant_growth_edit(
    blocks: &mc_world::BlockRegistry,
    storage: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
    state: mc_world::BlockStateId,
) -> Option<BlockEdit> {
    let current = blocks.by_id(state)?;
    if !matches!(current.block.id.path(), "sugar_cane" | "cactus" | "bamboo") {
        return None;
    }
    let plant_state = blocks.block(&current.block.id).map(|block| block.default)?;
    let air = named_block_default(blocks, "minecraft:air")?;

    let mut bottom_y = pos.y;
    while same_block_at(
        blocks,
        storage,
        current.block.id.path(),
        mc_world::BlockPos {
            y: bottom_y - 1,
            ..pos
        },
    )? {
        bottom_y -= 1;
    }
    let support = mc_world::BlockPos {
        y: bottom_y - 1,
        ..pos
    };
    if storage
        .get_block(support)
        .ok()
        .flatten()
        .is_none_or(|found| found == air)
    {
        return None;
    }

    let mut top_y = pos.y;
    while same_block_at(
        blocks,
        storage,
        current.block.id.path(),
        mc_world::BlockPos {
            y: top_y + 1,
            ..pos
        },
    )? {
        top_y += 1;
    }
    if top_y - bottom_y + 1 >= 3 {
        return None;
    }

    let above = mc_world::BlockPos {
        y: top_y + 1,
        ..pos
    };
    match storage.get_block(above) {
        Ok(Some(found)) if found == air => Some(BlockEdit {
            pos: above,
            new_state: plant_state,
        }),
        Ok(Some(_)) | Ok(None) => None,
        Err(err) => {
            warn!(error = %err, x = above.x, y = above.y, z = above.z, "vertical plant growth target read failed");
            None
        }
    }
}

fn same_block_at(
    blocks: &mc_world::BlockRegistry,
    storage: &mut mc_world::WorldStorage,
    path: &str,
    pos: mc_world::BlockPos,
) -> Option<bool> {
    let state = storage.get_block(pos).ok().flatten()?;
    Some(
        blocks
            .by_id(state)
            .is_some_and(|found| found.block.id.path() == path),
    )
}

pub(super) fn next_crop_growth_state(
    blocks: &mc_world::BlockRegistry,
    state: mc_world::BlockStateId,
) -> Option<mc_world::BlockStateId> {
    let current = blocks.by_id(state)?;
    if !is_supported_age_crop(&current.block.id) {
        return None;
    }
    let age = block_state_property(current, "age")?.parse::<u8>().ok()?;
    sibling_state_with_property(blocks, current, "age", &age.checked_add(1)?.to_string())
}

fn is_supported_age_crop(block: &Identifier) -> bool {
    matches!(
        block.as_str(),
        "minecraft:wheat"
            | "minecraft:carrots"
            | "minecraft:potatoes"
            | "minecraft:beetroots"
            | "minecraft:nether_wart"
            | "minecraft:melon_stem"
            | "minecraft:pumpkin_stem"
            | "minecraft:sweet_berry_bush"
            | "minecraft:cocoa"
    )
}

pub(super) fn bonemeal_growth_edit(
    blocks: &mc_world::BlockRegistry,
    pos: mc_world::BlockPos,
    state: mc_world::BlockStateId,
) -> Option<BlockEdit> {
    next_crop_growth_state(blocks, state).map(|new_state| BlockEdit { pos, new_state })
}

pub(super) fn bonemeal_growth_edits(
    blocks: &mc_world::BlockRegistry,
    storage: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
    state: mc_world::BlockStateId,
) -> Option<Vec<BlockEdit>> {
    if let Some(edit) = bonemeal_growth_edit(blocks, pos, state) {
        return Some(vec![edit]);
    }
    if let Some(edits) = stem_fruit_edits(blocks, storage, pos, state) {
        return Some(edits);
    }
    sapling_tree_edits(blocks, storage, pos, state)
}

pub(super) fn stem_fruit_edits(
    blocks: &mc_world::BlockRegistry,
    storage: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
    state: mc_world::BlockStateId,
) -> Option<Vec<BlockEdit>> {
    let current = blocks.by_id(state)?;
    let (fruit_name, attached_name) = stem_lifecycle_blocks(current.block.id.as_str())?;
    let age = block_state_property(current, "age")?.parse::<u8>().ok()?;
    if sibling_state_with_property(blocks, current, "age", &age.checked_add(1)?.to_string())
        .is_some()
    {
        return None;
    }

    let fruit_state = named_block_default(blocks, fruit_name)?;
    let attached = Identifier::parse(attached_name).expect("static identifier");
    let attached_default = blocks
        .block(&attached)
        .and_then(|block| blocks.by_id(block.default))?;
    let air = named_block_default(blocks, "minecraft:air")?;

    for (facing, dx, dz) in [
        ("north", 0, -1),
        ("south", 0, 1),
        ("west", -1, 0),
        ("east", 1, 0),
    ] {
        let fruit_pos = mc_world::BlockPos {
            x: pos.x + dx,
            z: pos.z + dz,
            ..pos
        };
        if !matches!(storage.get_block(fruit_pos), Ok(Some(found)) if found == air) {
            continue;
        }
        let attached_state =
            sibling_state_with_property(blocks, attached_default, "facing", facing)?;
        return Some(vec![
            BlockEdit {
                pos,
                new_state: attached_state,
            },
            BlockEdit {
                pos: fruit_pos,
                new_state: fruit_state,
            },
        ]);
    }

    None
}

fn stem_lifecycle_blocks(stem: &str) -> Option<(&'static str, &'static str)> {
    match stem {
        "minecraft:melon_stem" => Some(("minecraft:melon", "minecraft:attached_melon_stem")),
        "minecraft:pumpkin_stem" => Some(("minecraft:pumpkin", "minecraft:attached_pumpkin_stem")),
        _ => None,
    }
}

pub(super) fn sapling_tree_edits(
    blocks: &mc_world::BlockRegistry,
    storage: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
    state: mc_world::BlockStateId,
) -> Option<Vec<BlockEdit>> {
    let current = blocks.by_id(state)?;
    let (log_name, leaves_name) = sapling_tree_blocks(current.block.id.as_str())?;

    let log = tree_state_with_props(blocks, log_name, &[("axis", "y")])?;
    let leaves = tree_leaves_state(blocks, leaves_name)?;
    let air = named_block_default(blocks, "minecraft:air")?;

    let mut edits = Vec::new();
    for dy in 0..=3 {
        edits.push(BlockEdit {
            pos: mc_world::BlockPos {
                y: pos.y + dy,
                ..pos
            },
            new_state: log,
        });
    }

    for dx in -1..=1 {
        for dz in -1..=1 {
            if dx == 0 && dz == 0 {
                continue;
            }
            edits.push(BlockEdit {
                pos: mc_world::BlockPos {
                    x: pos.x + dx,
                    y: pos.y + 3,
                    z: pos.z + dz,
                },
                new_state: leaves,
            });
        }
    }
    for (dx, dz) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)] {
        edits.push(BlockEdit {
            pos: mc_world::BlockPos {
                x: pos.x + dx,
                y: pos.y + 4,
                z: pos.z + dz,
            },
            new_state: leaves,
        });
    }

    for edit in &edits {
        if edit.pos == pos {
            continue;
        }
        match storage.get_block(edit.pos) {
            Ok(Some(found)) if found == air => {}
            Ok(Some(_)) | Ok(None) => return None,
            Err(err) => {
                warn!(error = %err, x = edit.pos.x, y = edit.pos.y, z = edit.pos.z, "sapling tree volume read failed");
                return None;
            }
        }
    }

    Some(edits)
}

fn sapling_tree_blocks(sapling: &str) -> Option<(&'static str, &'static str)> {
    match sapling {
        "minecraft:oak_sapling" => Some(("minecraft:oak_log", "minecraft:oak_leaves")),
        "minecraft:birch_sapling" => Some(("minecraft:birch_log", "minecraft:birch_leaves")),
        "minecraft:spruce_sapling" => Some(("minecraft:spruce_log", "minecraft:spruce_leaves")),
        "minecraft:jungle_sapling" => Some(("minecraft:jungle_log", "minecraft:jungle_leaves")),
        "minecraft:acacia_sapling" => Some(("minecraft:acacia_log", "minecraft:acacia_leaves")),
        "minecraft:dark_oak_sapling" => {
            Some(("minecraft:dark_oak_log", "minecraft:dark_oak_leaves"))
        }
        _ => None,
    }
}

fn tree_state_with_props(
    blocks: &mc_world::BlockRegistry,
    name: &str,
    props: &[(&str, &str)],
) -> Option<mc_world::BlockStateId> {
    let id = Identifier::parse(name).expect("static identifier");
    let props = props
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect::<Vec<_>>();
    blocks
        .by_name_and_props(&id, &props)
        .or_else(|| blocks.block(&id).map(|block| block.default))
}

fn tree_leaves_state(
    blocks: &mc_world::BlockRegistry,
    name: &str,
) -> Option<mc_world::BlockStateId> {
    tree_state_with_props(
        blocks,
        name,
        &[
            ("distance", "1"),
            ("persistent", "false"),
            ("waterlogged", "false"),
        ],
    )
    .or_else(|| {
        tree_state_with_props(
            blocks,
            name,
            &[
                ("distance", "1"),
                ("persistent", "true"),
                ("waterlogged", "false"),
            ],
        )
    })
    .or_else(|| {
        tree_state_with_props(
            blocks,
            name,
            &[("persistent", "true"), ("waterlogged", "false")],
        )
    })
}

pub(super) fn sweet_berry_harvest(
    blocks: &mc_world::BlockRegistry,
    items: &ItemRegistry,
    pos: mc_world::BlockPos,
    state: mc_world::BlockStateId,
) -> Option<(BlockEdit, ItemStack)> {
    let current = blocks.by_id(state)?;
    if current.block.id.as_str() != "minecraft:sweet_berry_bush" {
        return None;
    }
    let age = block_state_property(current, "age")?.parse::<u8>().ok()?;
    if age < 2 {
        return None;
    }

    let harvested_state = sibling_state_with_property(blocks, current, "age", "1")?;
    let berries = Identifier::parse("minecraft:sweet_berries").expect("static identifier");
    let item_id = items.id_of(&berries)?;
    Some((
        BlockEdit {
            pos,
            new_state: harvested_state,
        },
        ItemStack::new(item_id, i32::from(age - 1)),
    ))
}

pub(super) fn plant_drop_stacks(
    items: &ItemRegistry,
    block: &mc_world::BlockState,
) -> Option<Vec<ItemStack>> {
    const WHEAT_MATURE_DROPS: &[(&str, i32)] =
        &[("minecraft:wheat", 1), ("minecraft:wheat_seeds", 1)];
    const WHEAT_IMMATURE_DROPS: &[(&str, i32)] = &[("minecraft:wheat_seeds", 1)];
    const CARROT_MATURE_DROPS: &[(&str, i32)] = &[("minecraft:carrot", 2)];
    const CARROT_IMMATURE_DROPS: &[(&str, i32)] = &[("minecraft:carrot", 1)];
    const POTATO_MATURE_DROPS: &[(&str, i32)] = &[("minecraft:potato", 2)];
    const POTATO_IMMATURE_DROPS: &[(&str, i32)] = &[("minecraft:potato", 1)];
    const BEETROOT_MATURE_DROPS: &[(&str, i32)] =
        &[("minecraft:beetroot", 1), ("minecraft:beetroot_seeds", 1)];
    const BEETROOT_IMMATURE_DROPS: &[(&str, i32)] = &[("minecraft:beetroot_seeds", 1)];
    const NETHER_WART_MATURE_DROPS: &[(&str, i32)] = &[("minecraft:nether_wart", 2)];
    const NETHER_WART_IMMATURE_DROPS: &[(&str, i32)] = &[("minecraft:nether_wart", 1)];
    const COCOA_MATURE_DROPS: &[(&str, i32)] = &[("minecraft:cocoa_beans", 3)];
    const COCOA_IMMATURE_DROPS: &[(&str, i32)] = &[("minecraft:cocoa_beans", 1)];

    let (mature_age, mature_drops, immature_drops) = match block.block.id.as_str() {
        "minecraft:wheat" => (7, WHEAT_MATURE_DROPS, WHEAT_IMMATURE_DROPS),
        "minecraft:carrots" => (7, CARROT_MATURE_DROPS, CARROT_IMMATURE_DROPS),
        "minecraft:potatoes" => (7, POTATO_MATURE_DROPS, POTATO_IMMATURE_DROPS),
        "minecraft:beetroots" => (3, BEETROOT_MATURE_DROPS, BEETROOT_IMMATURE_DROPS),
        "minecraft:nether_wart" => (3, NETHER_WART_MATURE_DROPS, NETHER_WART_IMMATURE_DROPS),
        "minecraft:cocoa" => (2, COCOA_MATURE_DROPS, COCOA_IMMATURE_DROPS),
        _ => return None,
    };

    let age = block_state_property(block, "age")?.parse::<u8>().ok()?;
    let drops = if age >= mature_age {
        mature_drops
    } else {
        immature_drops
    };
    Some(
        drops
            .iter()
            .filter_map(|(item, count)| item_id(items, item).map(|id| ItemStack::new(id, *count)))
            .collect(),
    )
}

pub(super) fn is_cocoa_beans_item(items: &ItemRegistry, item_id: u32) -> bool {
    items
        .name_of(item_id)
        .is_some_and(|item| item.as_str() == "minecraft:cocoa_beans")
}

pub(super) fn cocoa_state_for_use_on(
    clicked_state: mc_world::BlockStateId,
    direction: Direction,
    blocks: &mc_world::BlockRegistry,
) -> Option<mc_world::BlockStateId> {
    let facing = cocoa_facing_for_direction(direction)?;
    let clicked = blocks.by_id(clicked_state)?;
    if !matches!(clicked.block.id.as_str(), "minecraft:jungle_log") {
        return None;
    }
    let cocoa = Identifier::parse("minecraft:cocoa").expect("static identifier");
    blocks.by_name_and_props(
        &cocoa,
        &[
            ("age".to_string(), "0".to_string()),
            ("facing".to_string(), facing.to_string()),
        ],
    )
}

fn cocoa_facing_for_direction(direction: Direction) -> Option<&'static str> {
    match direction {
        Direction::North => Some("north"),
        Direction::South => Some("south"),
        Direction::West => Some("west"),
        Direction::East => Some("east"),
        Direction::Down | Direction::Up => None,
    }
}

fn item_id(items: &ItemRegistry, name: &str) -> Option<u32> {
    let id = Identifier::parse(name).expect("static identifier");
    items.id_of(&id)
}
