use mc_data::items::ItemRegistry;
use mc_protocol::codec::Identifier;
use mc_protocol::packets::play::{Direction, ItemStack};

use super::{
    BlockEdit, BlockPlanningRead, block_state_property, named_block_default,
    sibling_state_with_property, splitmix64,
};

pub(super) fn vertical_plant_growth_edits(
    blocks: &mc_world::BlockRegistry,
    world: &impl BlockPlanningRead,
    pos: mc_world::BlockPos,
    state: mc_world::BlockStateId,
    random_seed: u64,
) -> Option<Vec<BlockEdit>> {
    let current = blocks.by_id(state)?;
    if !matches!(current.block.id.path(), "sugar_cane" | "cactus" | "bamboo") {
        return None;
    }
    let plant_path = current.block.id.path();
    if plant_path == "bamboo"
        && block_state_property(current, "age").is_some()
        && block_state_property(current, "leaves").is_some()
        && block_state_property(current, "stage").is_some()
    {
        return bamboo_growth_edits(blocks, world, pos, state, random_seed);
    }
    let plant_state = blocks.block(&current.block.id).map(|block| block.default)?;
    let air = named_block_default(blocks, "minecraft:air")?;
    let above = mc_world::BlockPos {
        y: pos.y + 1,
        ..pos
    };
    if world.get_cached_block(above)? != air {
        return None;
    }

    let mut bottom_y = pos.y;
    while same_block_at(
        blocks,
        world,
        plant_path,
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
    if !vertical_plant_supported_base(blocks, world, plant_path, support, air) {
        return None;
    }

    let max_height = if plant_path == "bamboo" { 16 } else { 3 };
    if pos.y - bottom_y + 1 >= max_height {
        return None;
    }

    if plant_path == "cactus" && !cactus_growth_target_clear(world, above, air) {
        return None;
    }
    Some(vec![BlockEdit {
        pos: above,
        new_state: plant_state,
    }])
}

pub(super) fn bamboo_sapling_growth_edits(
    blocks: &mc_world::BlockRegistry,
    world: &impl BlockPlanningRead,
    pos: mc_world::BlockPos,
    random_seed: u64,
) -> Option<Vec<BlockEdit>> {
    if !random_seed.is_multiple_of(3) {
        return None;
    }
    let current = blocks.by_id(world.get_cached_block(pos)?)?;
    if current.block.id.path() != "bamboo_sapling" {
        return None;
    }
    let air = named_block_default(blocks, "minecraft:air")?;
    let above = mc_world::BlockPos {
        y: pos.y + 1,
        ..pos
    };
    if world.get_cached_block(above)? != air {
        return None;
    }
    let bamboo = blocks.block(&Identifier::parse("minecraft:bamboo").ok()?)?;
    let bottom = bamboo_state_with_properties(blocks, bamboo.default, "0", "none", "0")?;
    let top = bamboo_state_with_properties(blocks, bamboo.default, "0", "small", "0")?;
    Some(vec![
        BlockEdit {
            pos,
            new_state: bottom,
        },
        BlockEdit {
            pos: above,
            new_state: top,
        },
    ])
}

fn bamboo_growth_edits(
    blocks: &mc_world::BlockRegistry,
    world: &impl BlockPlanningRead,
    pos: mc_world::BlockPos,
    state: mc_world::BlockStateId,
    random_seed: u64,
) -> Option<Vec<BlockEdit>> {
    let current = blocks.by_id(state)?;
    if block_state_property(current, "stage") != Some("0") || !random_seed.is_multiple_of(3) {
        return None;
    }
    let air = named_block_default(blocks, "minecraft:air")?;
    let above = mc_world::BlockPos {
        y: pos.y + 1,
        ..pos
    };
    if world.get_cached_block(above)? != air {
        return None;
    }

    let mut bottom_y = pos.y;
    while bottom_y > pos.y - 15
        && same_block_at(
            blocks,
            world,
            "bamboo",
            mc_world::BlockPos {
                y: bottom_y - 1,
                ..pos
            },
        )?
    {
        bottom_y -= 1;
    }
    let height = pos.y - bottom_y + 1;
    if height >= 16 {
        return None;
    }

    let b1_pos = mc_world::BlockPos {
        y: pos.y - 1,
        ..pos
    };
    let b2_pos = mc_world::BlockPos {
        y: pos.y - 2,
        ..pos
    };
    let b1 = world
        .get_cached_block(b1_pos)
        .and_then(|state| blocks.by_id(state))
        .filter(|block| block.block.id.path() == "bamboo");
    let b2 = world
        .get_cached_block(b2_pos)
        .and_then(|state| blocks.by_id(state))
        .filter(|block| block.block.id.path() == "bamboo");
    let leaves = if b1
        .and_then(|block| block_state_property(block, "leaves"))
        .is_none_or(|leaves| leaves == "none")
    {
        "small"
    } else {
        "large"
    };
    let age = if block_state_property(current, "age") == Some("1") || b2.is_some() {
        "1"
    } else {
        "0"
    };
    let terminal_roll =
        height >= 11 && splitmix64(random_seed ^ 0x4241_4d42_4f4f_5354).is_multiple_of(4);
    let stage = if terminal_roll || height == 15 {
        "1"
    } else {
        "0"
    };

    let mut edits = Vec::new();
    for y in bottom_y..=pos.y {
        let existing_pos = mc_world::BlockPos { y, ..pos };
        let existing = world.get_cached_block(existing_pos)?;
        let existing_state = blocks.by_id(existing)?;
        let desired_age = if age == "1" {
            "1"
        } else {
            block_state_property(existing_state, "age")?
        };
        let desired_leaves = if leaves == "large" && y == pos.y - 1 {
            "small"
        } else if leaves == "large" && y == pos.y - 2 {
            "none"
        } else {
            block_state_property(existing_state, "leaves")?
        };
        let desired = bamboo_state_with_properties(
            blocks,
            existing,
            desired_age,
            desired_leaves,
            block_state_property(existing_state, "stage")?,
        )?;
        if desired != existing {
            edits.push(BlockEdit {
                pos: existing_pos,
                new_state: desired,
            });
        }
    }
    let bamboo = blocks.block(&current.block.id)?;
    edits.push(BlockEdit {
        pos: above,
        new_state: bamboo_state_with_properties(blocks, bamboo.default, age, leaves, stage)?,
    });
    Some(edits)
}

fn bamboo_state_with_properties(
    blocks: &mc_world::BlockRegistry,
    state: mc_world::BlockStateId,
    age: &str,
    leaves: &str,
    stage: &str,
) -> Option<mc_world::BlockStateId> {
    let current = blocks.by_id(state)?;
    let mut properties = current.properties.clone();
    for (name, value) in [("age", age), ("leaves", leaves), ("stage", stage)] {
        properties.iter_mut().find(|(key, _)| key == name)?.1 = value.to_string();
    }
    blocks.by_name_and_props(&current.block.id, &properties)
}

pub(super) fn vertical_plant_can_survive_at(
    blocks: &mc_world::BlockRegistry,
    snapshot: &mc_world::WorldReadSnapshot,
    pos: mc_world::BlockPos,
    state: mc_world::BlockStateId,
) -> bool {
    let Some(current) = blocks.by_id(state) else {
        return false;
    };
    let path = current.block.id.path();
    if !matches!(path, "sugar_cane" | "cactus" | "bamboo") {
        return true;
    }
    let below = mc_world::BlockPos {
        y: pos.y - 1,
        ..pos
    };
    if same_block_at_snapshot(blocks, snapshot, path, below).unwrap_or(false) {
        return true;
    }
    let Some(air) = named_block_default(blocks, "minecraft:air") else {
        return false;
    };
    vertical_plant_supported_base_snapshot(blocks, snapshot, path, below, air)
}

fn vertical_plant_supported_base_snapshot(
    blocks: &mc_world::BlockRegistry,
    snapshot: &mc_world::WorldReadSnapshot,
    path: &str,
    support: mc_world::BlockPos,
    air: mc_world::BlockStateId,
) -> bool {
    let Some(support_state) = snapshot.get_cached_block(support) else {
        return false;
    };
    if support_state == air {
        return false;
    }
    let Some(support_path) = blocks
        .by_id(support_state)
        .map(|state| state.block.id.path())
    else {
        return false;
    };

    match path {
        "cactus" => matches!(support_path, "sand" | "red_sand" | "suspicious_sand"),
        "sugar_cane" => {
            supports_overworld_plant(support_path)
                && has_adjacent_sugar_cane_support_snapshot(blocks, snapshot, support)
        }
        "bamboo" => {
            supports_overworld_plant(support_path)
                || matches!(
                    support_path,
                    "bamboo" | "bamboo_sapling" | "gravel" | "suspicious_gravel"
                )
        }
        _ => false,
    }
}

fn has_adjacent_sugar_cane_support_snapshot(
    blocks: &mc_world::BlockRegistry,
    snapshot: &mc_world::WorldReadSnapshot,
    pos: mc_world::BlockPos,
) -> bool {
    [(1, 0), (-1, 0), (0, 1), (0, -1)]
        .into_iter()
        .any(|(dx, dz)| {
            let side = mc_world::BlockPos {
                x: pos.x + dx,
                z: pos.z + dz,
                ..pos
            };
            let Some(state) = snapshot.get_cached_block(side) else {
                return false;
            };
            blocks.by_id(state).is_some_and(|state| {
                matches!(
                    state.block.id.path(),
                    "water" | "flowing_water" | "frosted_ice"
                )
            })
        })
}

fn same_block_at_snapshot(
    blocks: &mc_world::BlockRegistry,
    snapshot: &mc_world::WorldReadSnapshot,
    path: &str,
    pos: mc_world::BlockPos,
) -> Option<bool> {
    let state = snapshot.get_cached_block(pos)?;
    Some(
        blocks
            .by_id(state)
            .is_some_and(|found| found.block.id.path() == path),
    )
}

fn vertical_plant_supported_base(
    blocks: &mc_world::BlockRegistry,
    world: &impl BlockPlanningRead,
    path: &str,
    support: mc_world::BlockPos,
    air: mc_world::BlockStateId,
) -> bool {
    let Some(support_state) = world.get_cached_block(support) else {
        return false;
    };
    if support_state == air {
        return false;
    }
    let Some(support_path) = blocks
        .by_id(support_state)
        .map(|state| state.block.id.path())
    else {
        return false;
    };

    match path {
        "cactus" => matches!(support_path, "sand" | "red_sand" | "suspicious_sand"),
        "sugar_cane" => {
            supports_overworld_plant(support_path)
                && has_adjacent_sugar_cane_support(blocks, world, support)
        }
        "bamboo" => {
            supports_overworld_plant(support_path)
                || matches!(
                    support_path,
                    "bamboo" | "bamboo_sapling" | "gravel" | "suspicious_gravel"
                )
        }
        _ => false,
    }
}

fn supports_overworld_plant(path: &str) -> bool {
    matches!(
        path,
        "sand"
            | "red_sand"
            | "suspicious_sand"
            | "dirt"
            | "coarse_dirt"
            | "rooted_dirt"
            | "mud"
            | "muddy_mangrove_roots"
            | "moss_block"
            | "pale_moss_block"
            | "grass_block"
            | "podzol"
            | "mycelium"
    )
}

fn has_adjacent_sugar_cane_support(
    blocks: &mc_world::BlockRegistry,
    world: &impl BlockPlanningRead,
    pos: mc_world::BlockPos,
) -> bool {
    [(1, 0), (-1, 0), (0, 1), (0, -1)]
        .into_iter()
        .any(|(dx, dz)| {
            let side = mc_world::BlockPos {
                x: pos.x + dx,
                z: pos.z + dz,
                ..pos
            };
            let Some(state) = world.get_cached_block(side) else {
                return false;
            };
            blocks.by_id(state).is_some_and(|state| {
                matches!(
                    state.block.id.path(),
                    "water" | "flowing_water" | "frosted_ice"
                )
            })
        })
}

fn cactus_growth_target_clear(
    world: &impl BlockPlanningRead,
    pos: mc_world::BlockPos,
    air: mc_world::BlockStateId,
) -> bool {
    [(1, 0), (-1, 0), (0, 1), (0, -1)]
        .into_iter()
        .all(|(dx, dz)| {
            let side = mc_world::BlockPos {
                x: pos.x + dx,
                z: pos.z + dz,
                ..pos
            };
            matches!(world.get_cached_block(side), Some(found) if found == air)
        })
}

fn same_block_at(
    blocks: &mc_world::BlockRegistry,
    world: &impl BlockPlanningRead,
    path: &str,
    pos: mc_world::BlockPos,
) -> Option<bool> {
    let state = world.get_cached_block(pos)?;
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

fn is_bonemeal_age_crop(block: &Identifier) -> bool {
    matches!(
        block.as_str(),
        "minecraft:wheat"
            | "minecraft:carrots"
            | "minecraft:potatoes"
            | "minecraft:beetroots"
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
    let current = blocks.by_id(state)?;
    if !is_bonemeal_age_crop(&current.block.id) {
        return None;
    }
    next_crop_growth_state(blocks, state).map(|new_state| BlockEdit { pos, new_state })
}

pub(super) fn bonemeal_growth_edits(
    blocks: &mc_world::BlockRegistry,
    world: &impl BlockPlanningRead,
    pos: mc_world::BlockPos,
    state: mc_world::BlockStateId,
    tree_seed: u64,
) -> Option<Vec<BlockEdit>> {
    if let Some(edit) = bonemeal_growth_edit(blocks, pos, state) {
        return Some(vec![edit]);
    }
    if let Some(edits) = stem_fruit_edits(blocks, world, pos, state) {
        return Some(edits);
    }
    sapling_tree_edits(blocks, world, pos, state, tree_seed)
}

pub(super) fn stem_fruit_edits(
    blocks: &mc_world::BlockRegistry,
    world: &impl BlockPlanningRead,
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
        if !matches!(world.get_cached_block(fruit_pos), Some(found) if found == air) {
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
    world: &impl BlockPlanningRead,
    pos: mc_world::BlockPos,
    state: mc_world::BlockStateId,
    tree_seed: u64,
) -> Option<Vec<BlockEdit>> {
    let current = blocks.by_id(state)?;
    let sapling_name = current.block.id.as_str();
    let (log_name, leaves_name, base_height, random_height) = sapling_tree_blocks(sapling_name)?;

    let two_by_two = find_two_by_two_saplings(blocks, world, pos, sapling_name);
    if sapling_name == "minecraft:dark_oak_sapling" && two_by_two.is_none() {
        return None;
    }

    let log = tree_state_with_props(blocks, log_name, &[("axis", "y")])?;
    let leaves = tree_leaves_state(blocks, leaves_name)?;
    let air = named_block_default(blocks, "minecraft:air")?;

    match block_state_property(current, "stage")? {
        "0" => {
            let staged = sibling_state_with_property(blocks, current, "stage", "1")?;
            return Some(vec![BlockEdit {
                pos,
                new_state: staged,
            }]);
        }
        "1" => {}
        _ => return None,
    }

    if sapling_name == "minecraft:dark_oak_sapling" {
        return dark_oak_two_by_two_edits(
            blocks,
            world,
            two_by_two?,
            log_name,
            leaves_name,
            base_height,
            tree_seed,
        );
    }

    if matches!(
        sapling_name,
        "minecraft:spruce_sapling" | "minecraft:jungle_sapling"
    ) && two_by_two.is_some()
    {
        return spruce_or_jungle_two_by_two_edits(
            blocks,
            world,
            two_by_two?,
            sapling_name,
            log_name,
            leaves_name,
            tree_seed,
        );
    }

    let tree_height = base_height + (tree_seed % (random_height + 1)) as i32;

    let mut edits = Vec::new();
    for dy in 0..tree_height {
        edits.push(BlockEdit {
            pos: mc_world::BlockPos {
                y: pos.y + dy,
                ..pos
            },
            new_state: log,
        });
    }

    for (layer, radius) in [(0, 1i32), (-1, 1), (-2, 2)] {
        let foliage_y = pos.y + tree_height + layer;
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let corner =
                    dx.unsigned_abs() == radius as u32 && dz.unsigned_abs() == radius as u32;
                if corner
                    && (layer != 0
                        || ((tree_seed
                            ^ (dx as i64 as u64).rotate_left(17)
                            ^ (dz as i64 as u64).rotate_left(41))
                            & 1
                            != 0))
                {
                    continue;
                }
                if dx == 0 && dz == 0 && foliage_y < pos.y + tree_height {
                    continue;
                }
                edits.push(BlockEdit {
                    pos: mc_world::BlockPos {
                        x: pos.x + dx,
                        y: foliage_y,
                        z: pos.z + dz,
                    },
                    new_state: leaves,
                });
            }
        }
    }

    for edit in &edits {
        if edit.pos == pos {
            continue;
        }
        match world.get_cached_block(edit.pos) {
            Some(found) if tree_growth_can_replace(blocks, found, air) => {}
            Some(_) | None => return None,
        }
    }

    Some(edits)
}

fn find_two_by_two_saplings(
    blocks: &mc_world::BlockRegistry,
    world: &impl BlockPlanningRead,
    clicked: mc_world::BlockPos,
    sapling_name: &str,
) -> Option<[mc_world::BlockPos; 4]> {
    for (dx, dz) in [(0, 0), (-1, 0), (0, -1), (-1, -1)] {
        let northwest = mc_world::BlockPos {
            x: clicked.x.checked_add(dx)?,
            z: clicked.z.checked_add(dz)?,
            ..clicked
        };
        let square = [
            northwest,
            mc_world::BlockPos {
                x: northwest.x.checked_add(1)?,
                ..northwest
            },
            mc_world::BlockPos {
                z: northwest.z.checked_add(1)?,
                ..northwest
            },
            mc_world::BlockPos {
                x: northwest.x.checked_add(1)?,
                z: northwest.z.checked_add(1)?,
                ..northwest
            },
        ];
        if square.iter().all(|position| {
            world
                .get_cached_block(*position)
                .and_then(|state| blocks.by_id(state))
                .is_some_and(|state| state.block.id.as_str() == sapling_name)
        }) {
            return Some(square);
        }
    }
    None
}

fn dark_oak_two_by_two_edits(
    blocks: &mc_world::BlockRegistry,
    world: &impl BlockPlanningRead,
    saplings: [mc_world::BlockPos; 4],
    log_name: &str,
    leaves_name: &str,
    tree_height: i32,
    tree_seed: u64,
) -> Option<Vec<BlockEdit>> {
    let northwest = saplings[0];
    let log = tree_state_with_props(blocks, log_name, &[("axis", "y")])?;
    let leaves = tree_leaves_state(blocks, leaves_name)?;
    let air = named_block_default(blocks, "minecraft:air")?;
    let mut edits = Vec::new();

    for dy in 0..tree_height {
        for trunk in saplings {
            edits.push(BlockEdit {
                pos: mc_world::BlockPos {
                    y: northwest.y.checked_add(dy)?,
                    ..trunk
                },
                new_state: log,
            });
        }
    }

    for layer in [0, -1, -2] {
        let foliage_y = northwest.y.checked_add(tree_height)?.checked_add(layer)?;
        for dx in -1i32..=2 {
            for dz in -1i32..=2 {
                let outer_x = dx == -1 || dx == 2;
                let outer_z = dz == -1 || dz == 2;
                if outer_x
                    && outer_z
                    && (layer != 0
                        || ((tree_seed
                            ^ (dx as i64 as u64).rotate_left(17)
                            ^ (dz as i64 as u64).rotate_left(41))
                            & 1
                            != 0))
                {
                    continue;
                }
                if (0..=1).contains(&dx)
                    && (0..=1).contains(&dz)
                    && foliage_y < northwest.y + tree_height
                {
                    continue;
                }
                edits.push(BlockEdit {
                    pos: mc_world::BlockPos {
                        x: northwest.x.checked_add(dx)?,
                        y: foliage_y,
                        z: northwest.z.checked_add(dz)?,
                    },
                    new_state: leaves,
                });
            }
        }
    }

    for edit in &edits {
        if saplings.contains(&edit.pos) {
            continue;
        }
        match world.get_cached_block(edit.pos) {
            Some(found) if tree_growth_can_replace(blocks, found, air) => {}
            Some(_) | None => return None,
        }
    }

    Some(edits)
}

fn spruce_or_jungle_two_by_two_edits(
    blocks: &mc_world::BlockRegistry,
    world: &impl BlockPlanningRead,
    saplings: [mc_world::BlockPos; 4],
    sapling_name: &str,
    log_name: &str,
    leaves_name: &str,
    tree_seed: u64,
) -> Option<Vec<BlockEdit>> {
    let northwest = saplings[0];
    let log = tree_state_with_props(blocks, log_name, &[("axis", "y")])?;
    let leaves = tree_leaves_state(blocks, leaves_name)?;
    let air = named_block_default(blocks, "minecraft:air")?;

    // Vanilla 26.1.2 uses trunk placer parameters (13, 2, 14) for mega spruce
    // and (10, 2, 19) for mega jungle. Solaris keeps those height ranges but
    // uses its deterministic tree seed instead of vanilla's RandomSource.
    let (base_height, first_random, second_random) = match sapling_name {
        "minecraft:spruce_sapling" => (13, 2u64, 14u64),
        "minecraft:jungle_sapling" => (10, 2u64, 19u64),
        _ => return None,
    };
    let tree_height = base_height
        + (tree_seed % (first_random + 1)) as i32
        + (splitmix64(tree_seed ^ 0x4d45_4741_5f48_4549) % (second_random + 1)) as i32;
    let trunk_top = northwest.y.checked_add(tree_height)?;
    let mut edits = Vec::new();

    for dy in 0..tree_height {
        for trunk in saplings {
            edits.push(BlockEdit {
                pos: mc_world::BlockPos {
                    y: northwest.y.checked_add(dy)?,
                    ..trunk
                },
                new_state: log,
            });
        }
    }

    if sapling_name == "minecraft:spruce_sapling" {
        let crown_depth = 6 + (splitmix64(tree_seed ^ 0x5350_5255_4345_5f43) % 4) as i32;
        for depth in 0..=crown_depth {
            let radius = ((depth + 1) / 2).min(3);
            push_two_by_two_leaf_layer(
                &mut edits,
                northwest,
                trunk_top.checked_sub(depth)?,
                radius,
                leaves,
                tree_seed ^ depth as u64,
                trunk_top,
            )?;
        }
    } else {
        for (dy, radius) in [(1, 1), (0, 2), (-1, 2), (-2, 2), (-3, 1)] {
            push_two_by_two_leaf_layer(
                &mut edits,
                northwest,
                trunk_top.checked_add(dy)?,
                radius,
                leaves,
                tree_seed ^ (dy as i64 as u64).rotate_left(23),
                trunk_top,
            )?;
        }
    }

    for edit in &edits {
        if saplings.contains(&edit.pos) {
            continue;
        }
        match world.get_cached_block(edit.pos) {
            Some(found) if tree_growth_can_replace(blocks, found, air) => {}
            Some(_) | None => return None,
        }
    }

    Some(edits)
}

fn push_two_by_two_leaf_layer(
    edits: &mut Vec<BlockEdit>,
    northwest: mc_world::BlockPos,
    y: i32,
    radius: i32,
    leaves: mc_world::BlockStateId,
    layer_seed: u64,
    trunk_top: i32,
) -> Option<()> {
    for dx in -radius..=radius + 1 {
        for dz in -radius..=radius + 1 {
            if y < trunk_top && (0..=1).contains(&dx) && (0..=1).contains(&dz) {
                continue;
            }
            let corner = (dx == -radius || dx == radius + 1) && (dz == -radius || dz == radius + 1);
            if corner
                && (splitmix64(
                    layer_seed
                        ^ (dx as i64 as u64).rotate_left(17)
                        ^ (dz as i64 as u64).rotate_left(41),
                ) & 1
                    != 0)
            {
                continue;
            }
            edits.push(BlockEdit {
                pos: mc_world::BlockPos {
                    x: northwest.x.checked_add(dx)?,
                    y,
                    z: northwest.z.checked_add(dz)?,
                },
                new_state: leaves,
            });
        }
    }
    Some(())
}

fn tree_growth_can_replace(
    blocks: &mc_world::BlockRegistry,
    state: mc_world::BlockStateId,
    air: mc_world::BlockStateId,
) -> bool {
    state == air
        || blocks
            .by_id(state)
            .is_some_and(|found| is_supported_tree_replaceable(found.block.id.as_str()))
}

fn is_supported_tree_replaceable(block: &str) -> bool {
    matches!(
        block,
        "minecraft:acacia_leaves"
            | "minecraft:azalea_leaves"
            | "minecraft:birch_leaves"
            | "minecraft:cherry_leaves"
            | "minecraft:dark_oak_leaves"
            | "minecraft:flowering_azalea_leaves"
            | "minecraft:jungle_leaves"
            | "minecraft:mangrove_leaves"
            | "minecraft:oak_leaves"
            | "minecraft:pale_oak_leaves"
            | "minecraft:spruce_leaves"
            | "minecraft:allium"
            | "minecraft:azure_bluet"
            | "minecraft:blue_orchid"
            | "minecraft:closed_eyeblossom"
            | "minecraft:cornflower"
            | "minecraft:dandelion"
            | "minecraft:golden_dandelion"
            | "minecraft:lily_of_the_valley"
            | "minecraft:open_eyeblossom"
            | "minecraft:orange_tulip"
            | "minecraft:oxeye_daisy"
            | "minecraft:pink_tulip"
            | "minecraft:poppy"
            | "minecraft:red_tulip"
            | "minecraft:torchflower"
            | "minecraft:white_tulip"
            | "minecraft:wither_rose"
            | "minecraft:bush"
            | "minecraft:crimson_roots"
            | "minecraft:dead_bush"
            | "minecraft:fern"
            | "minecraft:firefly_bush"
            | "minecraft:glow_lichen"
            | "minecraft:hanging_roots"
            | "minecraft:large_fern"
            | "minecraft:leaf_litter"
            | "minecraft:lilac"
            | "minecraft:nether_sprouts"
            | "minecraft:pale_moss_carpet"
            | "minecraft:peony"
            | "minecraft:pitcher_plant"
            | "minecraft:rose_bush"
            | "minecraft:seagrass"
            | "minecraft:short_dry_grass"
            | "minecraft:short_grass"
            | "minecraft:sunflower"
            | "minecraft:tall_dry_grass"
            | "minecraft:tall_grass"
            | "minecraft:tall_seagrass"
            | "minecraft:vine"
            | "minecraft:water"
            | "minecraft:warped_roots"
    )
}

fn sapling_tree_blocks(sapling: &str) -> Option<(&'static str, &'static str, i32, u64)> {
    match sapling {
        "minecraft:oak_sapling" => Some(("minecraft:oak_log", "minecraft:oak_leaves", 4, 2)),
        "minecraft:birch_sapling" => Some(("minecraft:birch_log", "minecraft:birch_leaves", 5, 2)),
        "minecraft:spruce_sapling" => {
            Some(("minecraft:spruce_log", "minecraft:spruce_leaves", 4, 0))
        }
        "minecraft:jungle_sapling" => {
            Some(("minecraft:jungle_log", "minecraft:jungle_leaves", 4, 0))
        }
        "minecraft:acacia_sapling" => {
            Some(("minecraft:acacia_log", "minecraft:acacia_leaves", 4, 0))
        }
        "minecraft:dark_oak_sapling" => {
            Some(("minecraft:dark_oak_log", "minecraft:dark_oak_leaves", 4, 0))
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
    let stacks = drops
        .iter()
        .map(|(item, count)| item_id(items, item).map(|id| ItemStack::new(id, *count)))
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default();
    Some(stacks)
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
