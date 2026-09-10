use std::sync::Arc;

use mc_data::Identifier;
use mc_protocol::packets::play::Direction;
use mc_world::{BlockPos, BlockRegistry, BlockStateId};

use super::super::PlayerPose;
use super::super::use_item_on_adapter::placement_snapshot_for_test;
use super::{chest, plan_block_placement, property};

fn blocks() -> Arc<BlockRegistry> {
    Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
    )
}

fn default(blocks: &BlockRegistry, name: &str) -> BlockStateId {
    blocks
        .block(&Identifier::parse(name).unwrap())
        .unwrap()
        .default
}

#[test]
fn adjacent_chest_placement_updates_both_halves_and_break_resets_partner() {
    let blocks = blocks();
    let air = default(&blocks, "minecraft:air");
    let single = default(&blocks, "minecraft:chest");
    let pos = BlockPos { x: 0, y: 64, z: 0 };
    let neighbor = BlockPos { x: 1, ..pos };
    let snapshot = placement_snapshot_for_test(Arc::clone(&blocks), &[(neighbor, single)]);
    let pose = PlayerPose::new(0.0, 64.0, 0.0);
    let plan = plan_block_placement(
        &blocks,
        single,
        Some(&snapshot),
        pos,
        pose,
        Direction::Up,
        0.5,
        air,
    )
    .unwrap();
    let first = plan
        .edits
        .iter()
        .find(|edit| edit.pos == pos)
        .unwrap()
        .new_state;
    let second = plan
        .edits
        .iter()
        .find(|edit| edit.pos == neighbor)
        .unwrap()
        .new_state;
    assert_eq!(property(blocks.by_id(first).unwrap(), "type"), Some("left"));
    assert_eq!(
        property(blocks.by_id(second).unwrap(), "type"),
        Some("right")
    );
    let paired =
        placement_snapshot_for_test(Arc::clone(&blocks), &[(pos, first), (neighbor, second)]);
    assert_eq!(
        chest::paired_position(&blocks, |p| paired.get_cached_block(p), pos, first),
        Some(neighbor)
    );
    assert_eq!(
        chest::paired_position(&blocks, |p| paired.get_cached_block(p), neighbor, second),
        Some(pos)
    );
    let reset = chest::reset_partner(&blocks, |p| paired.get_cached_block(p), pos, first).unwrap();
    assert_eq!(reset.pos, neighbor);
    assert_eq!(
        property(blocks.by_id(reset.new_state).unwrap(), "type"),
        Some("single")
    );
    let third = BlockPos { x: 2, ..pos };
    let plan = plan_block_placement(
        &blocks,
        single,
        Some(&paired),
        third,
        pose,
        Direction::Up,
        0.5,
        air,
    )
    .unwrap();
    assert_eq!(
        property(blocks.by_id(plan.edits[0].new_state).unwrap(), "type"),
        Some("single")
    );
    assert!(!plan.edits.iter().any(|edit| edit.pos == neighbor));
}

#[test]
fn secondary_use_on_ground_keeps_adjacent_chests_separate() {
    let blocks = blocks();
    let single = default(&blocks, "minecraft:chest");
    let air = default(&blocks, "minecraft:air");
    let pos = BlockPos { x: 0, y: 64, z: 0 };
    let neighbor = BlockPos { x: 1, ..pos };
    let snapshot = placement_snapshot_for_test(Arc::clone(&blocks), &[(neighbor, single)]);
    let pose = PlayerPose {
        shifting: true,
        ..PlayerPose::new(0.0, 64.0, 0.0)
    };
    let plan = plan_block_placement(
        &blocks,
        single,
        Some(&snapshot),
        pos,
        pose,
        Direction::Up,
        0.5,
        air,
    )
    .unwrap();
    assert_eq!(
        property(blocks.by_id(plan.edits[0].new_state).unwrap(), "type"),
        Some("single")
    );
    assert!(!plan.edits.iter().any(|edit| edit.pos == neighbor));
    let separate = placement_snapshot_for_test(
        Arc::clone(&blocks),
        &[(pos, plan.edits[0].new_state), (neighbor, single)],
    );
    assert_eq!(
        chest::paired_position(&blocks, |p| separate.get_cached_block(p), pos, single),
        None
    );
}

#[test]
fn mirrored_placement_order_pairs_with_complementary_types() {
    let blocks = blocks();
    let air = default(&blocks, "minecraft:air");
    let single = default(&blocks, "minecraft:chest");
    let pos = BlockPos { x: 0, y: 64, z: 0 };
    let neighbor = BlockPos { x: -1, ..pos };
    let snapshot =
        placement_snapshot_for_test(Arc::clone(&blocks), &[(pos, air), (neighbor, single)]);
    let pose = PlayerPose::new(0.0, 64.0, 0.0);
    let plan = plan_block_placement(
        &blocks,
        single,
        Some(&snapshot),
        pos,
        pose,
        Direction::Up,
        0.5,
        air,
    )
    .unwrap();
    assert_eq!(plan.edits.len(), 2);
    let first = plan
        .edits
        .iter()
        .find(|edit| edit.pos == pos)
        .unwrap()
        .new_state;
    let second = plan
        .edits
        .iter()
        .find(|edit| edit.pos == neighbor)
        .unwrap()
        .new_state;
    let first_type = property(blocks.by_id(first).unwrap(), "type");
    let second_type = property(blocks.by_id(second).unwrap(), "type");
    assert!(
        matches!(
            (first_type, second_type),
            (Some("left"), Some("right")) | (Some("right"), Some("left"))
        ),
        "mirrored order must pair with complementary types"
    );
    assert_eq!(
        property(blocks.by_id(first).unwrap(), "facing"),
        property(blocks.by_id(second).unwrap(), "facing")
    );
    let paired =
        placement_snapshot_for_test(Arc::clone(&blocks), &[(pos, first), (neighbor, second)]);
    assert_eq!(
        chest::paired_position(&blocks, |p| paired.get_cached_block(p), pos, first),
        Some(neighbor),
        "opening from the new half must find the partner"
    );
    assert_eq!(
        chest::paired_position(&blocks, |p| paired.get_cached_block(p), neighbor, second),
        Some(pos),
        "opening from the old half must find the partner"
    );
}

#[test]
fn unloaded_neighbor_places_single_instead_of_aborting() {
    let blocks = blocks();
    let air = default(&blocks, "minecraft:air");
    let single = default(&blocks, "minecraft:chest");
    let pos = BlockPos { x: 0, y: 64, z: 0 };
    // Only the placed cell's chunk is loaded; the west neighbor chunk is not.
    let snapshot = placement_snapshot_for_test(Arc::clone(&blocks), &[(pos, air)]);
    let pose = PlayerPose::new(0.0, 64.0, 0.0);
    let plan = plan_block_placement(
        &blocks,
        single,
        Some(&snapshot),
        pos,
        pose,
        Direction::Up,
        0.5,
        air,
    )
    .expect("unloaded neighbor must not abort placement");
    assert_eq!(plan.edits.len(), 1);
    assert_eq!(
        property(blocks.by_id(plan.edits[0].new_state).unwrap(), "type"),
        Some("single")
    );
}
