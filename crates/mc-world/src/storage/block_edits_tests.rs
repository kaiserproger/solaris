use std::sync::Arc;

use mc_data::Identifier;

use crate::block::BlockStateId;
use crate::chunk::{BlockPos, Chunk, ChunkPos, ScheduledBlockTick};
use crate::resident::{ResidentBlockEdit, ResidentBlockEditBatchResult, ResidentBlockPrecondition};

use super::super::test_support::*;
use super::*;

fn stone_tick(pos: BlockPos, trigger_tick: u64) -> ScheduledBlockTick {
    ScheduledBlockTick::new(
        pos,
        Identifier::parse("minecraft:stone").unwrap(),
        trigger_tick,
        0,
    )
}

#[test]
fn resident_opaque_block_entity_commit_rejects_stale_token() {
    let registry = air_stone_registry();
    let mut world = WorldStorage::in_memory(registry);
    let cpos = ChunkPos { x: 0, z: 0 };
    let position = BlockPos { x: 1, y: 2, z: 3 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    world.set_block_at(position, BlockStateId(1)).unwrap();
    let stale_token = world.block_mutation_token(position).unwrap();
    world.set_block_at(position, BlockStateId(0)).unwrap();
    world.set_block_at(position, BlockStateId(1)).unwrap();
    let current_token = world.block_mutation_token(position).unwrap();
    let bytes = vec![10, 0, 0, 0];

    assert!(
        !world
            .commit_opaque_block_entity_conditionally(
                position,
                BlockStateId(1),
                stale_token,
                bytes.clone(),
            )
            .unwrap()
    );
    assert!(
        !world
            .cached_chunk(cpos)
            .unwrap()
            .block_entities
            .contains_key(&position)
    );

    assert!(
        world
            .commit_opaque_block_entity_conditionally(
                position,
                BlockStateId(1),
                current_token,
                bytes.clone(),
            )
            .unwrap()
    );
    assert_eq!(
        world
            .cached_chunk(cpos)
            .unwrap()
            .block_entities
            .get(&position),
        Some(&bytes)
    );
}

#[test]
fn cross_region_invalid_height_rejects_before_publication() {
    let mut world = WorldStorage::in_memory(air_stone_registry());
    for position in [ChunkPos { x: 7, z: 0 }, ChunkPos { x: 8, z: 0 }] {
        world
            .insert_generated_chunk(
                position,
                Chunk::empty(
                    position,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
    }
    let west = BlockPos { x: 127, y: 0, z: 0 };
    let token = world.block_mutation_token(west).unwrap();
    let edits = [
        ResidentBlockEdit {
            pos: west,
            new_state: BlockStateId(1),
            preserve_light: false,
        },
        ResidentBlockEdit {
            pos: BlockPos {
                x: 128,
                y: crate::MAX_Y + 1,
                z: 0,
            },
            new_state: BlockStateId(1),
            preserve_light: false,
        },
    ];
    assert_eq!(
        world
            .apply_block_edits_conditionally(&edits, &[], &[stone_tick(west, 20)], None, None)
            .unwrap(),
        ResidentBlockEditBatchResult::Missing
    );
    assert_eq!(world.get_cached_block(west), Some(BlockStateId(0)));
    assert_eq!(world.block_mutation_token(west), Some(token));
    assert!(
        world
            .scheduled_block_ticks(ChunkPos { x: 7, z: 0 })
            .unwrap()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn single_region_commit_applies_edits_and_keeps_ticks_for_changed_positions() {
    let registry = air_stone_registry();
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(chunk_pos, Chunk::empty(chunk_pos, BlockStateId(0), biome))
        .unwrap();
    let changed = BlockPos { x: 1, y: 0, z: 1 };
    let unchanged = BlockPos { x: 2, y: 0, z: 1 };
    let changed_token = world.block_mutation_token(changed).unwrap();
    let unchanged_token = world.block_mutation_token(unchanged).unwrap();
    let changed_tick = stone_tick(changed, 20);
    let unchanged_tick = stone_tick(unchanged, 20);

    let result = world
        .apply_block_edits_conditionally(
            &[
                ResidentBlockEdit {
                    pos: changed,
                    new_state: BlockStateId(1),
                    preserve_light: false,
                },
                ResidentBlockEdit {
                    pos: unchanged,
                    new_state: BlockStateId(0),
                    preserve_light: false,
                },
            ],
            &[
                ResidentBlockPrecondition {
                    pos: changed,
                    expected_state: BlockStateId(0),
                    expected_token: changed_token,
                },
                ResidentBlockPrecondition {
                    pos: unchanged,
                    expected_state: BlockStateId(0),
                    expected_token: unchanged_token,
                },
            ],
            &[changed_tick.clone(), unchanged_tick],
            None,
            None,
        )
        .unwrap();

    let ResidentBlockEditBatchResult::Applied(applied) = result else {
        panic!("single-region storage commit applies");
    };
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].pos, changed);
    assert_eq!(world.get_cached_block(changed), Some(BlockStateId(1)));
    assert_eq!(world.get_cached_block(unchanged), Some(BlockStateId(0)));
    assert_ne!(world.block_mutation_token(changed).unwrap(), changed_token);
    assert_eq!(
        world.scheduled_block_ticks(chunk_pos).unwrap().unwrap(),
        &[changed_tick]
    );
}

#[test]
fn cross_region_commit_applies_both_sides_atomically() {
    let registry = air_stone_registry();
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let west_chunk = ChunkPos { x: 7, z: 0 };
    let east_chunk = ChunkPos { x: 8, z: 0 };
    for chunk_pos in [west_chunk, east_chunk] {
        world
            .insert_generated_chunk(
                chunk_pos,
                Chunk::empty(chunk_pos, BlockStateId(0), biome.clone()),
            )
            .unwrap();
    }
    let west = BlockPos { x: 127, y: 2, z: 3 };
    let east = BlockPos { x: 128, y: 2, z: 3 };
    world.set_block_at(west, BlockStateId(1)).unwrap();
    world.set_block_at(east, BlockStateId(1)).unwrap();
    let west_token = world.block_mutation_token(west).unwrap();
    let east_token = world.block_mutation_token(east).unwrap();
    let west_tick = stone_tick(west, 20);

    let result = world
        .apply_block_edits_conditionally(
            &[
                ResidentBlockEdit {
                    pos: west,
                    new_state: BlockStateId(0),
                    preserve_light: true,
                },
                ResidentBlockEdit {
                    pos: east,
                    new_state: BlockStateId(0),
                    preserve_light: true,
                },
            ],
            &[
                ResidentBlockPrecondition {
                    pos: west,
                    expected_state: BlockStateId(1),
                    expected_token: west_token,
                },
                ResidentBlockPrecondition {
                    pos: east,
                    expected_state: BlockStateId(1),
                    expected_token: east_token,
                },
            ],
            std::slice::from_ref(&west_tick),
            None,
            None,
        )
        .unwrap();

    let ResidentBlockEditBatchResult::Applied(applied) = result else {
        panic!("cross-region storage commit applies");
    };
    assert_eq!(applied.len(), 2);
    assert_eq!(world.get_cached_block(west), Some(BlockStateId(0)));
    assert_eq!(world.get_cached_block(east), Some(BlockStateId(0)));
    assert_ne!(world.block_mutation_token(west).unwrap(), west_token);
    assert_ne!(world.block_mutation_token(east).unwrap(), east_token);
    assert_eq!(
        world.scheduled_block_ticks(west_chunk).unwrap().unwrap(),
        &[west_tick]
    );
    assert!(
        world
            .scheduled_block_ticks(east_chunk)
            .unwrap()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn stale_token_rejects_whole_batch_without_partial_mutation() {
    let registry = air_stone_registry();
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(chunk_pos, Chunk::empty(chunk_pos, BlockStateId(0), biome))
        .unwrap();
    let first = BlockPos { x: 1, y: 0, z: 1 };
    let second = BlockPos { x: 2, y: 0, z: 1 };
    let first_token = world.block_mutation_token(first).unwrap();
    let stale_token = world.block_mutation_token(second).unwrap();
    // ABA: the state reads the same but the token advanced underneath.
    world.set_block_at(second, BlockStateId(1)).unwrap();
    world.set_block_at(second, BlockStateId(0)).unwrap();
    let advanced_token = world.block_mutation_token(second).unwrap();
    assert_ne!(advanced_token, stale_token);
    let first_tick = stone_tick(first, 20);

    let result = world
        .apply_block_edits_conditionally(
            &[
                ResidentBlockEdit {
                    pos: first,
                    new_state: BlockStateId(1),
                    preserve_light: false,
                },
                ResidentBlockEdit {
                    pos: second,
                    new_state: BlockStateId(1),
                    preserve_light: false,
                },
            ],
            &[
                ResidentBlockPrecondition {
                    pos: first,
                    expected_state: BlockStateId(0),
                    expected_token: first_token,
                },
                ResidentBlockPrecondition {
                    pos: second,
                    expected_state: BlockStateId(0),
                    expected_token: stale_token,
                },
            ],
            &[first_tick],
            None,
            None,
        )
        .unwrap();

    assert_eq!(result, ResidentBlockEditBatchResult::Stale);
    assert_eq!(world.get_cached_block(first), Some(BlockStateId(0)));
    assert_eq!(world.get_cached_block(second), Some(BlockStateId(0)));
    assert_eq!(world.block_mutation_token(first).unwrap(), first_token);
    assert_eq!(world.block_mutation_token(second).unwrap(), advanced_token);
    assert!(
        world
            .scheduled_block_ticks(chunk_pos)
            .unwrap()
            .unwrap()
            .is_empty()
    );
}
