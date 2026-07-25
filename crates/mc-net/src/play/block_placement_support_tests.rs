use std::sync::Arc;

use mc_data::Identifier;
use mc_protocol::packets::play::Direction;
use mc_world::{BlockPos, BlockRegistry, BlockStateId};

use super::super::PlayerPose;
use super::super::use_item_on_adapter::{
    conditional_placement_rejects_test_mutation, placement_snapshot_for_test,
};
use super::plan_block_placement;

fn canonical_blocks() -> Arc<BlockRegistry> {
    Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
    )
}

fn blocks_with_remapped_stone_state() -> Arc<BlockRegistry> {
    let mut report = mc_data::blocks::solaris_required_blocks_report();
    let stone_index = report
        .iter()
        .position(|block| block.id.as_str() == "minecraft:stone")
        .expect("embedded stone block");
    let cobblestone_index = report
        .iter()
        .position(|block| block.id.as_str() == "minecraft:cobblestone")
        .expect("embedded cobblestone block");
    let stone_id = report[stone_index].states[0].id;
    let cobblestone_id = report[cobblestone_index].states[0].id;
    report[stone_index].states[0].id = cobblestone_id;
    report[cobblestone_index].states[0].id = stone_id;
    Arc::new(BlockRegistry::from_report(&report).unwrap())
}

fn block_state(blocks: &BlockRegistry, name: &str, properties: &[(&str, &str)]) -> BlockStateId {
    blocks
        .by_name_and_props(
            &Identifier::parse(name).unwrap(),
            &properties
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect::<Vec<_>>(),
        )
        .unwrap()
}

#[test]
fn standing_torch_uses_exact_full_cube_support() {
    let blocks = canonical_blocks();
    let air = blocks
        .block(&Identifier::parse("minecraft:air").unwrap())
        .unwrap()
        .default;
    let torch = blocks
        .block(&Identifier::parse("minecraft:torch").unwrap())
        .unwrap()
        .default;
    let stone = blocks
        .block(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default;
    let pos = BlockPos { x: 4, y: 64, z: 4 };
    let support = BlockPos { y: 63, ..pos };
    let snapshot = placement_snapshot_for_test(Arc::clone(&blocks), &[(support, stone)]);

    assert!(
        plan_block_placement(
            &blocks,
            torch,
            Some(&snapshot),
            pos,
            PlayerPose::new(4.5, 64.0, 4.5),
            Direction::Up,
            1.0,
            air,
        )
        .is_some()
    );
}

#[test]
fn standing_torch_rejects_name_only_full_cube_fallback() {
    let blocks = blocks_with_remapped_stone_state();
    let air = blocks
        .block(&Identifier::parse("minecraft:air").unwrap())
        .unwrap()
        .default;
    let torch = blocks
        .block(&Identifier::parse("minecraft:torch").unwrap())
        .unwrap()
        .default;
    let stone = blocks
        .block(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default;
    let pos = BlockPos { x: 4, y: 64, z: 4 };
    let support = BlockPos { y: 63, ..pos };
    let snapshot = placement_snapshot_for_test(Arc::clone(&blocks), &[(support, stone)]);

    assert!(
        plan_block_placement(
            &blocks,
            torch,
            Some(&snapshot),
            pos,
            PlayerPose::new(4.5, 64.0, 4.5),
            Direction::Up,
            1.0,
            air,
        )
        .is_none(),
        "a block name without an exact 26.1.2 state fingerprint must not become sturdy"
    );
}

#[test]
fn standing_torch_uses_full_up_face_of_top_slab() {
    let blocks = canonical_blocks();
    let air = blocks
        .block(&Identifier::parse("minecraft:air").unwrap())
        .unwrap()
        .default;
    let torch = blocks
        .block(&Identifier::parse("minecraft:torch").unwrap())
        .unwrap()
        .default;
    let top_slab = block_state(
        &blocks,
        "minecraft:oak_slab",
        &[("type", "top"), ("waterlogged", "false")],
    );
    let pos = BlockPos { x: 4, y: 64, z: 4 };
    let support = BlockPos { y: 63, ..pos };
    let snapshot = placement_snapshot_for_test(Arc::clone(&blocks), &[(support, top_slab)]);

    let plan = plan_block_placement(
        &blocks,
        torch,
        Some(&snapshot),
        pos,
        PlayerPose::new(4.5, 64.0, 4.5),
        Direction::Up,
        1.0,
        air,
    )
    .expect("a top slab has a full UP support face");

    assert_eq!(plan.edits[0].new_state, torch);
}

#[test]
fn stale_top_slab_support_rejects_standing_torch_placement() {
    let blocks = canonical_blocks();
    let air = blocks
        .block(&Identifier::parse("minecraft:air").unwrap())
        .unwrap()
        .default;
    let torch = blocks
        .block(&Identifier::parse("minecraft:torch").unwrap())
        .unwrap()
        .default;
    let top_slab = block_state(
        &blocks,
        "minecraft:oak_slab",
        &[("type", "top"), ("waterlogged", "false")],
    );
    let pos = BlockPos { x: 4, y: 64, z: 4 };
    let support = BlockPos { y: 63, ..pos };

    assert!(conditional_placement_rejects_test_mutation(
        Arc::clone(&blocks),
        &[(support, top_slab)],
        pos,
        (support, air),
        |snapshot| plan_block_placement(
            &blocks,
            torch,
            Some(snapshot),
            pos,
            PlayerPose::new(4.5, 64.0, 4.5),
            Direction::Up,
            1.0,
            air,
        ),
    ));
}

#[test]
fn standing_torch_rejects_partial_up_face_of_bottom_slab() {
    let blocks = canonical_blocks();
    let air = blocks
        .block(&Identifier::parse("minecraft:air").unwrap())
        .unwrap()
        .default;
    let torch = blocks
        .block(&Identifier::parse("minecraft:torch").unwrap())
        .unwrap()
        .default;
    let bottom_slab = block_state(
        &blocks,
        "minecraft:oak_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    );
    let pos = BlockPos { x: 4, y: 64, z: 4 };
    let support = BlockPos { y: 63, ..pos };
    let snapshot = placement_snapshot_for_test(Arc::clone(&blocks), &[(support, bottom_slab)]);

    assert!(
        plan_block_placement(
            &blocks,
            torch,
            Some(&snapshot),
            pos,
            PlayerPose::new(4.5, 64.0, 4.5),
            Direction::Up,
            1.0,
            air,
        )
        .is_none()
    );
}

#[test]
fn wall_torch_rejects_partial_side_face_of_top_slab() {
    let blocks = canonical_blocks();
    let air = blocks
        .block(&Identifier::parse("minecraft:air").unwrap())
        .unwrap()
        .default;
    let torch = blocks
        .block(&Identifier::parse("minecraft:torch").unwrap())
        .unwrap()
        .default;
    let top_slab = block_state(
        &blocks,
        "minecraft:oak_slab",
        &[("type", "top"), ("waterlogged", "false")],
    );
    let pos = BlockPos { x: 4, y: 64, z: 4 };
    let support = BlockPos { x: 5, ..pos };
    let snapshot = placement_snapshot_for_test(Arc::clone(&blocks), &[(support, top_slab)]);

    assert!(
        plan_block_placement(
            &blocks,
            torch,
            Some(&snapshot),
            pos,
            PlayerPose::new(4.5, 64.0, 4.5),
            Direction::West,
            0.5,
            air,
        )
        .is_none()
    );
}

#[test]
fn standing_torch_uses_full_up_face_of_top_stair() {
    let blocks = canonical_blocks();
    let air = blocks
        .block(&Identifier::parse("minecraft:air").unwrap())
        .unwrap()
        .default;
    let torch = blocks
        .block(&Identifier::parse("minecraft:torch").unwrap())
        .unwrap()
        .default;
    let top_stair = block_state(
        &blocks,
        "minecraft:oak_stairs",
        &[
            ("facing", "north"),
            ("half", "top"),
            ("shape", "straight"),
            ("waterlogged", "false"),
        ],
    );
    let pos = BlockPos { x: 4, y: 64, z: 4 };
    let support = BlockPos { y: 63, ..pos };
    let snapshot = placement_snapshot_for_test(Arc::clone(&blocks), &[(support, top_stair)]);

    assert!(
        plan_block_placement(
            &blocks,
            torch,
            Some(&snapshot),
            pos,
            PlayerPose::new(4.5, 64.0, 4.5),
            Direction::Up,
            1.0,
            air,
        )
        .is_some()
    );
}
