use super::*;
use crate::boundary::{BlockPos, ChangeSet};
use crate::entities::EntitySlot;
use crate::settlements::allocator::tests::{block, test_state, villager};
use crate::villager::{Activity, building};
use mc_data::ItemStack;

pub(crate) fn fixture() -> (TickState, SettlementId, EntitySlot) {
    let mut state = test_state();
    for x in 0..16 {
        for z in 0..16 {
            for y in 201..224 {
                state.world.set_block(x, y, z, state.registry.states.air);
            }
        }
    }
    let id = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "builders".into(),
            "owner".into(),
            [8, 201, 8],
            48,
        )
        .unwrap();
    let chest = block(&state, "minecraft:chest", &[]);
    state.world.set_block(1, 201, 1, chest);
    state.block_entities.insert(
        (1, 201, 1),
        crate::tick::BlockEntityState {
            slots: vec![ItemStack::EMPTY; 27],
            furnace: None,
        },
    );
    state.settlements.get_mut(id).unwrap().bootstrap_storage = Some([1, 201, 1]);
    // Beside the storage chest, never inside its solid cell.
    let slot = villager(&mut state, [1.5, 201.0, 2.5]);
    let entity = state.entities.body(slot).id;
    crate::settlements::bind(&mut state, id, entity).unwrap();
    let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
    v.data.profession = Profession::Builder;
    v.activity = Activity::Work;
    state.day_time = 3000;
    state.world.fold_overlay();
    (state, id, slot)
}

pub(crate) fn start(
    state: &mut TickState,
    id: SettlementId,
    blueprint: &str,
    level: u8,
    building: Option<BuildingId>,
) {
    assert!(begin(
        state,
        Project {
            settlement: id,
            blueprint: blueprint.into(),
            level,
            origin: [3, 200, 3],
            rotation: 0,
            building,
            structure: String::new(),
            ordered: true,
        },
        "oak",
    ));
}

pub(crate) fn fund(state: &mut TickState, blueprint: &str, level: u8) -> Vec<(u32, u32)> {
    fund_at(state, (1, 201, 1), blueprint, level)
}

pub(crate) fn fund_at(
    state: &mut TickState,
    chest: (i32, i32, i32),
    blueprint: &str,
    level: u8,
) -> Vec<(u32, u32)> {
    let materials = blueprints::get(&state.registry, blueprint)
        .unwrap()
        .level(level)
        .unwrap()
        .materials
        .clone();
    let slots = &mut state.block_entities.get_mut(&chest).unwrap().slots;
    let mut index = 0;
    for (item, quantity) in &materials {
        let mut quantity = *quantity;
        while quantity > 0 {
            let count = quantity.min(64);
            slots[index] = ItemStack::new(*item, count as i32);
            quantity -= count;
            index += 1;
        }
    }
    materials
}

pub(crate) fn attempt(state: &mut TickState, id: SettlementId, slot: EntitySlot) {
    let mut v = state.entities.row_mut(slot).villager.take().unwrap();
    // A container is approached, never entered: stand on its free neighbour.
    let target = building::work(state, slot, &mut v, &mut ChangeSet::default()).map(|target| {
        if target.exact {
            target.pos
        } else {
            BlockPos {
                z: target.pos.z + 1,
                ..target.pos
            }
        }
    });
    state.entities.row_mut(slot).villager = Some(v);
    if let Some(BlockPos { x, y, z }) = target {
        state
            .entities
            .set_pos(slot, [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5]);
    }
    state.tick += 20;
    building::tick(state, id);
}

pub(crate) fn complete(state: &mut TickState, id: SettlementId, slot: EntitySlot) {
    for _ in 0..4096 {
        if state.settlements.get(id).unwrap().project.is_none() {
            return;
        }
        attempt(state, id, slot);
    }
    let project = state.settlements.get(id).unwrap().project.as_ref().unwrap();
    panic!(
        "construction did not finish: {:?}; next={:?}; material={:?}; builder={:?}; carry={:?}",
        project_status(state, id),
        executor::next_cell(state, id, &project.structure),
        executor::next_material(state, id, &project.structure),
        state.entities.pos(slot),
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .inventory,
    );
}

#[test]
fn storage_uses_available_acacia_logs_for_real_acacia_blocks() {
    let (mut state, id, slot) = fixture();
    let item = |name| {
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .unwrap()
    };
    let acacia = item("minecraft:acacia_log");
    let cobble = item("minecraft:cobblestone");
    let chest = &mut state.block_entities.get_mut(&(1, 201, 1)).unwrap().slots;
    chest[0] = ItemStack::new(acacia, 64);
    chest[1] = ItemStack::new(acacia, 64);
    chest[2] = ItemStack::new(cobble, 64);
    plan(&mut state, id);
    let project = state
        .settlements
        .get(id)
        .unwrap()
        .project
        .as_ref()
        .expect("acacia stock funds storage");
    assert_eq!(project.blueprint, "storage");
    let origin = project.origin;
    complete(&mut state, id, slot);
    let placed = state
        .world
        .block_at(origin[0], origin[1] + 1, origin[2])
        .unwrap();
    assert_eq!(
        state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(placed))
            .unwrap()
            .block
            .id
            .path(),
        "acacia_log"
    );
}

#[test]
fn funded_oak_storage_survives_larger_acacia_stack_and_stock_changes() {
    for ordered in [false, true] {
        let (mut state, id, slot) = fixture();
        fund(&mut state, "storage", 1);
        let acacia_planks = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:acacia_planks").unwrap())
            .unwrap();
        let acacia_log = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:acacia_log").unwrap())
            .unwrap();
        state.block_entities.get_mut(&(1, 201, 1)).unwrap().slots[26] =
            ItemStack::new(acacia_planks, 64);
        state.block_entities.get_mut(&(1, 201, 1)).unwrap().slots[25] =
            ItemStack::new(acacia_planks, 64);
        if ordered {
            request_build(&mut state, id, "storage").expect("oak bill funds owner order");
        } else {
            plan(&mut state, id);
        }
        let project = state
            .settlements
            .get(id)
            .unwrap()
            .project
            .as_ref()
            .expect("oak bill funds storage despite acacia stock");
        assert_eq!(project.blueprint, "storage");
        let origin = project.origin;
        state.block_entities.get_mut(&(1, 201, 1)).unwrap().slots[25] =
            ItemStack::new(acacia_log, 64);
        complete(&mut state, id, slot);
        let placed = state
            .world
            .block_at(origin[0], origin[1] + 1, origin[2])
            .unwrap();
        assert_eq!(
            state
                .registry
                .blocks
                .by_id(mc_world::BlockStateId(placed))
                .unwrap()
                .block
                .id
                .path(),
            "oak_log"
        );
    }
}

#[test]
fn planner_prioritizes_storage_then_housing() {
    let (mut state, id, slot) = fixture();
    let extra = crate::boundary::ChunkPos { x: 1, z: 0 };
    state
        .world
        .ensure_blocking(&rustc_hash::FxHashMap::from_iter([(extra, 0)]));
    state
        .tickets
        .insert("construction-extra".into(), (extra, 0));
    state.refresh_active_chunks(0);
    for x in 16..32 {
        for z in 0..16 {
            state
                .world
                .set_block(x, 200, z, state.registry.states.stone);
            for y in 201..224 {
                state.world.set_block(x, y, z, state.registry.states.air);
            }
        }
    }
    assert_eq!(
        needed_kinds(state.settlements.get(id).unwrap(), 1).next(),
        Some(BuildingKind::Storage)
    );
    fund(&mut state, "storage", 1);
    for _ in 0..128 {
        plan(&mut state, id);
        if state.settlements.get(id).unwrap().project.is_some() {
            break;
        }
    }
    assert_eq!(
        state
            .settlements
            .get(id)
            .unwrap()
            .project
            .as_ref()
            .unwrap()
            .blueprint,
        "storage"
    );
    complete(&mut state, id, slot);
    assert_eq!(
        needed_kinds(state.settlements.get(id).unwrap(), 1).next(),
        Some(BuildingKind::House)
    );
    fund(&mut state, "house", 1);
    for _ in 0..4096 {
        plan(&mut state, id);
        if state.settlements.get(id).unwrap().project.is_some() {
            break;
        }
    }
    assert_eq!(
        state
            .settlements
            .get(id)
            .unwrap()
            .project
            .as_ref()
            .unwrap()
            .blueprint,
        "house"
    );
}

#[test]
fn site_selection_rejects_overlap_steep_paths_and_unloaded() {
    let (mut state, id, _) = fixture();
    assert!(site_free(
        &state,
        state.settlements.get(id).unwrap(),
        [5, 200, 5],
        [3, 4, 3]
    ));
    state.world.set_block(6, 200, 6, state.registry.states.air);
    assert!(!site_free(
        &state,
        state.settlements.get(id).unwrap(),
        [5, 200, 5],
        [3, 4, 3]
    ));
    let path = block(&state, "minecraft:dirt_path", &[]);
    state.world.set_block(6, 200, 6, path);
    assert!(!site_free(
        &state,
        state.settlements.get(id).unwrap(),
        [5, 200, 5],
        [3, 4, 3]
    ));
    state
        .world
        .set_block(6, 200, 6, state.registry.states.stone);
    let building_id = BuildingId((id.0 << 32) | 1);
    let record = state.settlements.get_mut(id).unwrap();
    record.buildings.insert(
        building_id,
        buildings::Building {
            id: building_id,
            settlement: id,
            kind: BuildingKind::Storage,
            level: 1,
            bounds: ScriptWorldBounds::new([4, 200, 4], [8, 204, 8]).unwrap(),
            work_points: vec![[4, 201, 4]],
            revision: 1,
            natural_zone: false,
        },
    );
    assert!(!site_free(
        &state,
        state.settlements.get(id).unwrap(),
        [5, 200, 5],
        [3, 4, 3]
    ));
    assert!(!site_free(
        &state,
        state.settlements.get(id).unwrap(),
        [40, 200, 40],
        [3, 4, 3]
    ));
}

#[test]
fn native_site_clearance_refuses_fluid_states_and_paid_or_authored_blocks() {
    let (mut state, id, _) = fixture();
    for name in [
        "water",
        "bubble_column",
        "seagrass",
        "tall_seagrass",
        "lava",
        "wheat",
        "light",
        "structure_void",
    ] {
        let block = block(&state, &format!("minecraft:{name}"), &[]);
        state.world.set_block(6, 201, 6, block);
        assert!(
            !site_free(
                &state,
                state.settlements.get(id).unwrap(),
                [5, 200, 5],
                [3, 4, 3]
            ),
            "construction must not clear {name}"
        );
    }
}

#[test]
fn replaceable_site_vegetation_is_cleared_only_by_shared_journaled_work() {
    let (mut state, id, _slot) = fixture();
    let grass = block(&state, "minecraft:short_grass", &[]);
    state.world.set_block(4, 201, 4, grass);
    let size = blueprints::get(&state.registry, "house")
        .unwrap()
        .footprint(Rotation::from_quarter_turns(0));
    assert!(site_free(
        &state,
        state.settlements.get(id).unwrap(),
        [3, 200, 3],
        size
    ));
    start(&mut state, id, "house", 1, None);
    assert_eq!(
        state.world.block_at(4, 201, 4),
        Some(grass),
        "planning never mutates blocks"
    );
    building::tick(&mut state, id);
    assert_eq!(
        state.world.block_at(4, 201, 4),
        Some(state.registry.states.air)
    );
    assert_eq!(state.plugin_block_writes.len(), 1);
    assert_eq!(
        state.plugin_block_writes[0].0,
        BlockPos { x: 4, y: 201, z: 4 }
    );
}

#[test]
fn builder_hauls_before_shared_executor_debits_and_journals_a_single_cell() {
    let (mut state, id, slot) = fixture();
    start(&mut state, id, "house", 1, None);
    fund(&mut state, "house", 1);
    let project = state.settlements.get(id).unwrap().project.as_ref().unwrap();
    let item = executor::next_material(&state, id, &project.structure)
        .unwrap()
        .unwrap()
        .0;
    let needed = blueprints::get(&state.registry, "house")
        .unwrap()
        .level(1)
        .unwrap()
        .materials
        .iter()
        .find(|(candidate, _)| *candidate == item)
        .unwrap()
        .1 as i32;
    // More physical stock than this project needs must stay in storage.
    state
        .block_entities
        .get_mut(&(1, 201, 1))
        .unwrap()
        .slots
        .iter_mut()
        .find(|stack| stack.item_id == item && stack.count > 0)
        .unwrap()
        .count = 64;
    let stock = |state: &TickState| {
        state.block_entities[&(1, 201, 1)]
            .slots
            .iter()
            .map(|stack| stack.count)
            .sum::<i32>()
    };
    let before = stock(&state);
    building::tick(&mut state, id);
    let hauled = before - stock(&state);
    assert!(
        hauled >= needed.min(64),
        "the haul covers at least the next material"
    );
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .inventory
            .iter()
            .flatten()
            .map(|stack| stack.count)
            .sum::<i32>(),
        hauled
    );
    assert!(
        state.plugin_block_writes.is_empty(),
        "hauling never places a block"
    );
    building::tick(&mut state, id);
    assert_eq!(
        stock(&state),
        before - hauled,
        "placement cannot debit storage twice"
    );
    assert_eq!(
        state.plugin_block_writes.len(),
        1,
        "shared one-cell advance"
    );
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .inventory
            .iter()
            .flatten()
            .map(|stack| stack.count)
            .sum::<i32>(),
        hauled - 1,
        "one cell debits one item, retaining the real remaining batch"
    );
}

#[test]
fn builder_haul_fits_the_real_carry_and_moves_exactly_what_it_loads() {
    let (mut state, id, slot) = fixture();
    start(&mut state, id, "house", 1, None);
    fund(&mut state, "house", 1);
    let stock = |state: &TickState| {
        state.block_entities[&(1, 201, 1)]
            .slots
            .iter()
            .map(|stack| stack.count)
            .sum::<i32>()
    };
    let before = stock(&state);
    building::tick(&mut state, id);
    let carry = &state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .unwrap()
        .inventory;
    assert_eq!(carry.len(), 8, "villager carry is eight real slots");
    assert!(
        carry
            .iter()
            .flatten()
            .all(|stack| (1..=64).contains(&stack.count)),
        "every carried stack is a legal real stack"
    );
    let carried: i32 = carry.iter().flatten().map(|stack| stack.count).sum();
    // Crafting surplus stays in storage, so storage gives up at least what moved.
    assert!(carried > 0 && stock(&state) <= before - carried);
    assert!(
        state.plugin_block_writes.is_empty(),
        "a haul is not placement"
    );
}

#[test]
fn builder_stalls_without_stock_then_consumes_exact_materials_in_order_and_certifies() {
    let (mut state, id, slot) = fixture();
    start(&mut state, id, "house", 1, None);
    let project = state.settlements.get(id).unwrap().project.clone().unwrap();
    let (_, planned) = planned_cells(&state, &project, "oak").unwrap();
    let before = executor::status(&state, id, &project.structure).unwrap();
    for _ in 0..4 {
        attempt(&mut state, id, slot);
    }
    assert_eq!(
        executor::status(&state, id, &project.structure).unwrap(),
        before
    );
    assert!(state.plugin_block_writes.is_empty());
    assert!(
        project_status(&state, id)
            .iter()
            .any(|line| line == "Missing materials:")
    );
    let materials = fund(&mut state, "house", 1);
    complete(&mut state, id, slot);
    assert_eq!(state.plugin_block_writes.len(), planned.len());
    for ((position, block), expected) in state.plugin_block_writes.iter().zip(planned) {
        assert_eq!([position.x, position.y, position.z], expected.at);
        assert_eq!(*block, expected.state);
    }
    let record = state.settlements.get(id).unwrap();
    assert!(
        record
            .buildings
            .values()
            .any(|building| building.kind == BuildingKind::House && building.level == 1)
    );
    let snapshot = executor::status(&state, id, &project.structure).unwrap();
    assert_eq!(snapshot.state, ScriptStructureState::Committed);
    for (item, count) in materials {
        let name = state.registry.items.name_of(item).unwrap().to_string();
        assert_eq!(
            snapshot
                .consumed
                .iter()
                .find(|material| material.resource == name)
                .unwrap()
                .quantity,
            u64::from(count)
        );
    }
    assert!(
        state.block_entities[&(1, 201, 1)]
            .slots
            .iter()
            .all(ItemStack::is_empty)
    );
    assert!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .inventory
            .iter()
            .all(Option::is_none)
    );
}

#[test]
fn upgrade_uses_diff_and_certifies_level_two() {
    let (mut state, id, slot) = fixture();
    start(&mut state, id, "house", 1, None);
    fund(&mut state, "house", 1);
    complete(&mut state, id, slot);
    let building = *state
        .settlements
        .get(id)
        .unwrap()
        .buildings
        .keys()
        .next()
        .unwrap();
    let old_writes = state.plugin_block_writes.len();
    start(&mut state, id, "house", 2, Some(building));
    let project = state.settlements.get(id).unwrap().project.clone().unwrap();
    let (_, planned) = planned_cells(&state, &project, "oak").unwrap();
    fund(&mut state, "house", 2);
    complete(&mut state, id, slot);
    assert_eq!(
        state.settlements.get(id).unwrap().buildings[&building].level,
        2
    );
    assert_eq!(state.plugin_block_writes.len() - old_writes, planned.len());
}

#[test]
fn mid_project_checkpoint_restores_handle_native_progress_carry_and_resumes() {
    let (mut state, id, slot) = fixture();
    start(&mut state, id, "house", 1, None);
    fund(&mut state, "house", 1);
    for _ in 0..5 {
        attempt(&mut state, id, slot);
    }
    let project = state.settlements.get(id).unwrap().project.clone().unwrap();
    let snapshot = executor::status(&state, id, &project.structure).unwrap();
    assert!(snapshot.completed_work_units > 0);
    let rows = super::super::persistence::checkpoint_rows(&state, state.tick).unwrap();
    let ledger = serde_json::to_vec(
        state
            .plugins
            .get_domain("@native-structures", "ledger")
            .unwrap(),
    )
    .unwrap();
    let carry = serde_json::to_vec(state.entities.row(slot).villager.as_ref().unwrap()).unwrap();
    let (mut restored, restored_id, restored_slot) = fixture();
    assert_eq!(restored_id, id);
    restored.settlements = super::super::Registry::default();
    super::super::persistence::hydrate_dimension(
        &mut restored,
        &rows,
        &crate::journal::Replay::default(),
        id.0,
    )
    .unwrap();
    restored.plugins.set_domain(
        "@native-structures",
        "ledger",
        serde_json::from_slice(&ledger).unwrap(),
    );
    restored.entities.row_mut(restored_slot).villager =
        Some(serde_json::from_slice(&carry).unwrap());
    restored.block_entities.get_mut(&(1, 201, 1)).unwrap().slots =
        state.block_entities[&(1, 201, 1)].slots.clone();
    for (position, block) in &state.plugin_block_writes {
        restored
            .world
            .set_block(position.x, position.y, position.z, *block);
    }
    assert_eq!(
        executor::status(&restored, id, &project.structure).unwrap(),
        snapshot
    );
    complete(&mut restored, id, restored_slot);
    assert!(restored.settlements.get(id).unwrap().project.is_none());
    assert!(
        restored.block_entities[&(1, 201, 1)]
            .slots
            .iter()
            .all(ItemStack::is_empty)
    );
}

#[test]
fn builder_crafts_missing_door_from_real_stock_and_keeps_vanilla_surplus() {
    let (mut state, id, slot) = fixture();
    start(&mut state, id, "house", 1, None);
    let materials = fund(&mut state, "house", 1);
    let item = |state: &TickState, name| {
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .unwrap()
    };
    let door = item(&state, "minecraft:oak_door");
    let log = item(&state, "minecraft:oak_log");
    let planks = item(&state, "minecraft:oak_planks");
    let doors_needed = materials.iter().find(|(id, _)| *id == door).unwrap().1;
    assert!(doors_needed <= 3);
    let stock = &mut state.block_entities.get_mut(&(1, 201, 1)).unwrap().slots;
    for stack in &mut *stock {
        if stack.item_id == door {
            *stack = ItemStack::EMPTY;
        }
    }
    let empty = stock.iter_mut().find(|stack| stack.is_empty()).unwrap();
    *empty = ItemStack::new(log, 2);
    complete(&mut state, id, slot);
    let stock = &state.block_entities[&(1, 201, 1)].slots;
    let count = |item| {
        stock
            .iter()
            .filter(|stack| stack.item_id == item)
            .map(|stack| stack.count)
            .sum::<i32>()
    };
    assert_eq!(count(door), 3 - doors_needed as i32);
    assert_eq!(count(planks), 2);
    assert_eq!(count(log), 0);
}

#[test]
fn removing_settlement_cancels_canonical_work_without_destroying_carried_materials() {
    let (mut state, id, slot) = fixture();
    start(&mut state, id, "house", 1, None);
    fund(&mut state, "house", 1);
    let project = state.settlements.get(id).unwrap().project.clone().unwrap();
    building::tick(&mut state, id);
    let carry = state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .unwrap()
        .inventory
        .clone();
    assert!(carry.iter().any(Option::is_some));
    assert!(crate::settlements::remove(&mut state, id));
    let resident = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(resident.inventory, carry);
    assert_eq!(resident.data.profession, Profession::None);
    assert_eq!(resident.settlement, None);
    assert_eq!(
        executor::status(&state, id, &project.structure)
            .unwrap()
            .state,
        ScriptStructureState::Cancelled
    );
    let size = blueprints::get(&state.registry, "house")
        .unwrap()
        .footprint(Rotation::from_quarter_turns(0));
    let bounds = ScriptWorldBounds::new(
        project.origin,
        std::array::from_fn(|axis| project.origin[axis] + i32::from(size[axis]) - 1),
    )
    .unwrap();
    assert!(!executor::area_reserved(&state, bounds));
    assert!(state.plugin_block_writes.is_empty());
}

#[test]
fn native_material_mappings_and_double_slabs_pass_shared_startup_validation() {
    for blueprint in ["house", "farm", "lumber_camp"] {
        let (mut state, id, _slot) = fixture();
        start(&mut state, id, blueprint, 1, None);
        crate::plugin_structures::validate(&state).expect("prepared native sources must reopen");
    }
}

#[test]
fn native_clear_and_build_stages_validate_before_and_after_a_restart_boundary() {
    let (mut state, id, slot) = fixture();
    let grass = block(&state, "minecraft:short_grass", &[]);
    state.world.set_block(4, 201, 4, grass);
    start(&mut state, id, "house", 1, None);
    let project = state.settlements.get(id).unwrap().project.clone().unwrap();
    crate::plugin_structures::validate(&state).expect("clear+build stages have unique cells");
    building::tick(&mut state, id);
    crate::plugin_structures::validate(&state)
        .expect("cleared stage keeps future material accounting");
    let snapshot = executor::status(&state, id, &project.structure).unwrap();
    assert_eq!(snapshot.current_stage_index, 1);
    fund(&mut state, "house", 1);
    complete(&mut state, id, slot);
    crate::plugin_structures::validate(&state).expect("committed native source must reopen");
}

#[test]
fn builder_places_carried_cells_on_four_tick_admissions() {
    let (mut state, id, slot) = fixture();
    start(&mut state, id, "storage", 1, None);
    fund(&mut state, "storage", 1);
    let handle = state
        .settlements
        .get(id)
        .unwrap()
        .project
        .as_ref()
        .unwrap()
        .structure
        .clone();
    let (item, quantity) = executor::next_material(&state, id, &handle)
        .unwrap()
        .unwrap();
    // The initial admission fetches a real stack, but never also places.
    tick(&mut state);
    let carry = |state: &TickState| {
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .inventory
            .iter()
            .flatten()
            .filter(|stack| stack.item_id == item)
            .map(|stack| stack.count)
            .sum::<i32>()
    };
    let fetched = carry(&state);
    assert!(fetched >= (quantity * 2) as i32);
    let stock = state.block_entities[&(1, 201, 1)].slots.clone();
    for now in 1..=8 {
        state.tick = now;
        tick(&mut state);
        let placed = now / 4;
        assert_eq!(
            executor::status(&state, id, &handle)
                .unwrap()
                .completed_work_units,
            placed,
            "one canonical cell on each four-tick admission, tick {now}",
        );
        assert_eq!(carry(&state), fetched - (placed as u32 * quantity) as i32);
        assert_eq!(state.block_entities[&(1, 201, 1)].slots, stock);
    }
}

#[test]
fn builder_leaves_future_footprint_before_first_placement() {
    let (mut state, id, slot) = fixture();
    start(&mut state, id, "storage", 1, None);
    fund(&mut state, "storage", 1);
    let project = state
        .settlements
        .get(id)
        .unwrap()
        .project
        .as_ref()
        .unwrap()
        .clone();
    let size = blueprints::get(&state.registry, &project.blueprint)
        .unwrap()
        .footprint(blueprints::Rotation::from_quarter_turns(project.rotation));
    let outside = |state: &TickState| {
        let pos = state.entities.pos(slot);
        let width = state.entities.body(slot).half_width;
        [0, 2].into_iter().any(|axis| {
            pos[axis] + width <= f64::from(project.origin[axis])
                || pos[axis] - width >= f64::from(project.origin[axis] + i32::from(size[axis]))
        })
    };
    // Fetch actual project stock, then model a builder still inside the empty
    // future site when the next construction admission arrives.
    tick(&mut state);
    state.entities.set_pos(slot, [6.5, 201.0, 6.5]);
    assert!(!outside(&state));
    state.tick = 4;
    tick(&mut state);
    assert_eq!(
        executor::status(&state, id, &project.structure)
            .unwrap()
            .completed_work_units,
        0,
        "carrying materials and being in reach must not place through the future site occupant"
    );
    for now in 5..1000 {
        state.tick = now;
        state.tick_budget.reset(now);
        let mut changes = ChangeSet::default();
        crate::settlements::tick(&mut state, &mut changes);
        if executor::status(&state, id, &project.structure)
            .unwrap()
            .completed_work_units
            > 0
        {
            assert!(
                outside(&state),
                "first placement requires the full builder body outside the future footprint"
            );
            return;
        }
        // Actual brain movement only: no admitted-path teleport assistance.
        crate::villager::brain::tick(&mut state, &mut changes);
        state.world.fold_overlay();
    }
    panic!("builder did not walk out of the clear site and begin construction");
}

#[test]
fn builder_walks_to_solid_storage_then_selects_exact_site_target() {
    let (mut state, id, slot) = fixture();
    start(&mut state, id, "storage", 1, None);
    fund(&mut state, "storage", 1);
    state.entities.set_pos(slot, [14.5, 201.0, 14.5]);
    let handle = state
        .settlements
        .get(id)
        .unwrap()
        .project
        .as_ref()
        .unwrap()
        .structure
        .clone();
    let (item, _) = executor::next_material(&state, id, &handle)
        .unwrap()
        .unwrap();
    let stock = state.block_entities[&(1, 201, 1)].slots.clone();
    let mut v = state.entities.row_mut(slot).villager.take().unwrap();
    let target = building::work(&mut state, slot, &mut v, &mut ChangeSet::default()).unwrap();
    state.entities.row_mut(slot).villager = Some(v);
    assert_eq!(target.pos, BlockPos { x: 1, y: 201, z: 1 });
    assert!(
        !target.exact,
        "solid storage containers need an adjacent approach, not an exact occupied cell"
    );
    for now in 0..1200 {
        state.tick = now;
        state.tick_budget.reset(now);
        let mut changes = ChangeSet::default();
        crate::settlements::tick(&mut state, &mut changes);
        if state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .inventory
            .iter()
            .flatten()
            .any(|stack| stack.item_id == item && stack.count > 0)
        {
            assert_ne!(
                state.block_entities[&(1, 201, 1)].slots,
                stock,
                "material comes from the physical storage container"
            );
            assert_eq!(
                executor::status(&state, id, &handle)
                    .unwrap()
                    .completed_work_units,
                0,
                "a withdrawal does not also place a block"
            );
            let mut v = state.entities.row_mut(slot).villager.take().unwrap();
            let target = building::work(&mut state, slot, &mut v, &mut changes).unwrap();
            state.entities.row_mut(slot).villager = Some(v);
            assert!(
                target.exact,
                "carried materials select an exact perimeter goal"
            );
            return;
        }
        // Exercise actual navigation and physics, including the solid chest.
        crate::villager::brain::tick(&mut state, &mut changes);
        state.world.fold_overlay();
    }
    panic!("empty-carry builder did not walk to storage and withdraw real materials");
}

/// S1 from the buildings real-client QA: the builder halted at 71/249 because
/// its exact perimeter goal was reached by the 0.3-block arrival tolerance with
/// the 0.3-block body still straddling the future footprint edge.
#[test]
fn builder_perimeter_goal_keeps_body_outside_footprint_at_arrival_tolerance() {
    for (dx, dz) in [(0.29, 0.0), (-0.29, 0.0), (0.0, 0.29), (0.0, -0.29)] {
        let (mut state, id, slot) = fixture();
        start(&mut state, id, "house", 1, None);
        fund(&mut state, "house", 1);
        let handle = state
            .settlements
            .get(id)
            .unwrap()
            .project
            .as_ref()
            .unwrap()
            .structure
            .clone();
        // Fetch the real first stack at the chest, then take the exact goal.
        building::tick(&mut state, id);
        let mut v = state.entities.row_mut(slot).villager.take().unwrap();
        let target = building::work(&mut state, slot, &mut v, &mut ChangeSet::default()).unwrap();
        state.entities.row_mut(slot).villager = Some(v);
        assert!(target.exact);
        // Any body position the brain accepts as arrival (squared distance
        // below 0.09 from the cell centre) must be able to place.
        state.entities.set_pos(
            slot,
            [
                f64::from(target.pos.x) + 0.5 + dx,
                f64::from(target.pos.y),
                f64::from(target.pos.z) + 0.5 + dz,
            ],
        );
        state.tick += 4;
        building::tick(&mut state, id);
        assert_eq!(
            executor::status(&state, id, &handle)
                .unwrap()
                .completed_work_units,
            1,
            "arrival offset ({dx}, {dz}) at goal {:?} must place",
            target.pos
        );
    }
}

/// The same stall through real navigation: no admitted-path assistance.
#[test]
fn builder_finishes_a_house_through_real_movement() {
    let (mut state, id, slot) = fixture();
    start(&mut state, id, "house", 1, None);
    fund(&mut state, "house", 1);
    for now in 0..20_000 {
        state.tick = now;
        state.tick_budget.reset(now);
        let mut changes = ChangeSet::default();
        crate::settlements::tick(&mut state, &mut changes);
        if state.settlements.get(id).unwrap().project.is_none() {
            return;
        }
        crate::villager::brain::tick(&mut state, &mut changes);
        state.world.fold_overlay();
    }
    let project = state.settlements.get(id).unwrap().project.clone().unwrap();
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    panic!(
        "house stalled: {:?}\nnext={:?} material={:?} builder={:?} target={:?} path={:?} activity={:?} carry={:?}",
        &project_status(&state, id)[..2],
        executor::next_cell(&state, id, &project.structure),
        executor::next_material(&state, id, &project.structure),
        state.entities.pos(slot),
        v.navigation_target,
        v.navigation_path,
        v.activity,
        v.inventory
    );
}

/// QA geometry: the funded chest sits inside a sealed shell with a closed
/// wooden door one block above the outside ground, ~30 blocks from the site.
fn remote_shell_fixture() -> (TickState, SettlementId, EntitySlot, (i32, i32, i32)) {
    let (mut state, id, slot) = fixture();
    let columns: Vec<_> = (1..=2)
        .map(|x| crate::boundary::ChunkPos { x, z: 0 })
        .collect();
    state
        .world
        .ensure_blocking(&rustc_hash::FxHashMap::from_iter(
            columns.iter().map(|chunk| (*chunk, 0)),
        ));
    for (n, chunk) in columns.iter().enumerate() {
        state.tickets.insert(format!("shell-{n}"), (*chunk, 0));
    }
    state.refresh_active_chunks(0);
    for x in 16..48 {
        for z in 0..16 {
            state
                .world
                .set_block(x, 200, z, state.registry.states.stone);
            for y in 201..224 {
                state.world.set_block(x, y, z, state.registry.states.air);
            }
        }
    }
    let oak = block(&state, "minecraft:oak_planks", &[]);
    let cobble = block(&state, "minecraft:cobblestone", &[]);
    for x in 36..=40 {
        for z in 4..=8 {
            state.world.set_block(x, 201, z, oak);
            state.world.set_block(x, 205, z, oak);
            if x == 36 || x == 40 || z == 4 || z == 8 {
                for y in 202..=204 {
                    state.world.set_block(x, y, z, cobble);
                }
            }
        }
    }
    for (y, half) in [(202, "lower"), (203, "upper")] {
        let door = block(
            &state,
            "minecraft:oak_door",
            &[
                ("facing", "west"),
                ("half", half),
                ("hinge", "left"),
                ("open", "false"),
            ],
        );
        state.world.set_block(40, y, 6, door);
    }
    // A 2-high door above a 1-block sill cannot be entered by jumping (the head
    // meets the lintel), so QA's shell needs a step block outside the door.
    state.world.set_block(41, 201, 6, cobble);
    // Move the funded chest into the shell corner above the floor.
    state.world.set_block(1, 201, 1, state.registry.states.air);
    let image = state.block_entities.remove(&(1, 201, 1)).unwrap();
    let chest = block(&state, "minecraft:chest", &[]);
    state.world.set_block(37, 202, 5, chest);
    state.block_entities.insert((37, 202, 5), image);
    state.settlements.get_mut(id).unwrap().bootstrap_storage = Some([37, 202, 5]);
    state.entities.set_pos(slot, [38.5, 202.0, 6.5]);
    state.world.fold_overlay();
    (state, id, slot, (37, 202, 5))
}

#[test]
fn builder_builds_a_house_from_remote_shell_storage_within_ten_minutes() {
    let (mut state, id, slot, chest) = remote_shell_fixture();
    start(&mut state, id, "house", 1, None);
    fund_at(&mut state, chest, "house", 1);
    let mut changes = ChangeSet::default();
    // Five real minutes at 20 TPS (the product target is ten).
    for now in 0..6_000u64 {
        state.tick = now;
        state.day_time = 3000 + (now % 5000) as i64;
        state.tick_budget.reset(now);
        changes = ChangeSet::default();
        crate::settlements::tick(&mut state, &mut changes);
        if state.settlements.get(id).unwrap().project.is_none() {
            eprintln!("house finished at tick {now}");
            return;
        }
        crate::villager::brain::tick(&mut state, &mut changes);
        state.world.fold_overlay();
    }
    let _ = changes;
    let project = state.settlements.get(id).unwrap().project.clone().unwrap();
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    let door = |y| {
        state
            .world
            .block_at(40, y, 6)
            .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
            .map(|b| format!("{:?}", b.properties))
    };
    eprintln!(
        "door lower={:?} upper={:?} opened={:?} vel={:?} ground={}",
        door(202),
        door(203),
        v.opened_doors,
        state.entities.vel(slot),
        state.entities.ground(slot)
    );
    panic!(
        "house not done in 6000 ticks: {:?}\nnext={:?} material={:?} builder={:?} target={:?} path={:?} carry={:?}",
        &project_status(&state, id)[..2],
        executor::next_cell(&state, id, &project.structure),
        executor::next_material(&state, id, &project.structure),
        state.entities.pos(slot),
        v.navigation_target,
        v.navigation_path,
        v.inventory
    );
}

#[test]
fn one_haul_trip_carries_several_item_types_for_the_next_cells() {
    let (mut state, id, slot) = fixture();
    start(&mut state, id, "house", 1, None);
    fund(&mut state, "house", 1);
    building::tick(&mut state, id);
    let carry = &state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .unwrap()
        .inventory;
    let mut items: Vec<u32> = carry.iter().flatten().map(|stack| stack.item_id).collect();
    items.sort_unstable();
    items.dedup();
    assert!(
        items.len() >= 3,
        "one trip must load several materials, got {items:?}"
    );
    let units: i32 = carry.iter().flatten().map(|stack| stack.count).sum();
    assert!(units >= 50, "one trip carries a real load, got {units}");
}
