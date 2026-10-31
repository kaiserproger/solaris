use super::tests::{attempt, complete, fixture, fund, start};
use super::*;
use mc_script::ScriptWorldBounds;

fn certificate(record: &mut SettlementRecord, kind: BuildingKind, level: u8, natural_zone: bool) {
    record.building_sequence += 1;
    let id = BuildingId((record.id.0 << 32) | record.building_sequence);
    record.buildings.insert(
        id,
        buildings::Building {
            id,
            settlement: record.id,
            kind,
            level,
            bounds: ScriptWorldBounds::new([30, 200, 30], [32, 203, 32]).unwrap(),
            work_points: Vec::new(),
            revision: 1,
            natural_zone,
        },
    );
}

#[test]
fn autonomous_needs_cover_every_authored_kind_and_ignore_natural_zones() {
    let (mut state, id, _) = fixture();
    let record = state.settlements.get_mut(id).unwrap();
    assert_eq!(needed_kinds(record, 10).next(), Some(BuildingKind::Storage));
    certificate(record, BuildingKind::Storage, 1, false);
    for _ in 0..4 {
        certificate(record, BuildingKind::House, 3, false);
    }
    certificate(record, BuildingKind::Mine, 1, true);
    certificate(record, BuildingKind::LumberCamp, 1, true);
    for kind in [
        BuildingKind::Workshop(Profession::Farmer),
        BuildingKind::LumberCamp,
        BuildingKind::Mine,
        BuildingKind::Workshop(Profession::Builder),
        BuildingKind::Barracks,
        BuildingKind::Tower,
        BuildingKind::Wall,
    ] {
        assert_eq!(needed_kinds(record, 10).next(), Some(kind));
        certificate(record, kind, 1, false);
    }
    assert_eq!(needed_kinds(record, 10).next(), None);
    for (kind, threshold) in [
        (BuildingKind::Barracks, 6),
        (BuildingKind::Tower, 8),
        (BuildingKind::Wall, 10),
    ] {
        let building = record
            .buildings
            .values()
            .find(|building| building.kind == kind)
            .unwrap()
            .clone();
        record.buildings.remove(&building.id);
        assert_eq!(needed_kinds(record, threshold - 1).next(), None);
        assert_eq!(needed_kinds(record, threshold).next(), Some(kind));
        record.buildings.insert(building.id, building);
    }
    let authored: Vec<_> = blueprints::all(&state.registry)
        .iter()
        .map(|blueprint| blueprint.kind)
        .collect();
    assert_eq!(authored.len(), 9);
    assert!(authored.into_iter().all(|kind| {
        state
            .settlements
            .get(id)
            .unwrap()
            .buildings
            .values()
            .any(|building| building.kind == kind && !building.natural_zone)
    }));
}

fn plan_until_started(state: &mut TickState, id: SettlementId) {
    for _ in 0..2000 {
        plan(state, id);
        if state.settlements.get(id).unwrap().project.is_some() {
            return;
        }
    }
    panic!("bounded site search did not find the fixture's free area");
}

/// Seed 87122 replay: a funded hillside village never started its Storage in
/// 100k ticks. Candidate sites sat on the bell's own plane, and only 12 of
/// 61k columns had free ground anywhere within ten blocks of it. The planner
/// must follow the terrain to a flat site above or below the bell.
#[test]
fn funded_planner_follows_terrain_to_a_free_site_off_the_bell_plane() {
    let (mut state, id, _) = fixture();
    fund(&mut state, "storage", 1);
    // Raise the chunk five blocks everywhere but the chest corner: the bell
    // plane (y=200) has no clearable headroom in any footprint.
    for x in 0..16 {
        for z in 0..16 {
            if x < 4 && z < 4 {
                continue;
            }
            for y in 201..=205 {
                state.world.set_block(x, y, z, state.registry.states.stone);
            }
        }
    }
    state.world.fold_overlay();
    plan_until_started(&mut state, id);
    let project = state.settlements.get(id).unwrap().project.as_ref().unwrap();
    assert_eq!(project.blueprint, "storage");
    assert_eq!(
        project.origin[1], 205,
        "the raised surface, not the bell plane"
    );
}

/// Seed 87122 replay: the builder slot went to an unemployed resident whose
/// only house door was blocked by a tree crown. It sat in a ten-cell pocket
/// with the project active and every free villager idle. A resident that cannot
/// walk to storage is passed over for one that can.
#[test]
fn builder_choice_skips_a_resident_who_cannot_walk_to_storage() {
    builder_choice_skips_the_trapped_resident(false);
}

/// Buildings QA farm world (freeze 73e1c530): villagers and the allocator spend
/// the whole per-tick route allowance before the planning pass runs, so every
/// reachability probe was refused and no resident was ever made the builder
/// (0/226 placed for 55 minutes). The one-shot probe does not draw on the
/// shared tick allowance: an exhausted tick still picks the reachable resident.
#[test]
fn builder_choice_does_not_depend_on_the_shared_tick_route_allowance() {
    builder_choice_skips_the_trapped_resident(true);
}

fn builder_choice_skips_the_trapped_resident(starved_tick_budget: bool) {
    use crate::settlements::allocator::tests::villager;
    let (mut state, id, trapped) = fixture();
    state
        .entities
        .row_mut(trapped)
        .villager
        .as_mut()
        .unwrap()
        .data
        .profession = Profession::None;
    // Shut the first resident in a stone cell at (10, 201..202, 10).
    for x in 9..=11 {
        for z in 9..=11 {
            for y in 201..=203 {
                if (x, z) != (10, 10) || y == 203 {
                    state.world.set_block(x, y, z, state.registry.states.stone);
                }
            }
        }
    }
    state.world.fold_overlay();
    state.entities.set_pos(trapped, [10.5, 201.0, 10.5]);
    let free = villager(&mut state, [4.5, 201.0, 4.5]);
    let entity = state.entities.body(free).id;
    crate::settlements::bind(&mut state, id, entity).unwrap();
    state
        .entities
        .row_mut(free)
        .villager
        .as_mut()
        .unwrap()
        .data
        .profession = Profession::Mason;
    if starved_tick_budget {
        for kind in [
            crate::tick_budget::Kind::Decision,
            crate::tick_budget::Kind::PathNode,
        ] {
            state.tick_budget.exhaust_for_test(kind);
        }
    }
    assign_builder(&mut state, id);
    let profession = |slot| {
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .data
            .profession
    };
    assert_eq!(profession(trapped), Profession::None);
    assert_eq!(profession(free), Profession::Builder);
}

#[test]
fn inactive_incumbent_yields_to_an_active_builder_and_construction_advances() {
    use crate::boundary::{ChangeSet, ChunkPos};
    use crate::settlements::allocator::tests::villager;
    let (mut state, id, incumbent) = fixture();
    let inactive = ChunkPos { x: 1, z: 0 };
    state
        .world
        .ensure_blocking(&rustc_hash::FxHashMap::from_iter([(inactive, 0)]));
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
    state.world.fold_overlay();
    state.entities.set_pos(incumbent, [20.5, 201.0, 2.5]);
    state
        .entities
        .row_mut(incumbent)
        .villager
        .as_mut()
        .unwrap()
        .activity = crate::villager::Activity::Idle;
    let candidate = villager(&mut state, [1.5, 201.0, 2.5]);
    let entity = state.entities.body(candidate).id;
    crate::settlements::bind(&mut state, id, entity).unwrap();
    let v = state.entities.row_mut(candidate).villager.as_mut().unwrap();
    v.data.profession = Profession::Mason;
    v.activity = crate::villager::Activity::Work;
    assert!(!state.world.is_active(inactive.x, inactive.z));
    assert!(state.world.is_active(0, 0));
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
    for now in 0..400 {
        state.tick = now;
        state.tick_budget.reset(now);
        tick(&mut state);
        crate::villager::brain::tick(&mut state, &mut ChangeSet::default());
        state.world.fold_overlay();
    }
    let status = executor::status(&state, id, &handle).unwrap();
    let total: u64 = status.stages.iter().map(|stage| stage.work_units).sum();
    eprintln!("INACTIVE_BUILDER progress={}/{total}", status.watermark);
    assert!(
        status.watermark > 0,
        "inactive incumbent keeps the funded Storage at 0/{total}"
    );
    assert_eq!(
        state
            .entities
            .row(incumbent)
            .villager
            .as_ref()
            .unwrap()
            .data
            .profession,
        Profession::None,
        "the inactive resident must not be selected again"
    );
    assert_eq!(
        state
            .entities
            .row(candidate)
            .villager
            .as_ref()
            .unwrap()
            .data
            .profession,
        Profession::Builder
    );
}

#[test]
fn funded_owner_order_starts_the_single_project_without_world_or_stock_debit() {
    let (mut state, id, _) = fixture();
    fund(&mut state, "watch_tower", 1);
    let before_stock = state.block_entities[&(1, 201, 1)].slots.clone();
    let before_block = state.world.block_at(8, 201, 8);
    request_build(&mut state, id, "watch_tower").unwrap();
    let record = state.settlements.get(id).unwrap();
    let project = record
        .project
        .as_ref()
        .expect("funded owner order starts immediately");
    assert_eq!(project.blueprint, "watch_tower");
    assert_eq!(project.level, 1);
    assert!(
        !project.structure.is_empty(),
        "canonical structure authority"
    );
    assert!(executor::status(&state, id, &project.structure).is_ok());
    assert_eq!(state.block_entities[&(1, 201, 1)].slots, before_stock);
    assert_eq!(state.world.block_at(8, 201, 8), before_block);
    assert!(
        state.plugin_block_writes.is_empty(),
        "admission never places blocks"
    );
}

#[test]
fn unfunded_owner_order_is_not_saved_and_cannot_obstruct_autonomous_planning() {
    let (mut state, id, _) = fixture();
    let revision = state.settlements.get(id).unwrap().revision;
    let stock = state.block_entities[&(1, 201, 1)].slots.clone();
    state.settlements.take_images();
    let error = request_build(&mut state, id, "house").unwrap_err();
    assert!(error.starts_with("not enough:"), "{error}");
    assert!(state.settlements.get(id).unwrap().project.is_none());
    assert_eq!(state.settlements.get(id).unwrap().revision, revision);
    assert!(
        state.settlements.take_images().is_empty(),
        "unfunded order has no saved request"
    );
    assert_eq!(state.block_entities[&(1, 201, 1)].slots, stock);
    fund(&mut state, "storage", 1);
    plan_until_started(&mut state, id);
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
}

#[test]
fn active_project_refuses_owner_build_and_upgrade_without_mutation() {
    let (mut state, id, _) = fixture();
    start(&mut state, id, "storage", 1, None);
    fund(&mut state, "storage", 1);
    let before = state.settlements.get(id).unwrap().project.clone();
    let revision = state.settlements.get(id).unwrap().revision;
    let stock = state.block_entities[&(1, 201, 1)].slots.clone();
    state.settlements.take_images();
    assert_eq!(request_build(&mut state, id, "house").unwrap_err(), "Busy.");
    assert_eq!(
        request_upgrade(&mut state, id, BuildingId(123)).unwrap_err(),
        "Busy."
    );
    assert_eq!(state.settlements.get(id).unwrap().project, before);
    assert_eq!(state.settlements.get(id).unwrap().revision, revision);
    assert_eq!(state.block_entities[&(1, 201, 1)].slots, stock);
    assert!(state.settlements.take_images().is_empty());
    assert!(state.plugin_block_writes.is_empty());
}

#[test]
fn requested_upgrade_requires_present_materials_and_starts_immediately() {
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
    assert!(upgrade_available(&state, id, building));
    state
        .block_entities
        .get_mut(&(1, 201, 1))
        .unwrap()
        .slots
        .fill(mc_data::ItemStack::EMPTY);
    let revision = state.settlements.get(id).unwrap().revision;
    state.settlements.take_images();
    let error = request_upgrade(&mut state, id, building).unwrap_err();
    assert!(error.starts_with("not enough:"), "{error}");
    assert!(state.settlements.get(id).unwrap().project.is_none());
    assert_eq!(state.settlements.get(id).unwrap().revision, revision);
    assert!(state.settlements.take_images().is_empty());
    fund(&mut state, "house", 2);
    request_upgrade(&mut state, id, building).unwrap();
    let project = state.settlements.get(id).unwrap().project.as_ref().unwrap();
    assert_eq!(project.building, Some(building));
    assert_eq!(project.level, 2);
    complete(&mut state, id, slot);
    let record = state.settlements.get(id).unwrap();
    assert!(record.project.is_none());
    assert_eq!(record.buildings[&building].level, 2);
    assert_eq!(record.buildings.len(), 1);
}

#[test]
fn autonomous_upgrade_requires_blueprint_provenance_and_materials() {
    let (mut state, id, slot) = fixture();
    start(&mut state, id, "house", 1, None);
    fund(&mut state, "house", 1);
    complete(&mut state, id, slot);
    let house = *state
        .settlements
        .get(id)
        .unwrap()
        .buildings
        .keys()
        .next()
        .unwrap();
    let record = state.settlements.get_mut(id).unwrap();
    record.owner = None;
    for kind in [
        BuildingKind::Storage,
        BuildingKind::Workshop(Profession::Farmer),
        BuildingKind::LumberCamp,
        BuildingKind::Mine,
        BuildingKind::Workshop(Profession::Builder),
    ] {
        certificate(record, kind, 1, false);
    }
    assert_eq!(needed_kinds(record, 1).next(), None);
    let player_certificate = record
        .buildings
        .values()
        .find(|building| building.kind == BuildingKind::Storage)
        .unwrap()
        .id;
    assert!(!upgrade_available(&state, id, player_certificate));
    plan(&mut state, id);
    assert!(state.settlements.get(id).unwrap().project.is_none());
    fund(&mut state, "house", 2);
    plan(&mut state, id);
    let project = state.settlements.get(id).unwrap().project.as_ref().unwrap();
    assert_eq!(project.building, Some(house));
    assert_eq!(project.level, 2);
    complete(&mut state, id, slot);
    assert_eq!(
        state.settlements.get(id).unwrap().buildings[&house].level,
        2
    );
}

#[test]
fn partial_delivery_funds_a_blueprint_proven_upgrade_without_mutating_stock() {
    let (mut state, id, slot) = fixture();
    start(&mut state, id, "storage", 1, None);
    fund(&mut state, "storage", 1);
    complete(&mut state, id, slot);
    for [x, y, z] in buildings::storage_points(&state, id) {
        if let Some(container) = state.block_entities.get_mut(&(x, y, z)) {
            container.slots.fill(mc_data::ItemStack::EMPTY);
        }
    }
    fund(&mut state, "storage", 2);
    let plank = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:oak_planks").unwrap())
        .unwrap();
    let slots = &mut state.block_entities.get_mut(&(1, 201, 1)).unwrap().slots;
    let index = slots.iter().position(|s| s.item_id == plank).unwrap();
    let carried = slots[index].clone();
    slots[index] = mc_data::ItemStack::EMPTY;
    let stock = slots.clone();
    assert!(!has_funded_candidate(&state, id));
    assert!(candidate_funded_by_delivery(&state, id, &[Some(carried)]));
    assert_eq!(state.block_entities[&(1, 201, 1)].slots, stock);
    assert!(state.settlements.get(id).unwrap().project.is_none());
}

#[test]
fn invalid_or_stale_owner_targets_leave_no_request_to_obstruct_planning() {
    let (mut state, id, _) = fixture();
    let record = state.settlements.get_mut(id).unwrap();
    certificate(record, BuildingKind::Mine, 1, true);
    let mine = *record.buildings.keys().next().unwrap();
    let revision = record.revision;
    assert!(!upgrade_available(&state, id, mine));
    state.settlements.take_images();
    assert!(request_build(&mut state, id, "not-a-blueprint").is_err());
    assert!(request_upgrade(&mut state, id, BuildingId(123)).is_err());
    assert!(request_upgrade(&mut state, id, mine).is_err());
    assert_eq!(state.settlements.get(id).unwrap().revision, revision);
    assert!(state.settlements.get(id).unwrap().project.is_none());
    assert!(state.settlements.take_images().is_empty());
    fund(&mut state, "storage", 1);
    plan_until_started(&mut state, id);
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
}

#[test]
fn funded_owner_order_without_a_free_site_leaves_no_saved_request_or_cursor() {
    let (mut state, id, _) = fixture();
    fund(&mut state, "watch_tower", 1);
    // Terrain-following sites reach ±SITE_BAND around the bell, so the
    // plateau must be solid through the whole band.
    for x in 0..16 {
        for z in 0..16 {
            if (x, z) != (1, 1) {
                for y in 201..=216 {
                    state.world.set_block(x, y, z, state.registry.states.stone);
                }
            }
        }
    }
    state.world.fold_overlay();
    let record = state.settlements.get(id).unwrap();
    let revision = record.revision;
    let cursor = record.construction_cursor;
    let stock = state.block_entities[&(1, 201, 1)].slots.clone();
    state.settlements.take_images();
    let error = request_build(&mut state, id, "watch_tower").unwrap_err();
    assert!(
        error.contains("No free loaded construction site"),
        "{error}"
    );
    let record = state.settlements.get(id).unwrap();
    assert!(record.project.is_none());
    assert_eq!(record.revision, revision);
    assert_eq!(record.construction_cursor, cursor);
    assert_eq!(state.block_entities[&(1, 201, 1)].slots, stock);
    assert!(state.settlements.take_images().is_empty());
}

fn extend_planning_site(state: &mut TickState) {
    // Leave actual loaded room for the farm's full seven-by-ten footprint
    // beside the storage executor's reserved footprint.
    state
        .world
        .ensure_blocking(&rustc_hash::FxHashMap::from_iter([(
            crate::boundary::ChunkPos { x: 0, z: 1 },
            0,
        )]));
    for x in 0..16 {
        for z in 16..32 {
            state
                .world
                .set_block(x, 200, z, state.registry.states.stone);
            for y in 201..224 {
                state.world.set_block(x, y, z, state.registry.states.air);
            }
        }
    }
    state.world.fold_overlay();
}

#[test]
fn unfunded_house_skips_to_funded_farm_without_debiting_stock() {
    let (mut state, id, slot) = fixture();
    extend_planning_site(&mut state);
    start(&mut state, id, "storage", 1, None);
    fund(&mut state, "storage", 1);
    complete(&mut state, id, slot);
    assert!(
        state
            .settlements
            .get(id)
            .unwrap()
            .buildings
            .values()
            .any(|building| building.kind == BuildingKind::Storage && !building.natural_zone),
        "storage is really built, not certified"
    );
    // Order is unchanged: the house is still the top need.
    assert_eq!(
        needed_kinds(state.settlements.get(id).unwrap(), 1).next(),
        Some(BuildingKind::House)
    );
    // No shepherd on the bound resident and no sheep in the fixture, so wool
    // has no source here.
    assert!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .is_some_and(|villager| villager.data.profession != Profession::Shepherd)
    );
    // Fund only the later kind's real materials: no wool, no bed.
    fund(&mut state, "farm", 1);
    let house_materials = blueprints::get(&state.registry, "house")
        .unwrap()
        .level(1)
        .unwrap()
        .materials
        .clone();
    assert!(
        !super::super::construction_crafting::can_fund(
            &state,
            &house_materials,
            &executor::source_images(&state, id)
        ),
        "house must be unfundable without wool"
    );
    let before = state.block_entities[&(1, 201, 1)].slots.clone();
    plan_until_started(&mut state, id);
    let record = state.settlements.get(id).unwrap();
    let project = record
        .project
        .as_ref()
        .expect("planner must start the next fundable need");
    assert_eq!(
        project.blueprint, "farm",
        "unfunded house must not block a funded later need"
    );
    assert_eq!(
        state.block_entities[&(1, 201, 1)].slots,
        before,
        "selection must not debit stock"
    );
    let status = project_status(&state, id).join("\n");
    assert!(
        status.contains("house"),
        "waiting explanation names the skipped kind:\n{status}"
    );
    assert!(
        status.contains("white_wool"),
        "waiting explanation names the missing wool:\n{status}"
    );
    assert!(
        status.to_lowercase().contains("no source"),
        "waiting explanation reports no wool source:\n{status}"
    );
}

#[test]
fn funded_house_retains_priority_over_later_needs() {
    let (mut state, id, slot) = fixture();
    extend_planning_site(&mut state);
    start(&mut state, id, "storage", 1, None);
    fund(&mut state, "storage", 1);
    complete(&mut state, id, slot);
    fund(&mut state, "house", 1);
    let house_materials = blueprints::get(&state.registry, "house")
        .unwrap()
        .level(1)
        .unwrap()
        .materials
        .clone();
    assert!(
        super::super::construction_crafting::can_fund(
            &state,
            &house_materials,
            &executor::source_images(&state, id)
        ),
        "exact house stock funds the house"
    );
    let before = state.block_entities[&(1, 201, 1)].slots.clone();
    plan_until_started(&mut state, id);
    let record = state.settlements.get(id).unwrap();
    let project = record.project.as_ref().expect("funded house must start");
    assert_eq!(
        project.blueprint, "house",
        "a truly funded house keeps priority"
    );
    assert_eq!(
        state.block_entities[&(1, 201, 1)].slots,
        before,
        "selection must not debit stock"
    );
}

/// Merge several blueprints' level-one materials into the fixture chest.
fn fund_all(state: &mut TickState, blueprints_: &[&str]) {
    let mut total: Vec<(u32, u32)> = Vec::new();
    for name in blueprints_ {
        for (item, count) in &blueprints::get(&state.registry, name)
            .unwrap()
            .level(1)
            .unwrap()
            .materials
        {
            match total.iter_mut().find(|(id, _)| id == item) {
                Some((_, sum)) => *sum += count,
                None => total.push((*item, *count)),
            }
        }
    }
    let slots = &mut state.block_entities.get_mut(&(1, 201, 1)).unwrap().slots;
    slots.fill(mc_data::ItemStack::EMPTY);
    let mut index = 0;
    for (item, mut quantity) in total {
        while quantity > 0 {
            let count = quantity.min(64);
            slots[index] = mc_data::ItemStack::new(item, count as i32);
            quantity -= count;
            index += 1;
        }
    }
}

fn autonomous_storage(state: &mut TickState, id: SettlementId) {
    extend_planning_site(state);
    fund_all(state, &["storage", "house"]);
    plan_until_started(state, id);
    let project = state.settlements.get(id).unwrap().project.as_ref().unwrap();
    assert_eq!(project.blueprint, "storage");
    assert!(!project.ordered, "the planner's project is autonomous");
}

#[test]
fn owner_order_preempts_an_autonomous_project_that_placed_nothing() {
    let (mut state, id, _) = fixture();
    autonomous_storage(&mut state, id);
    let old = state
        .settlements
        .get(id)
        .unwrap()
        .project
        .as_ref()
        .unwrap()
        .structure
        .clone();
    let stock = state.block_entities[&(1, 201, 1)].slots.clone();
    request_build(&mut state, id, "house").expect("untouched autonomous project yields");
    let project = state.settlements.get(id).unwrap().project.as_ref().unwrap();
    assert_eq!(project.blueprint, "house");
    assert!(project.ordered);
    assert_ne!(project.structure, old);
    assert_eq!(
        executor::status(&state, id, &old).unwrap().state,
        mc_script::ScriptStructureState::Cancelled
    );
    assert_eq!(state.block_entities[&(1, 201, 1)].slots, stock);
    assert!(state.plugin_block_writes.is_empty());
}

#[test]
fn owner_order_is_busy_once_an_autonomous_project_placed_a_block() {
    let (mut state, id, slot) = fixture();
    autonomous_storage(&mut state, id);
    let handle = state
        .settlements
        .get(id)
        .unwrap()
        .project
        .as_ref()
        .unwrap()
        .structure
        .clone();
    for _ in 0..400 {
        super::tests::attempt(&mut state, id, slot);
        if executor::status(&state, id, &handle)
            .unwrap()
            .completed_work_units
            > 0
        {
            break;
        }
    }
    assert!(
        executor::status(&state, id, &handle)
            .unwrap()
            .completed_work_units
            > 0
    );
    assert_eq!(request_build(&mut state, id, "house").unwrap_err(), "Busy.");
    assert_eq!(
        state
            .settlements
            .get(id)
            .unwrap()
            .project
            .as_ref()
            .unwrap()
            .structure,
        handle
    );
}

#[test]
fn ordered_project_is_never_preempted_by_planner_or_a_second_order() {
    let (mut state, id, _) = fixture();
    extend_planning_site(&mut state);
    fund_all(&mut state, &["storage", "house"]);
    request_build(&mut state, id, "house").unwrap();
    let before = state.settlements.get(id).unwrap().project.clone();
    assert!(before.as_ref().unwrap().ordered);
    for _ in 0..200 {
        plan(&mut state, id);
    }
    assert_eq!(
        request_build(&mut state, id, "storage").unwrap_err(),
        "Busy."
    );
    assert_eq!(state.settlements.get(id).unwrap().project, before);
}

#[test]
fn refused_owner_order_keeps_the_autonomous_project() {
    let (mut state, id, _) = fixture();
    autonomous_storage(&mut state, id);
    let before = state.settlements.get(id).unwrap().project.clone();
    let error = request_build(&mut state, id, "watch_tower").unwrap_err();
    assert!(error.starts_with("not enough:"), "{error}");
    assert_eq!(state.settlements.get(id).unwrap().project, before);
}

#[test]
fn preempting_returns_the_builders_hauled_materials_before_funding_the_order() {
    let (mut state, id, slot) = fixture();
    autonomous_storage(&mut state, id);
    let stock_total = |state: &TickState| -> i32 {
        state.block_entities[&(1, 201, 1)]
            .slots
            .iter()
            .map(|stack| stack.count)
            .sum()
    };
    let before = stock_total(&state);
    // The builder hauls the autonomous project's materials out of the chest.
    crate::villager::building::tick(&mut state, id);
    let carried = |state: &TickState| -> i32 {
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
            .sum()
    };
    assert!(carried(&state) > 0, "the haul left the chest");
    assert!(stock_total(&state) < before);
    request_build(&mut state, id, "house").expect("hauled stock still funds the order");
    assert_eq!(carried(&state), 0, "carry went back to storage");
    assert_eq!(stock_total(&state), before, "nothing lost or duplicated");
}

#[test]
fn upgrade_is_offered_for_an_ordered_house_that_replaced_an_autonomous_site() {
    let (mut state, id, slot) = fixture();
    autonomous_storage(&mut state, id);
    fund_all(&mut state, &["house"]);
    request_build(&mut state, id, "house").unwrap();
    let project = state.settlements.get(id).unwrap().project.clone().unwrap();
    complete(&mut state, id, slot);
    let (bid, building) = state
        .settlements
        .get(id)
        .unwrap()
        .buildings
        .iter()
        .find(|(_, b)| b.kind == BuildingKind::House)
        .map(|(id, b)| (*id, b.clone()))
        .expect("house certified");
    assert_eq!(
        building.bounds.min, project.origin,
        "certificate follows the executor origin"
    );
    assert!(executor::built_at(&state, id, "house", building.bounds.min));
    assert!(upgrade_available(&state, id, bid), "L2 upgrade row exists");
    // The row is orderable and the upgraded house offers L3 the same way.
    for level in [2, 3] {
        fund(&mut state, "house", level);
        request_upgrade(&mut state, id, bid).unwrap();
        complete(&mut state, id, slot);
        let upgraded = &state.settlements.get(id).unwrap().buildings[&bid];
        assert_eq!(upgraded.level, level);
        assert_eq!(upgraded.bounds.min, project.origin);
        assert_eq!(upgrade_available(&state, id, bid), level < 3);
    }
}

/// Real-client QA farm L1_blockmatch read 12 air cells at origin.x + 6. L1 was
/// whole (every planned cell placed, structure committed); the surplus QA stock
/// let the planner start the L2 upgrade at once, and its clear stage removed
/// exactly the L1 east-wall cells that L2 does not keep.
#[test]
fn farm_level_one_is_fully_placed_and_a_funded_upgrade_clears_only_its_removals() {
    let (mut state, id, slot) = fixture();
    start(&mut state, id, "farm", 1, None);
    fund(&mut state, "farm", 1);
    complete(&mut state, id, slot);
    let origin = [3, 200, 3];
    let farm = blueprints::get(&state.registry, "farm").unwrap();
    let level1 = farm
        .level(1)
        .unwrap()
        .rotated(Rotation::from_quarter_turns(0));
    let level2 = farm
        .level(2)
        .unwrap()
        .rotated(Rotation::from_quarter_turns(0));
    let at = |offset: &[i8; 3]| level1.world(origin, *offset);
    let mismatched = |state: &TickState| -> std::collections::BTreeSet<[i32; 3]> {
        level1
            .blocks
            .iter()
            .map(|(offset, block)| (at(offset), block))
            .filter(|(pos, block)| state.world.block_at(pos[0], pos[1], pos[2]) != Some(block.0))
            .map(|(pos, _)| pos)
            .collect()
    };
    assert!(
        level1.blocks.iter().any(|(offset, _)| offset[0] == 6),
        "the blueprint has a last column"
    );
    assert!(
        mismatched(&state).is_empty(),
        "certified L1 holds every blueprint cell, last column included"
    );
    // Surplus stock and every other need already met: the planner upgrades.
    let record = state.settlements.get_mut(id).unwrap();
    record.owner = None;
    for kind in [
        BuildingKind::Storage,
        BuildingKind::House,
        BuildingKind::LumberCamp,
        BuildingKind::Mine,
        BuildingKind::Workshop(Profession::Builder),
    ] {
        certificate(record, kind, 1, false);
    }
    fund(&mut state, "farm", 2);
    plan(&mut state, id);
    let project = state.settlements.get(id).unwrap().project.clone().unwrap();
    assert_eq!((project.level, project.ordered), (2, false));
    for _ in 0..4096 {
        let status = executor::status(&state, id, &project.structure).unwrap();
        if status.current_stage_index >= 1 {
            break;
        }
        attempt(&mut state, id, slot);
    }
    let removals: std::collections::BTreeSet<[i32; 3]> = level2
        .removals
        .iter()
        .map(|offset| level2.world(origin, *offset))
        .collect();
    assert_eq!(mismatched(&state), removals, "the clear stage only removes");
    assert_eq!(removals.len(), 12);
    assert!(removals.iter().all(|pos| pos[0] == origin[0] + 6));
}

/// Buildings QA farm world (freeze bc04b53b): a certified farm and an autonomous
/// L2 project appeared while the L1 structure was believed to be Running. A
/// building certifies only through a Committed structure, and the single
/// project slot keeps the planner from admitting L+1 before L is Committed.
#[test]
fn running_structure_neither_certifies_nor_admits_an_upgrade() {
    let (mut state, id, slot) = fixture();
    start(&mut state, id, "farm", 1, None);
    fund(&mut state, "farm", 1);
    for _ in 0..64 {
        attempt(&mut state, id, slot);
    }
    let project = state.settlements.get(id).unwrap().project.clone().unwrap();
    let status = executor::status(&state, id, &project.structure).unwrap();
    assert_eq!(status.state, ScriptStructureState::Running);
    assert!(status.completed_work_units > 0, "the build is under way");
    let record = state.settlements.get_mut(id).unwrap();
    record.owner = None;
    for kind in [
        BuildingKind::Storage,
        BuildingKind::House,
        BuildingKind::LumberCamp,
        BuildingKind::Mine,
        BuildingKind::Workshop(Profession::Builder),
    ] {
        certificate(record, kind, 1, false);
    }
    let certified = state.settlements.get(id).unwrap().buildings.len();
    fund(&mut state, "farm", 2);
    assert!(!finish_project(&mut state, id), "Running never certifies");
    plan(&mut state, id);
    let record = state.settlements.get(id).unwrap();
    assert_eq!(record.buildings.len(), certified, "no farm certificate yet");
    let kept = record.project.as_ref().unwrap();
    assert_eq!(
        (kept.level, kept.structure.as_str()),
        (1, project.structure.as_str())
    );
}
