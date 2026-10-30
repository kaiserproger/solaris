use super::*;

fn doorway(state: &mut TickState, material: &str) -> BlockPos {
    let door = BlockPos { x: 8, y: 201, z: 8 };
    for x in 0..16 {
        for y in 201..205 {
            if x != door.x {
                state.world.set_block(x, y, 8, state.registry.states.stone);
            }
        }
    }
    for (y, half) in [(201, "lower"), (202, "upper")] {
        let id = block(
            state,
            material,
            &[("half", half), ("facing", "north"), ("open", "false")],
        );
        state.world.set_block(8, y, 8, id);
    }
    state.world.fold_overlay();
    door
}

fn door_open(state: &TickState, pos: BlockPos) -> bool {
    let id = state.world.block_at(pos.x, pos.y, pos.z).unwrap();
    state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(id))
        .unwrap()
        .properties
        .iter()
        .any(|(key, value)| key == "open" && value == "true")
}

#[test]
fn wooden_door_route_reaches_other_side_but_iron_remains_blocked() {
    for (material, expected) in [
        ("minecraft:oak_door", true),
        ("minecraft:crimson_door", true),
        ("minecraft:iron_door", false),
        ("minecraft:copper_door", false),
    ] {
        let mut state = test_state();
        doorway(&mut state, material);
        let mut cache = PoiCache::default();
        let mut path = Vec::new();
        state.tick_budget.reset(state.tick);
        assert_eq!(
            find_path(
                &mut state,
                &mut cache,
                [8.5, 201.0, 6.5],
                BlockPos {
                    x: 8,
                    y: 201,
                    z: 10
                },
                0.3,
                1.95,
                &mut path
            ),
            expected,
            "{material}"
        );
        if expected {
            assert!(path.contains(&BlockPos { x: 8, y: 201, z: 8 }));
            state.tick_budget.reset(state.tick + 1);
            assert!(
                !find_path_exact(
                    &mut state,
                    &mut cache,
                    [8.5, 201.0, 6.5],
                    BlockPos {
                        x: 8,
                        y: 201,
                        z: 10
                    },
                    0.3,
                    1.95,
                    &mut path
                ),
                "generic NPC routes must not assume the villager's door interaction"
            );
        }
    }
}

#[test]
fn bound_bedless_nitwit_enters_occupied_house_at_night() {
    let mut state = control_state();
    doorway(&mut state, "minecraft:oak_door");
    for z in 9..=14 {
        for y in 201..=203 {
            state.world.set_block(6, y, z, state.registry.states.stone);
            state.world.set_block(10, y, z, state.registry.states.stone);
        }
    }
    for x in 6..=10 {
        for y in 201..=203 {
            state.world.set_block(x, y, 14, state.registry.states.stone);
        }
        for z in 8..=14 {
            state
                .world
                .set_block(x, 204, z, state.registry.states.stone);
        }
    }
    let home = bed(&mut state, 8, 12);
    let outdoor = bed(&mut state, 12, 5);
    state.world.fold_overlay();
    let settlement = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "town".into(),
            "owner".into(),
            [8, 201, 8],
            32,
        )
        .unwrap();
    let building_id = crate::settlements::buildings::BuildingId((settlement.0 << 32) | 1);
    let record = state.settlements.get_mut(settlement).unwrap();
    record.building_sequence = 1;
    record.buildings.insert(
        building_id,
        crate::settlements::buildings::Building {
            id: building_id,
            settlement,
            kind: crate::settlements::buildings::BuildingKind::Workshop(Profession::Farmer),
            level: 1,
            bounds: mc_script::ScriptWorldBounds::new([6, 200, 8], [10, 204, 14]).unwrap(),
            work_points: vec![],
            revision: 1,
            natural_zone: false,
        },
    );
    let outdoor_owner = villager(&mut state, [12.5, 201.0, 5.5]);
    let v = state
        .entities
        .row_mut(outdoor_owner)
        .villager
        .as_mut()
        .unwrap();
    v.settlement = Some(settlement);
    v.home = Some(outdoor);
    let owner = villager(&mut state, [8.5, 201.0, 12.5]);
    let v = state.entities.row_mut(owner).villager.as_mut().unwrap();
    v.settlement = Some(settlement);
    v.home = Some(home);
    let nitwit = villager(&mut state, [8.5, 201.0, 5.5]);
    let v = state.entities.row_mut(nitwit).villager.as_mut().unwrap();
    v.settlement = Some(settlement);
    v.data.profession = Profession::Nitwit;
    let unemployed = villager(&mut state, [7.5, 201.0, 4.5]);
    state
        .entities
        .row_mut(unemployed)
        .villager
        .as_mut()
        .unwrap()
        .settlement = Some(settlement);
    let child = villager(&mut state, [9.5, 201.0, 4.5]);
    let v = state.entities.row_mut(child).villager.as_mut().unwrap();
    v.settlement = Some(settlement);
    v.age = -10_000;
    state.day_time = 18000;
    for _ in 0..600 {
        crate::tick::run_tick(&mut state, &[]);
    }
    for resident in [nitwit, unemployed, child, outdoor_owner] {
        let v = state.entities.row(resident).villager.as_ref().unwrap();
        if resident != outdoor_owner {
            assert!(v.home.is_none(), "all beds belong to other residents");
        }
        let [x, _, z] = state.entities.pos(resident);
        assert!(
            x > 6.0 && x < 10.0 && z > 8.5 && z < 14.0,
            "resident stayed outside: {:?}",
            state.entities.pos(resident)
        );
    }
    let cells: std::collections::BTreeSet<_> = [nitwit, unemployed, child]
        .into_iter()
        .map(|slot| {
            let [x, _, z] = state.entities.pos(slot);
            (x.floor() as i32, z.floor() as i32)
        })
        .collect();
    assert_eq!(cells.len(), 3, "bedless residents must not share a cell");
}

#[test]
fn night_shelter_does_not_override_panic_or_soldier_duty() {
    let mut state = control_state();
    let settlement = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "town".into(),
            "owner".into(),
            [8, 201, 8],
            32,
        )
        .unwrap();
    let panicking = villager(&mut state, [7.5, 201.0, 5.5]);
    let v = state.entities.row_mut(panicking).villager.as_mut().unwrap();
    v.settlement = Some(settlement);
    v.panic_until = state.tick + 1_000;
    let soldier = villager(&mut state, [9.5, 201.0, 5.5]);
    let v = state.entities.row_mut(soldier).villager.as_mut().unwrap();
    v.settlement = Some(settlement);
    v.data.profession = Profession::Soldier;
    state.day_time = 18000;
    crate::tick::run_tick(&mut state, &[]);
    assert_eq!(
        state
            .entities
            .row(panicking)
            .villager
            .as_ref()
            .unwrap()
            .activity,
        Activity::Panic
    );
    assert_eq!(
        state
            .entities
            .row(soldier)
            .villager
            .as_ref()
            .unwrap()
            .activity,
        Activity::Work
    );
}

#[test]
fn resident_opens_both_door_halves_and_closes_after_entering_bed_at_night() {
    let mut state = control_state();
    let door = doorway(&mut state, "minecraft:oak_door");
    let home = bed(&mut state, 8, 12);
    state.world.fold_overlay();
    state.day_time = 13000;
    let slot = villager(&mut state, [8.5, 201.0, 5.5]);
    state.entities.row_mut(slot).villager.as_mut().unwrap().home = Some(home);
    let mut opened = false;
    let mut closed = false;
    for _ in 0..240 {
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        let changes = &output.changes;
        if changes.blocks.iter().any(|change| change.position == door) {
            let upper = BlockPos {
                y: door.y + 1,
                ..door
            };
            assert!(changes.blocks.iter().any(|change| change.position == upper));
            let journal = crate::core::journal_entries_for_tick(&mut state, &output);
            for position in [door, upper] {
                assert!(journal.iter().any(|entry| matches!(entry,
                    crate::journal::JournalEntry::Block { pos, state: id }
                    if *pos == [position.x, position.y, position.z]
                        && Some(*id) == state.world.block_at(position.x, position.y, position.z))));
            }
            if door_open(&state, door) {
                opened = true;
                assert!(door_open(&state, upper));
            } else if opened {
                closed = true;
                assert!(!door_open(&state, upper));
            }
        }
        if state.entities.row(slot).villager.as_ref().unwrap().sleeping {
            break;
        }
    }
    assert!(opened, "villager never opened the door");
    assert!(closed, "villager left its door open");
    assert!(!door_open(&state, door));
    assert!(
        state.entities.row(slot).villager.as_ref().unwrap().sleeping,
        "resident never reached its bed: {:?}",
        state.entities.pos(slot)
    );
}

#[test]
fn interrupted_route_closes_only_the_door_the_resident_opened() {
    let mut state = test_state();
    let door = doorway(&mut state, "minecraft:oak_door");
    let slot = villager(&mut state, [8.5, 201.0, 7.5]);
    let mut v = VillagerState::default();
    v.navigation_path.push(door);
    let mut changes = ChangeSet::default();
    crate::villager::doors::interact(&mut state, slot, &mut v, &mut changes);
    assert!(door_open(&state, door));
    assert_eq!(v.opened_doors, [door]);
    v.navigation_path.clear();
    v.activity = Activity::Rest;
    v.sleeping = true;
    state.entities.set_pos(slot, [8.5, 201.0, 10.5]);
    crate::villager::doors::interact(&mut state, slot, &mut v, &mut changes);
    assert!(!door_open(&state, door));
    assert!(v.opened_doors.is_empty());

    // A pre-opened player door must remain untouched and unowned.
    let cells = crate::placement::use_block(&state.registry.blocks, door, 0.0, &|p| {
        state.world.block_at(p.x, p.y, p.z)
    })
    .unwrap();
    for (p, id) in cells {
        state.world.set_block(p.x, p.y, p.z, id);
    }
    state.entities.set_pos(slot, [8.5, 201.0, 7.5]);
    v.sleeping = false;
    v.navigation_path.push(door);
    changes.blocks.clear();
    crate::villager::doors::interact(&mut state, slot, &mut v, &mut changes);
    assert!(v.opened_doors.is_empty());
    assert!(changes.blocks.is_empty());
    assert!(door_open(&state, door));
}

#[test]
fn opening_door_preserves_own_and_neighbour_routes_before_and_after_fold() {
    let mut state = test_state();
    let door = doorway(&mut state, "minecraft:oak_door");
    let slot = villager(&mut state, [8.5, 201.0, 7.5]);
    let target = BlockPos { z: 10, ..door };
    let mut opener = VillagerState {
        navigation_path: vec![target, BlockPos { z: 9, ..door }, door],
        navigation_target: Some(target),
        activity: Activity::Work,
        ..Default::default()
    };
    let mut neighbour = opener.clone();
    capture_route(&state, &mut opener, state.entities.pos(slot), true);
    capture_route(&state, &mut neighbour, [8.5, 201.0, 6.5], true);
    let mut generic = neighbour.clone();
    capture_route(&state, &mut generic, [8.5, 201.0, 6.5], false);
    // Sample the closed panel through real skipped-motion collision before opening.
    state.entities.row_mut(slot).villager = Some(Box::new(opener));
    for _ in 0..2 {
        move_skipped(&mut state, slot, false, false, Activity::Work);
    }
    opener = *state
        .entities
        .row_mut_unmarked(slot)
        .villager
        .take()
        .unwrap();
    assert!(!door_open(&state, door));
    assert!(
        opener
            .motion_geometry
            .blocks
            .iter()
            .any(|&id| crate::villager::doors::wooden(&state.registry, id))
    );
    let mut geometry = MotionGeometryCache::default();
    assert!(geometry.begin(&state, &opener, 0.3, 1.95));
    assert!(geometry.standing(&state, &opener, door, true).is_some());
    assert!(geometry.standing.is_some());
    let mut changes = ChangeSet::default();
    crate::villager::doors::interact(&mut state, slot, &mut opener, &mut changes);
    assert!(door_open(&state, door));
    assert!(
        !route_changed(&state, &opener),
        "own door opening must not cancel the route"
    );
    assert!(
        !route_changed(&state, &neighbour),
        "a neighbour's route must remain usable"
    );
    assert!(
        route_changed(&state, &generic),
        "physical NPC routes must observe the door flip"
    );
    assert!(geometry.begin(&state, &opener, 0.3, 1.95));
    assert!(
        geometry.standing.is_none(),
        "physical door shapes must still invalidate cached collision"
    );
    state.entities.row_mut(slot).villager = Some(Box::new(opener));
    for _ in 0..20 {
        move_skipped(&mut state, slot, false, false, Activity::Work);
        if state.entities.pos(slot)[2] > 9.3 {
            break;
        }
    }
    assert!(
        state.entities.pos(slot)[2] > 9.3,
        "skipped movement must cross an opened door"
    );
    state.world.fold_overlay();
    assert!(!route_changed(
        &state,
        state.entities.row(slot).villager.as_ref().unwrap()
    ));
    assert!(!route_changed(&state, &neighbour));
    assert!(route_changed(&state, &generic));
    state
        .world
        .set_block(8, 201, 9, state.registry.states.stone);
    assert!(
        route_changed(&state, &neighbour),
        "a real pending obstacle must still invalidate"
    );
    let cells = crate::placement::use_block(&state.registry.blocks, door, 0.0, &|p| {
        state.world.block_at(p.x, p.y, p.z)
    })
    .unwrap();
    for (p, id) in cells {
        state.world.set_block(p.x, p.y, p.z, id);
    }
    assert!(
        route_changed(&state, &neighbour),
        "a door flip must not mask a pending obstacle"
    );
    state.world.fold_overlay();
    assert!(
        route_changed(&state, &neighbour),
        "folding must not hide the obstacle edit"
    );
}

#[test]
fn opener_waits_for_other_villager_body_and_next_path_cells_before_closing() {
    let mut state = test_state();
    let door = doorway(&mut state, "minecraft:oak_door");
    let opener = villager(&mut state, [8.5, 201.0, 7.5]);
    let follower = villager(&mut state, [8.5, 201.0, 6.5]);
    let mut v = VillagerState {
        navigation_path: vec![door],
        ..Default::default()
    };
    let mut changes = ChangeSet::default();
    crate::villager::doors::interact(&mut state, opener, &mut v, &mut changes);
    assert!(door_open(&state, door));
    v.navigation_path.clear();
    state.entities.set_pos(opener, [8.5, 201.0, 10.5]);
    state.entities.set_pos(follower, [8.5, 201.0, 8.5]);
    crate::villager::doors::interact(&mut state, opener, &mut v, &mut changes);
    assert!(
        door_open(&state, door),
        "do not close onto another villager's body"
    );
    state.entities.set_pos(follower, [8.5, 201.0, 7.0]);
    state
        .entities
        .row_mut(follower)
        .villager
        .as_mut()
        .unwrap()
        .navigation_path = vec![door, BlockPos { z: 7, ..door }];
    crate::villager::doors::interact(&mut state, opener, &mut v, &mut changes);
    assert!(
        door_open(&state, door),
        "keep open for another villager's next two path cells"
    );
    state
        .entities
        .row_mut(follower)
        .villager
        .as_mut()
        .unwrap()
        .navigation_path
        .clear();
    state.entities.set_pos(follower, [8.5, 201.0, 5.5]);
    crate::villager::doors::interact(&mut state, opener, &mut v, &mut changes);
    assert!(!door_open(&state, door), "close after everyone has passed");
}

#[test]
fn overlay_sampled_shapes_expire_even_when_door_toggles_cancel_before_fold() {
    let mut state = test_state();
    let door = doorway(&mut state, "minecraft:oak_door");
    let slot = villager(&mut state, [8.5, 201.0, 7.5]);
    let mut v = VillagerState {
        navigation_path: vec![door],
        ..Default::default()
    };
    capture_route(&state, &mut v, state.entities.pos(slot), true);
    let before = state.world.chunk_revision(0, 0);
    let mut changes = ChangeSet::default();
    crate::villager::doors::interact(&mut state, slot, &mut v, &mut changes);
    let mut geometry = MotionGeometryCache::default();
    assert!(geometry.begin(&state, &v, 0.3, 1.95));
    assert!(geometry.standing(&state, &v, door, true).is_some());
    let cells = crate::placement::use_block(&state.registry.blocks, door, 0.0, &|p| {
        state.world.block_at(p.x, p.y, p.z)
    })
    .unwrap();
    for (p, id) in cells {
        state.world.set_block(p.x, p.y, p.z, id);
    }
    state.world.fold_overlay();
    assert_eq!(
        state.world.chunk_revision(0, 0),
        before,
        "net-zero overlay has no wire revision"
    );
    assert!(!route_changed(&state, &v));
    assert!(geometry.begin(&state, &v, 0.3, 1.95));
    assert!(
        geometry.standing.is_none(),
        "never reuse a shape sampled from a folded overlay"
    );
}
