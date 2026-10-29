use super::tests::{fixture, fund};
use super::*;

/// Three-by-three chunks around the fixture's own chunk: stone floor at
/// y=200, then solid stone through the whole terrain-following band except
/// the fixture chest, so a site exists only where a test carves one.
fn solid_plateau(state: &mut TickState) {
    state
        .world
        .ensure_blocking(&rustc_hash::FxHashMap::from_iter((-1..=1).flat_map(|x| {
            (-1..=1).map(move |z| (crate::boundary::ChunkPos { x, z }, 0))
        })));
    for x in -16i32..32 {
        for z in -16..32 {
            state
                .world
                .set_block(x, 200, z, state.registry.states.stone);
            for y in 201..=216 {
                if (x, y, z) != (1, 201, 1) {
                    state.world.set_block(x, y, z, state.registry.states.stone);
                }
            }
        }
    }
}

fn clear(state: &mut TickState, origin: [i32; 3], size: [u8; 3]) {
    for x in origin[0]..origin[0] + i32::from(size[0]) {
        for z in origin[2]..origin[2] + i32::from(size[2]) {
            for y in 201..=216 {
                state.world.set_block(x, y, z, state.registry.states.air);
            }
        }
    }
}

fn storage_size(state: &TickState) -> [u8; 3] {
    blueprints::get(&state.registry, "storage")
        .unwrap()
        .footprint(Rotation::from_quarter_turns(0))
}

/// Origin whose footprint centre sits at `(dx, dz)` from the bell.
fn centred(anchor: [i32; 3], size: [u8; 3], dx: i32, dz: i32) -> [i32; 3] {
    [
        anchor[0] + dx - i32::from(size[0] / 2),
        200,
        anchor[2] + dz - i32::from(size[2] / 2),
    ]
}

/// Chebyshev distance of the footprint centre from the anchor.
fn distance(anchor: [i32; 3], size: [u8; 3], origin: [i32; 3]) -> i32 {
    (origin[0] + i32::from(size[0] / 2) - anchor[0])
        .abs()
        .max((origin[2] + i32::from(size[2] / 2) - anchor[2]).abs())
}

/// The truth `site_project` must match: the free site with the smallest
/// distance, over every loaded column of the settlement sphere.
fn nearest_free_distance(state: &TickState, id: SettlementId, size: [u8; 3]) -> Option<i32> {
    let record = state.settlements.get(id).unwrap();
    let mut best = None;
    for x in -16i32..32 {
        for z in -16..32 {
            let Some(ground) = surface_y(state, record.anchor[1], x, z) else {
                continue;
            };
            let origin = [x, ground, z];
            if site_free(state, record, origin, size) {
                let found = distance(record.anchor, size, origin);
                best = Some(best.map_or(found, |current: i32| current.min(found)));
            }
        }
    }
    best
}

#[test]
fn ring_order_visits_every_cell_once_nearest_ring_first() {
    let radius = 6;
    let mut seen = std::collections::BTreeSet::new();
    let mut previous = 0;
    for n in 0..(2 * radius + 1u32).pow(2) {
        let (dx, dz) = ring_offset(n);
        let ring = dx.abs().max(dz.abs());
        assert!(ring >= previous, "ring {ring} after {previous} at {n}");
        previous = ring;
        assert!(seen.insert((dx, dz)), "cell ({dx}, {dz}) repeated at {n}");
    }
    assert_eq!(seen.len(), 13 * 13);
}

/// Seed 87122 claim QA: a storage ordered at the anchor was sited 110 blocks
/// away, and 21 earlier orders were refused. Two free pockets exist; the order
/// takes the one nearer the bell, though both lie off the bell's own row.
#[test]
fn owner_order_takes_the_free_site_nearest_the_anchor() {
    let (mut state, id, _) = fixture();
    fund(&mut state, "storage", 1);
    solid_plateau(&mut state);
    let size = storage_size(&state);
    let anchor = state.settlements.get(id).unwrap().anchor;
    let far = centred(anchor, size, 12, -14);
    let near = centred(anchor, size, -6, 7);
    clear(&mut state, far, size);
    clear(&mut state, near, size);
    state.world.fold_overlay();
    request_build(&mut state, id, "storage").unwrap();
    let project = state.settlements.get(id).unwrap().project.as_ref().unwrap();
    assert_eq!(project.origin, near, "not the pocket at {far:?}");
}

/// A cluttered village: a pillar every third column leaves no footprint-sized
/// gap near the bell, and the only clearing lies off the bell's row. One owner
/// order must find it, and it must be the nearest free site there is.
#[test]
fn owner_order_in_a_cluttered_village_finds_the_nearest_site_at_once() {
    let (mut state, id, _) = fixture();
    fund(&mut state, "storage", 1);
    solid_plateau(&mut state);
    let size = storage_size(&state);
    let anchor = state.settlements.get(id).unwrap().anchor;
    for x in -16i32..32 {
        for z in -16i32..32 {
            if x.rem_euclid(3) != 0 && z.rem_euclid(3) != 0 && (x, z) != (1, 1) {
                for y in 201..=216 {
                    state.world.set_block(x, y, z, state.registry.states.air);
                }
            }
        }
    }
    let clearing = centred(anchor, size, 14, 13);
    clear(&mut state, clearing, size);
    state.world.fold_overlay();
    let nearest = nearest_free_distance(&state, id, size).expect("fixture has a free site");
    request_build(&mut state, id, "storage").unwrap();
    let project = state.settlements.get(id).unwrap().project.as_ref().unwrap();
    assert_eq!(distance(anchor, size, project.origin), nearest);
    assert!(
        distance(anchor, size, project.origin) <= 14,
        "{:?}",
        project.origin
    );
}

fn profession(state: &TickState, slot: crate::entities::EntitySlot) -> Profession {
    state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .unwrap()
        .data
        .profession
}

/// Seed 87122 autonomy QA: the storage project was admitted at once, but its
/// builder woke inside a house whose door step was roofed by a tree crown. It
/// stood in the doorway for 7000 daytime ticks, and no resident replaced it
/// because a builder, once chosen, was never re-checked. A builder on shift
/// that cannot walk to storage is released and a resident who can takes over.
#[test]
fn trapped_builder_on_shift_is_replaced_by_a_resident_who_reaches_storage() {
    use crate::settlements::allocator::tests::villager;
    let (mut state, id, trapped) = fixture();
    fund(&mut state, "storage", 1);
    // Shut the fixture's builder in a stone cell at (10, 201..202, 10).
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
    assert_eq!(profession(&state, trapped), Profession::Builder);
    assign_builder(&mut state, id);
    assert_eq!(profession(&state, trapped), Profession::None);
    assert_eq!(profession(&state, free), Profession::Builder);
}

/// The re-check never costs a working builder its slot, and an off-shift
/// builder (asleep behind a closed door at night) is left alone.
#[test]
fn reachable_or_off_shift_builder_keeps_the_slot() {
    use crate::settlements::allocator::tests::villager;
    let (mut state, id, builder) = fixture();
    let free = villager(&mut state, [4.5, 201.0, 4.5]);
    let entity = state.entities.body(free).id;
    crate::settlements::bind(&mut state, id, entity).unwrap();
    assign_builder(&mut state, id);
    assert_eq!(profession(&state, builder), Profession::Builder);
    assert_eq!(profession(&state, free), Profession::None);
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
    state.entities.set_pos(builder, [10.5, 201.0, 10.5]);
    state.day_time = 15_000;
    assign_builder(&mut state, id);
    assert_eq!(profession(&state, builder), Profession::Builder);
}

/// Deterministic terrain, not an operator-built pad: every building-sized
/// footprint crosses both low and high columns.
#[test]
fn hillside_village_prepares_and_builds_first_storage() {
    use super::tests::complete;
    use mc_data::ItemStack;
    let (mut state, id, builder) = fixture();
    state.settlements.get_mut(id).unwrap().anchor[0] = 10;
    fund(&mut state, "storage", 1);
    let dirt = crate::settlements::allocator::tests::block(&state, "minecraft:dirt", &[]);
    for x in 0..16 {
        for z in 0..16 {
            let height = if (x, z) == (1, 1) { 200 } else { 198 + x % 4 };
            for y in 197..=201 {
                if (x, y, z) != (1, 201, 1) {
                    let block = if y <= height {
                        dirt
                    } else {
                        state.registry.states.air
                    };
                    state.world.set_block(x, y, z, block);
                }
            }
        }
    }
    let dirt_item = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    state.block_entities.get_mut(&(1, 201, 1)).unwrap().slots[26] = ItemStack::new(dirt_item, 64);
    state.world.fold_overlay();
    for _ in 0..128 {
        plan(&mut state, id);
        if state.settlements.get(id).unwrap().project.is_some() {
            break;
        }
    }
    let project = state
        .settlements
        .get(id)
        .unwrap()
        .project
        .clone()
        .expect("hillside admits first storage");
    assert!(!project.ordered);
    let snapshot = executor::status(&state, id, &project.structure).unwrap();
    let foundation = snapshot
        .remaining
        .iter()
        .find(|m| m.resource == "minecraft:dirt")
        .expect("paid foundation");
    let bill = foundation.quantity;
    assert!(
        project_summary(&state, id)
            .iter()
            .any(|line| line == &format!("minecraft:dirt: {bill}"))
    );
    assert!(bill > 0);
    let floor = project.origin[1];
    let size = storage_size(&state);
    // Real brain/navigation, no teleports, operator commands or terrain edits.
    for now in 0..20_000 {
        state.tick = now;
        state.tick_budget.reset(now);
        let mut changes = crate::boundary::ChangeSet::default();
        let writes_before = state.plugin_block_writes.len();
        crate::settlements::tick(&mut state, &mut changes);
        assert!(
            state.plugin_block_writes.len() - writes_before <= 1,
            "preparation and building must use one-cell admissions"
        );
        if state.settlements.get(id).unwrap().project.is_none() {
            break;
        }
        crate::villager::brain::tick(&mut state, &mut changes);
        state.world.fold_overlay();
    }
    assert!(
        state.settlements.get(id).unwrap().project.is_none(),
        "hillside stalled: {:?}; builder={:?}; villager={:?}",
        project_status(&state, id),
        state.entities.pos(builder),
        state.entities.row(builder).villager
    );
    println!(
        "hillside storage completed at tick {}, origin {:?}, foundation dirt {}",
        state.tick, project.origin, bill
    );
    assert!(
        state
            .settlements
            .get(id)
            .unwrap()
            .buildings
            .values()
            .any(|b| b.kind == BuildingKind::Storage)
    );
    for x in project.origin[0]..project.origin[0] + i32::from(size[0]) {
        for z in project.origin[2]..project.origin[2] + i32::from(size[2]) {
            for y in floor + 1..floor + i32::from(size[1]) {
                assert_ne!(
                    state.world.physics_block_at(x, y, z),
                    Some(dirt),
                    "terrain above the floor must be excavated"
                );
            }
            for y in (198 + x % 4 + 1)..floor {
                assert_eq!(state.world.physics_block_at(x, y, z), Some(dirt));
            }
            assert!(
                !state.registry.collision_boxes
                    [state.world.physics_block_at(x, floor, z).unwrap() as usize]
                    .is_empty()
            );
        }
    }
    let stored: i32 = state
        .block_entities
        .values()
        .flat_map(|be| &be.slots)
        .filter(|s| s.item_id == dirt_item)
        .map(|s| s.count)
        .sum();
    let carried: i32 = state
        .entities
        .row(builder)
        .villager
        .as_ref()
        .unwrap()
        .inventory
        .iter()
        .flatten()
        .filter(|s| s.item_id == dirt_item)
        .map(|s| s.count)
        .sum();
    assert_eq!(stored + carried, 64 - bill as i32);
    let building = state
        .settlements
        .get(id)
        .unwrap()
        .buildings
        .values()
        .find(|b| b.kind == BuildingKind::Storage)
        .unwrap()
        .id;
    assert!(upgrade_available(&state, id, building));
    fund(&mut state, "storage", 2);
    request_upgrade(&mut state, id, building).unwrap();
    complete(&mut state, id, builder);
    assert_eq!(
        state.settlements.get(id).unwrap().buildings[&building].level,
        2
    );
}

#[test]
fn flat_site_keeps_bill_origin_and_same_site_upgrade() {
    use super::tests::{complete, start};
    let (mut state, id, builder) = fixture();
    let materials = fund(&mut state, "storage", 1);
    start(&mut state, id, "storage", 1, None);
    let project = state.settlements.get(id).unwrap().project.clone().unwrap();
    let snapshot = executor::status(&state, id, &project.structure).unwrap();
    assert_eq!(snapshot.origin, project.origin);
    assert_eq!(
        cell_bill(&state, &planned_cells(&state, &project, "oak").unwrap().1)
            .unwrap()
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>(),
        materials.into_iter().collect()
    );
    complete(&mut state, id, builder);
    let building = state
        .settlements
        .get(id)
        .unwrap()
        .buildings
        .values()
        .find(|b| b.kind == BuildingKind::Storage)
        .unwrap()
        .id;
    assert!(executor::built_at(&state, id, "storage", project.origin));
    assert!(!executor::built_at(
        &state,
        id,
        "storage",
        [project.origin[0], project.origin[1] + 1, project.origin[2]]
    ));
    fund(&mut state, "storage", 2);
    request_upgrade(&mut state, id, building).unwrap();
    complete(&mut state, id, builder);
    assert_eq!(
        state.settlements.get(id).unwrap().buildings[&building].level,
        2
    );
}

#[test]
fn terrain_preparation_rejects_four_block_variance_and_protected_cells() {
    let (mut state, id, _) = fixture();
    let origin = [5, 200, 5];
    let size = [3, 5, 3];
    for y in 201..=204 {
        state.world.set_block(6, y, 6, state.registry.states.stone);
    }
    assert!(!site_free(
        &state,
        state.settlements.get(id).unwrap(),
        origin,
        size
    ));
    state.world.set_block(6, 204, 6, state.registry.states.air);
    assert!(site_free(
        &state,
        state.settlements.get(id).unwrap(),
        origin,
        size
    ));
    for name in [
        "minecraft:oak_planks",
        "minecraft:dirt_path",
        "minecraft:water",
    ] {
        let block = crate::settlements::allocator::tests::block(&state, name, &[]);
        state.world.set_block(6, 203, 6, block);
        assert!(
            !site_free(&state, state.settlements.get(id).unwrap(), origin, size),
            "{name}"
        );
    }
}

fn unfunded_near_foundation() -> (TickState, SettlementId, [i32; 3]) {
    let (mut state, id, _) = fixture();
    fund(&mut state, "storage", 1);
    solid_plateau(&mut state);
    let size = storage_size(&state);
    let anchor = state.settlements.get(id).unwrap().anchor;
    let near = centred(anchor, size, 0, 0);
    let far = centred(anchor, size, -12, 0);
    clear(&mut state, near, size);
    clear(&mut state, far, size);
    // The near pocket needs one dirt below its authored floor. The stock
    // funds the complete building, but contains no foundation dirt.
    let (x, z) = (near[0] + 1, near[2] + 1);
    state
        .world
        .set_block(x, 198, z, state.registry.states.stone);
    for y in 199..=200 {
        state.world.set_block(x, y, z, state.registry.states.air);
    }
    state.world.fold_overlay();
    assert!(site_free(
        &state,
        state.settlements.get(id).unwrap(),
        near,
        size
    ));
    assert!(site_free(
        &state,
        state.settlements.get(id).unwrap(),
        far,
        size
    ));
    (state, id, far)
}

#[test]
fn hillside_review_autonomous_search_skips_unfunded_foundation_within_budget() {
    let (mut state, id, far) = unfunded_near_foundation();
    plan(&mut state, id);
    let record = state.settlements.get(id).unwrap();
    assert!(
        record.project.is_none(),
        "far site is outside the first pass"
    );
    assert_eq!(
        record.construction_cursor, SITE_ATTEMPTS,
        "unfunded site cannot reset the cursor"
    );
    for _ in 0..32 {
        plan(&mut state, id);
        if state.settlements.get(id).unwrap().project.is_some() {
            break;
        }
    }
    let project = state
        .settlements
        .get(id)
        .unwrap()
        .project
        .as_ref()
        .expect("far funded site");
    assert_eq!(project.origin, far);
}

#[test]
fn hillside_review_owner_search_skips_unfunded_foundation() {
    let (mut state, id, far) = unfunded_near_foundation();
    request_build(&mut state, id, "storage").expect("far funded site");
    let project = state.settlements.get(id).unwrap().project.as_ref().unwrap();
    assert_eq!(project.origin, far);
    assert!(project.ordered);
}

#[test]
fn hillside_review_floor_under_overhang_preserves_fluids_and_protected_blocks() {
    let (mut state, id, _) = fixture();
    let origin = [5, 200, 5];
    let size = [3, 5, 3];
    state
        .world
        .set_block(6, 201, 6, state.registry.states.stone);
    assert!(
        site_free(&state, state.settlements.get(id).unwrap(), origin, size),
        "solid natural ground below the overhang is replaceable"
    );
    for name in [
        "minecraft:water",
        "minecraft:lava",
        "minecraft:dirt_path",
        "minecraft:farmland",
        "minecraft:wheat",
        "minecraft:torch",
    ] {
        let block = crate::settlements::allocator::tests::block(&state, name, &[]);
        state.world.set_block(6, 200, 6, block);
        assert!(
            !site_free(&state, state.settlements.get(id).unwrap(), origin, size),
            "protected floor under overhang: {name}"
        );
    }
}
