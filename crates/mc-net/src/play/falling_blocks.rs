use std::collections::HashSet;

use mc_data::block_facts::BlockFactsTable;
use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_data::loot::LootTables;
use mc_entity::{EntityId, EntityItemStack, Vec3};
use mc_world::{
    BlockPos, BlockRegistry, BlockStateId, ChunkPos, MAX_Y, MIN_Y, SECTION_DIM, WorldReadSnapshot,
};

use super::survival::{block_drop_stacks_with_facts_from, entity_item_stack};
use super::{
    AppliedBlockEdit, BlockEdit, BlockEditPrecondition, BlockPlanningRead, SnapshotPlanningWorld,
    SnapshotReadPrecondition,
};
use mc_data::block_semantics_26_1_2::passable_block_name;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LandedFallingBlock {
    pub id: EntityId,
    pub pos: BlockPos,
    pub state: BlockStateId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FallingBlockStart {
    pub(super) pos: BlockPos,
    pub(super) state: BlockStateId,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct FallingBlockStartPlan {
    pub(super) starts: Vec<FallingBlockStart>,
    pub(super) preconditions: Vec<BlockEditPrecondition>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct FallingBlockLandingPlacement {
    pub(super) id: EntityId,
    pub(super) edit: BlockEdit,
}

#[derive(Debug)]
pub(super) struct FallingBlockLandingDrop {
    pub(super) position: Vec3,
    pub(super) stack: EntityItemStack,
}

#[derive(Debug, Default)]
pub(super) struct FallingBlockLandingPlan {
    pub(super) placements: Vec<FallingBlockLandingPlacement>,
    pub(super) blocked_ids: Vec<EntityId>,
    pub(super) drops: Vec<FallingBlockLandingDrop>,
    pub(super) preconditions: Vec<SnapshotReadPrecondition>,
}

pub(super) fn falling_block_start_chunks(applied: &[AppliedBlockEdit]) -> Vec<ChunkPos> {
    let mut chunks = applied
        .iter()
        .map(|edit| ChunkPos {
            x: edit.pos.x.div_euclid(SECTION_DIM as i32),
            z: edit.pos.z.div_euclid(SECTION_DIM as i32),
        })
        .collect::<Vec<_>>();
    chunks.sort_unstable_by_key(|chunk| (chunk.x, chunk.z));
    chunks.dedup();
    chunks
}

pub(super) fn plan_falling_block_starts(
    blocks: &BlockRegistry,
    facts: &BlockFactsTable,
    storage: &impl BlockPlanningRead,
    applied: &[AppliedBlockEdit],
    air: BlockStateId,
) -> FallingBlockStartPlan {
    let mut seen = HashSet::new();
    let mut guarded = HashSet::new();
    let mut plan = FallingBlockStartPlan::default();
    for edit in applied {
        if !falling_block_can_enter(facts, edit.new_state, air) {
            continue;
        }
        let Some(source_state) = storage.get_cached_block(edit.pos) else {
            continue;
        };
        let Some(source_token) = storage.block_mutation_token(edit.pos) else {
            continue;
        };
        if source_state != edit.new_state {
            continue;
        }
        if guarded.insert(edit.pos) {
            plan.preconditions.push(BlockEditPrecondition {
                pos: edit.pos,
                expected_state: source_state,
                expected_token: source_token,
            });
        }
        let Some(mut y) = edit.pos.y.checked_add(1) else {
            continue;
        };
        loop {
            let pos = BlockPos { y, ..edit.pos };
            if !seen.insert(pos) {
                break;
            }
            let Some(state) = storage.get_cached_block(pos) else {
                break;
            };
            let Some(token) = storage.block_mutation_token(pos) else {
                break;
            };
            if guarded.insert(pos) {
                plan.preconditions.push(BlockEditPrecondition {
                    pos,
                    expected_state: state,
                    expected_token: token,
                });
            }
            if !is_falling_block_state(blocks, state) {
                break;
            }
            plan.starts.push(FallingBlockStart { pos, state });
            let Some(next_y) = y.checked_add(1) else {
                break;
            };
            y = next_y;
        }
    }
    plan
}

pub(super) fn is_falling_block_state(blocks: &BlockRegistry, state_id: BlockStateId) -> bool {
    blocks.by_id(state_id).is_some_and(|state| {
        matches!(
            state.block.id.path(),
            "sand" | "red_sand" | "gravel" | "anvil" | "chipped_anvil" | "damaged_anvil"
        )
    })
}

fn falling_block_can_enter(
    facts: &BlockFactsTable,
    state: BlockStateId,
    air: BlockStateId,
) -> bool {
    state == air || facts.fluid(state.0).is_some()
}

pub(super) fn falling_block_landing_chunks(candidates: &[LandedFallingBlock]) -> Vec<ChunkPos> {
    let mut chunks = candidates
        .iter()
        .filter(|candidate| (MIN_Y..MAX_Y).contains(&candidate.pos.y))
        .map(|candidate| ChunkPos {
            x: candidate.pos.x.div_euclid(SECTION_DIM as i32),
            z: candidate.pos.z.div_euclid(SECTION_DIM as i32),
        })
        .collect::<Vec<_>>();
    chunks.sort_unstable_by_key(|chunk| (chunk.x, chunk.z));
    chunks.dedup();
    chunks
}

pub(super) fn plan_falling_block_landings(
    loot: &LootTables,
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    blocks: &BlockRegistry,
    block_facts: &BlockFactsTable,
    snapshot: &WorldReadSnapshot,
    candidates: &[LandedFallingBlock],
) -> FallingBlockLandingPlan {
    let mut world = SnapshotPlanningWorld::new(snapshot);
    let mut plan = FallingBlockLandingPlan::default();
    for candidate in candidates {
        if !(MIN_Y..MAX_Y).contains(&candidate.pos.y) {
            continue;
        }
        let Some(current) = world.get_cached_block(candidate.pos) else {
            continue;
        };
        if falling_block_landing_cell_is_solid(block_facts, blocks, current) {
            plan.blocked_ids.push(candidate.id);
            plan.drops.extend(
                block_drop_stacks_with_facts_from(loot, items, item_facts, blocks, candidate.state)
                    .into_iter()
                    .map(|drop| FallingBlockLandingDrop {
                        position: Vec3::new(
                            f64::from(candidate.pos.x) + 0.5,
                            f64::from(candidate.pos.y) + 0.5,
                            f64::from(candidate.pos.z) + 0.5,
                        ),
                        stack: entity_item_stack(drop),
                    }),
            );
            continue;
        }
        let edit = BlockEdit {
            pos: candidate.pos,
            new_state: candidate.state,
        };
        if world.apply(edit) {
            plan.placements.push(FallingBlockLandingPlacement {
                id: candidate.id,
                edit,
            });
        }
    }
    plan.preconditions = world.preconditions();
    plan
}

fn falling_block_landing_cell_is_solid(
    block_facts: &BlockFactsTable,
    blocks: &BlockRegistry,
    state: BlockStateId,
) -> bool {
    if block_facts.fluid(state.0).is_some() {
        return false;
    }
    blocks
        .by_id(state)
        .is_some_and(|block_state| !passable_block_name(block_state.block.id.as_str()))
}
