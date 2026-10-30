//! Resource targets must recover through the real allocator and brain.
use super::{VillagerProfession, brain, mining_tests};
use crate::boundary::ChangeSet;
use crate::settlements::buildings::BuildingKind;
use crate::tick::TickState;

fn tree(state: &mut TickState, x: i32, y: i32, z: i32) {
    let dirt = mining_tests::block(state, "dirt");
    let log = mining_tests::block(state, "oak_log");
    let leaves = mining_tests::block(state, "oak_leaves");
    state.world.set_block(x, y - 1, z, dirt);
    for layer in 0..4 {
        state.world.set_block(x, y + layer, z, log);
    }
    state.world.set_block(x, y + 4, z, leaves);
}

#[test]
fn lumberjack_abandons_unreachable_tree_and_delivers_reachable_logs_through_brain() {
    let (mut state, slot, mut resident) = mining_tests::fixture(BuildingKind::LumberCamp);
    let settlement = resident.settlement.unwrap();
    state.settlements.get_mut(settlement).unwrap().radius = 32;
    state.entities.set_pos(slot, [2.5, 201.0, 3.5]);
    // Only the native allocator may acquire the resource profession/job.
    resident.settlement = None;
    resident.job_site = None;
    resident.data.profession = VillagerProfession::None;
    state.entities.row_mut(slot).villager = Some(resident);
    let id = state.entities.body(slot).id;
    crate::settlements::bind(&mut state, settlement, id).unwrap();
    // The first mature tree in sweep order is on an isolated high pillar.
    // There is no walkable approach within harvesting reach. A later source
    // on the ordinary ground is reachable without assistance or teleporting.
    tree(&mut state, 8, 215, 8);
    tree(&mut state, 12, 201, 12);
    let resident = state.entities.row(slot).villager.as_ref().unwrap().clone();
    mining_tests::storage(&mut state, &resident, [2, 201, 12]);
    state.world.fold_overlay();
    let oak = super::mining::item_id(&state, "oak_log").unwrap();
    let mut acquired = false;
    let mut saw_unreachable = false;
    for _ in 0..2000 {
        state.tick_budget.reset(state.tick);
        let mut changes = ChangeSet::default();
        crate::settlements::allocator::tick(&mut state, &mut changes);
        brain::tick(&mut state, &mut changes);
        let v = state.entities.row(slot).villager.as_ref().unwrap();
        acquired |= v.data.profession == VillagerProfession::Lumberjack;
        saw_unreachable |= v.lumber_progress.has_target && v.lumber_progress.base == [8, 215, 8];
        if v.inventory
            .iter()
            .flatten()
            .any(|stack| stack.item_id == oak)
        {
            break;
        }
        state.tick += 1;
    }
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    assert!(acquired, "real allocator assigns the lumber camp");
    assert!(
        saw_unreachable,
        "the regression exercises the inaccessible source"
    );
    assert!(
        v.inventory
            .iter()
            .flatten()
            .any(|stack| stack.item_id == oak),
        "a failed route must not pin the worker forever: pos={:?}, target={:?}, path={:?}",
        state.entities.pos(slot),
        v.lumber_progress,
        v.navigation_path
    );
    // End the work shift and prove an ordinary return to real storage.
    state.day_time = 9000;
    for _ in 0..1000 {
        state.tick_budget.reset(state.tick);
        let mut changes = ChangeSet::default();
        crate::settlements::allocator::tick(&mut state, &mut changes);
        brain::tick(&mut state, &mut changes);
        state.tick += 1;
        if state.block_entities[&(2, 201, 12)]
            .slots
            .iter()
            .any(|stack| stack.item_id == oak && stack.count > 0)
        {
            return;
        }
    }
    panic!("harvested logs must arrive through the real storage commit");
}

#[test]
fn lumberjack_keeps_reachable_tree_when_brain_path_budget_is_denied() {
    let (mut state, slot, mut resident) = mining_tests::fixture(BuildingKind::LumberCamp);
    let settlement = resident.settlement.unwrap();
    state.entities.set_pos(slot, [2.5, 201.0, 3.5]);
    resident.settlement = None;
    resident.job_site = None;
    resident.data.profession = VillagerProfession::None;
    state.entities.row_mut(slot).villager = Some(resident);
    let id = state.entities.body(slot).id;
    crate::settlements::bind(&mut state, settlement, id).unwrap();
    tree(&mut state, 12, 201, 12);
    state.world.fold_overlay();
    let mut acquired = false;
    for _ in 0..240 {
        state.tick_budget.reset(state.tick);
        let mut changes = ChangeSet::default();
        crate::settlements::allocator::tick(&mut state, &mut changes);
        // Admit a search, but exhaust it after the starting node. A wholly
        // denied search never records a route and would miss this regression.
        for dimension in crate::dimensions::Dimension::ALL {
            for consumer in [
                crate::tick_budget::Consumer::Allocator,
                crate::tick_budget::Consumer::Brain,
            ] {
                state.tick_budget.finish(dimension, consumer);
            }
        }
        let available = state
            .tick_budget
            .remaining(crate::tick_budget::Kind::PathNode);
        assert!(state.tick_budget.try_charge(
            state.dimension,
            crate::tick_budget::Consumer::Brain,
            crate::tick_budget::Kind::PathNode,
            available - 1,
        ));
        brain::tick(&mut state, &mut changes);
        let v = state.entities.row(slot).villager.as_ref().unwrap();
        if acquired {
            assert!(
                v.lumber_progress.has_target,
                "a deferred route is not an unreachable source"
            );
            assert_eq!(v.lumber_progress.base, [12, 201, 12]);
        }
        acquired |= v.lumber_progress.has_target;
        state.tick += 1;
    }
    assert!(
        acquired,
        "real allocator and brain acquire the resource role and tree"
    );
    let oak = super::mining::item_id(&state, "oak_log").unwrap();
    for _ in 0..1000 {
        state.tick_budget.reset(state.tick);
        let mut changes = ChangeSet::default();
        crate::settlements::allocator::tick(&mut state, &mut changes);
        brain::tick(&mut state, &mut changes);
        state.tick += 1;
        if state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .inventory
            .iter()
            .flatten()
            .any(|stack| stack.item_id == oak)
        {
            return;
        }
    }
    panic!("the retained source must be harvested when the path allowance returns");
}
