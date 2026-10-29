use super::{Activity, VillagerProfession, VillagerState, mining};
use crate::boundary::{BlockPos, ChangeSet, ChunkPos};
use crate::entities::EntitySlot;
use crate::settlements::buildings::{Building, BuildingId, BuildingKind};
use crate::tick::TickState;
use mc_data::ItemStack;
use mc_script::ScriptWorldBounds;

pub(super) fn fixture(kind: BuildingKind) -> (TickState, EntitySlot, Box<VillagerState>) {
    let mut state = super::conversion_tests::state();
    state
        .world
        .ensure_blocking(&rustc_hash::FxHashMap::from_iter([(
            ChunkPos { x: 0, z: 0 },
            0,
        )]));
    // Workers only act in simulated chunks; the fixture chunk is one.
    state
        .tickets
        .insert("worker-fixture".into(), (ChunkPos { x: 0, z: 0 }, 0));
    state.refresh_active_chunks(0);
    for x in 0..16 {
        for z in 0..16 {
            for y in 190..206 {
                state.world.set_block(
                    x,
                    y,
                    z,
                    if y <= 200 {
                        state.registry.states.stone
                    } else {
                        state.registry.states.air
                    },
                );
            }
        }
    }
    let settlement = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "workers".into(),
            "owner".into(),
            [8, 201, 8],
            12,
        )
        .unwrap();
    let workstation = "crafting_table";
    let job = BlockPos { x: 3, y: 201, z: 3 };
    let block = block(&state, workstation);
    state.world.set_block(job.x, job.y, job.z, block);
    let id = BuildingId((settlement.0 << 32) | 1);
    let record = state.settlements.get_mut(settlement).unwrap();
    record.buildings.insert(
        id,
        Building {
            id,
            settlement,
            kind,
            level: 1,
            bounds: if kind == BuildingKind::Mine {
                ScriptWorldBounds::new([1, 192, 1], [14, 204, 14]).unwrap()
            } else {
                ScriptWorldBounds::new([1, 200, 1], [5, 204, 5]).unwrap()
            },
            work_points: vec![[3, 201, 3]],
            revision: 1,
            natural_zone: kind == BuildingKind::Mine,
        },
    );
    record.building_sequence = 1;
    let entity_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let entity = state.spawn_mob(entity_type, [3.5, 201.0, 3.5], 20.0);
    let slot = state.entities.slot_of(entity).unwrap();
    let mut v = state.entities.row_mut(slot).villager.take().unwrap();
    v.settlement = Some(settlement);
    v.job_site = Some(job);
    v.data.profession = if kind == BuildingKind::Mine {
        VillagerProfession::Miner
    } else {
        VillagerProfession::Lumberjack
    };
    v.activity = Activity::Work;
    state.day_time = 3000;
    (state, slot, v)
}

pub(super) fn block(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap()
        .default
        .0
}

pub(super) fn storage(state: &mut TickState, v: &VillagerState, position: [i32; 3]) {
    let chest = block(state, "chest");
    state
        .world
        .set_block(position[0], position[1], position[2], chest);
    state.block_entities.insert(
        (position[0], position[1], position[2]),
        crate::tick::BlockEntityState {
            slots: vec![ItemStack::EMPTY; 27],
            furnace: None,
        },
    );
    let settlement = v.settlement.unwrap();
    let id = BuildingId((settlement.0 << 32) | 2);
    let record = state.settlements.get_mut(settlement).unwrap();
    record.buildings.insert(
        id,
        Building {
            id,
            settlement,
            kind: BuildingKind::Storage,
            level: 1,
            bounds: ScriptWorldBounds::new(
                position,
                [position[0] + 2, position[1] + 2, position[2] + 2],
            )
            .unwrap(),
            work_points: vec![position],
            revision: 1,
            natural_zone: false,
        },
    );
    record.building_sequence = 2;
}

#[test]
fn miner_skips_cave_below_certified_surface_when_trying_alternate_lane() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    state.settlements.get_mut(v.settlement.unwrap()).unwrap().radius = 100;
    let mut changes = ChangeSet::default();
    state.tick = 100;
    mining::work(&mut state, slot, &mut v, &mut changes);
    for y in 184..=205 {
        let block = if (185..=186).contains(&y) {
            state.registry.states.air
        } else {
            state.registry.states.stone
        };
        state.world.set_block(3, y, 4, block);
    }
    v.mining_progress.lane = 1;
    v.mining_progress.cell = 0;
    v.mining_progress.layer = 0;
    v.mining_progress.last_open = None;
    let target = mining::work(&mut state, slot, &mut v, &mut changes);
    assert_ne!(target, Some(BlockPos { x: 3, y: 185, z: 4 }),
        "an underground cave outside the certified surface cannot be a new entrance");
    assert_eq!(v.mining_progress.lane, 2);
}

#[test]
fn miner_staircase_and_branches_stay_inside_zone_and_one_break_per_attempt() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    let bounds = crate::settlements::buildings::building_for_job(&state, &v)
        .unwrap()
        .bounds;
    let mut changes = ChangeSet::default();
    for tick in 0..600 {
        state.tick = tick * 20;
        let before = changes.blocks.len();
        let target = mining::work(&mut state, slot, &mut v, &mut changes);
        assert!(
            changes.blocks.len() - before <= 1,
            "fixed one-block mutation budget"
        );
        if let Some(target) = target {
            state.entities.set_pos(
                slot,
                [
                    f64::from(target.x) + 0.5,
                    f64::from(target.y),
                    f64::from(target.z) + 0.5,
                ],
            );
        }
    }
    assert!(
        changes.blocks.len() > 40,
        "miner must get beyond the stairs into branches"
    );
    assert!(v.mining_progress.cell > 20);
    for change in &changes.blocks {
        for (coordinate, (min, max)) in [change.position.x, change.position.y, change.position.z]
            .into_iter()
            .zip(bounds.min.into_iter().zip(bounds.max))
        {
            assert!((min..=max).contains(&coordinate));
        }
        assert_eq!(change.state, state.registry.states.air);
    }
    let dirt = mining::item_id(&state, "cobblestone").unwrap();
    assert!(
        v.inventory
            .iter()
            .flatten()
            .any(|s| s.item_id == dirt && s.count > 0)
    );
}

#[test]
fn native_miner_changes_publish_the_same_tick_chunk_afterimage() {
    let (mut state, slot, v) = fixture(BuildingKind::Mine);
    state.entities.row_mut(slot).villager = Some(v);
    state
        .tickets
        .insert("miner-publication".into(), (ChunkPos { x: 0, z: 0 }, 0));
    state.refresh_active_chunks(state.tick);
    for _ in 0..200 {
        let revision = state.world.chunk_revision(0, 0);
        let route_revision = state.world.villager_route_revision(0, 0);
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        let Some(change) = output.changes.blocks.iter().find(|change| {
            change.position == BlockPos { x: 4, y: 200, z: 3 }
                && change.state == state.registry.states.air
        }) else {
            continue;
        };
        assert_eq!(state.world.block_at(4, 200, 3), Some(change.state));
        let snapshot = output
            .snapshot
            .chunks
            .iter()
            .find(|chunk| chunk.pos == ChunkPos { x: 0, z: 0 })
            .unwrap();
        assert_eq!(
            snapshot.chunk.get_block(4, 200, 3).unwrap().0,
            change.state,
            "the client must not receive an old full chunk after the miner's air delta"
        );
        assert!(
            snapshot.revision > revision,
            "late edits advance the wire revision"
        );
        assert!(
            state.world.villager_route_revision(0, 0) > route_revision,
            "folding the pending route edits must retain route invalidation"
        );
        let entries = crate::core::journal_entries_for_tick(&mut state, &output);
        let directory = tempfile::tempdir().unwrap();
        {
            let mut writer = crate::journal::JournalWriter::open(directory.path(), 1).unwrap();
            writer.append(1, output.tick, &entries).unwrap();
        }
        let batches =
            crate::journal::read_complete_batches_after(&directory.path().join("journal"), 0)
                .unwrap();
        let replay = crate::journal::replay_batches(crate::journal::Replay::default(), &batches);
        assert_eq!(replay.tick, output.tick);
        assert_eq!(
            replay.blocks.get(&(4, 200, 3)),
            Some(&state.registry.states.air),
            "the same committed mined afterimage survives a closed writer and disk WAL replay"
        );
        return;
    }
    panic!("miner never excavated its first staircase floor");
}

#[test]
fn surface_mine_uses_clear_headroom_without_digging_outside_certificate() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    let settlement = v.settlement.unwrap();
    let building = crate::settlements::buildings::building_for_job(&state, &v)
        .unwrap()
        .id;
    state
        .settlements
        .get_mut(settlement)
        .unwrap()
        .buildings
        .get_mut(&building)
        .unwrap()
        .bounds
        .max[1] = v.job_site.unwrap().y;
    let bounds = crate::settlements::buildings::building_for_job(&state, &v)
        .unwrap()
        .bounds;
    let mut changes = ChangeSet::default();
    for tick in 0..6 {
        state.tick = tick * 20;
        mining::work(&mut state, slot, &mut v, &mut changes);
    }
    assert!(
        !changes.blocks.is_empty(),
        "surface entrance must start its staircase"
    );
    assert!(
        changes
            .blocks
            .iter()
            .all(|change| change.position.y <= bounds.max[1])
    );
    assert!(v.inventory.iter().flatten().any(|stack| stack.count > 0));
}

#[test]
fn surface_mine_never_clears_solid_headroom_above_certificate() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    let settlement = v.settlement.unwrap();
    let building = crate::settlements::buildings::building_for_job(&state, &v)
        .unwrap()
        .id;
    state
        .settlements
        .get_mut(settlement)
        .unwrap()
        .buildings
        .get_mut(&building)
        .unwrap()
        .bounds
        .max[1] = v.job_site.unwrap().y;
    state
        .world
        .set_block(4, 202, 3, state.registry.states.stone);
    let mut changes = ChangeSet::default();
    mining::work(&mut state, slot, &mut v, &mut changes);
    assert!(
        changes.blocks.is_empty(),
        "a certificate never authorizes breaking above it"
    );
    assert_eq!(
        state.world.block_at(4, 202, 3),
        Some(state.registry.states.stone)
    );
    assert_eq!(
        v.mining_progress.lane, 1,
        "solid headroom abandons the unsafe lane"
    );
}

#[test]
fn miner_refuses_fluids_adjacent_fluids_and_air_pockets_before_breaking() {
    for (position, path) in [
        ([4, 200, 3], "water"),
        ([4, 200, 3], "lava"),
        ([4, 200, 3], "air"),
        ([4, 200, 4], "water"),
        ([4, 199, 3], "lava"),
        ([4, 200, 4], "air"),
    ] {
        let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
        let id = block(&state, path);
        state
            .world
            .set_block(position[0], position[1], position[2], id);
        let mut changes = ChangeSet::default();
        for tick in 0..8 {
            state.tick = tick * 20;
            mining::work(&mut state, slot, &mut v, &mut changes);
            if tick == 0 {
                assert!(
                    changes.blocks.is_empty(),
                    "reject {path} before opening any face"
                );
            }
        }
        assert!(
            changes.blocks.iter().all(|change| {
                let p = change.position;
                (p.x - position[0]).abs() + (p.y - position[1]).abs() + (p.z - position[2]).abs()
                    > 1
            }),
            "a bypass must not expose {path} at {position:?}"
        );
        assert_eq!(
            state.world.block_at(position[0], position[1], position[2]),
            Some(id)
        );
    }
}

#[test]
fn miner_deposits_at_day_end_and_resumes_cursor_after_upgrade() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    let mut changes = ChangeSet::default();
    for tick in 0..3 {
        state.tick = tick * 20;
        mining::work(&mut state, slot, &mut v, &mut changes);
    }
    assert!(v.inventory.iter().any(Option::is_some));
    let cursor = v.mining_progress.cell;
    storage(&mut state, &v, [3, 201, 5]);
    state.day_time = 9000;
    v.activity = Activity::Meet;
    mining::work(&mut state, slot, &mut v, &mut changes);
    assert!(v.inventory.iter().all(Option::is_none));
    let chest = state.block_entities.get(&(3, 201, 5)).unwrap();
    assert!(chest.slots.iter().any(|s| !s.is_empty()));
    let id = BuildingId(v.mining_progress.building);
    state
        .settlements
        .get_mut(v.settlement.unwrap())
        .unwrap()
        .buildings
        .get_mut(&id)
        .unwrap()
        .revision += 1;
    state.day_time = 3000;
    v.activity = Activity::Work;
    state.tick += 1;
    mining::work(&mut state, slot, &mut v, &mut changes);
    assert_eq!(
        v.mining_progress.cell, cursor,
        "upgrading must not rewind into its own air tunnel"
    );
}

#[test]
fn miner_full_inventory_routes_to_storage_without_digging_and_retains_when_full() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    storage(&mut state, &v, [12, 201, 12]);
    let cobble = mining::item_id(&state, "cobblestone").unwrap();
    v.inventory = std::array::from_fn(|_| Some(ItemStack::new(cobble, 64)));
    let mut changes = ChangeSet::default();
    assert_eq!(
        mining::work(&mut state, slot, &mut v, &mut changes),
        Some(BlockPos {
            x: 12,
            y: 201,
            z: 12
        })
    );
    assert!(changes.blocks.is_empty());
    state.entities.set_pos(slot, [12.5, 201.0, 11.5]);
    mining::work(&mut state, slot, &mut v, &mut changes);
    assert!(v.inventory.iter().all(Option::is_none));
    assert_eq!(
        state
            .block_entities
            .get(&(12, 201, 12))
            .unwrap()
            .slots
            .iter()
            .map(|s| s.count)
            .sum::<i32>(),
        512
    );
    let stock = state.block_entities.get_mut(&(12, 201, 12)).unwrap();
    stock.slots.fill(ItemStack::new(cobble, 64));
    v.inventory = std::array::from_fn(|_| Some(ItemStack::new(cobble, 64)));
    let carry = v.inventory.clone();
    let before = changes.blocks.len();
    assert!(mining::work(&mut state, slot, &mut v, &mut changes).is_none());
    assert_eq!(
        v.inventory, carry,
        "full storage must retain carried resources"
    );
    assert_eq!(
        changes.blocks.len(),
        before,
        "do not dig while waiting to unload"
    );
}

#[test]
fn miner_respects_work_hours_mob_griefing_and_wire_profession_looks() {
    assert_eq!(
        VillagerProfession::Miner.profession_id(),
        VillagerProfession::Toolsmith.profession_id()
    );
    assert_eq!(
        VillagerProfession::Lumberjack.profession_id(),
        VillagerProfession::Fletcher.profession_id()
    );
    for reason in 0..4 {
        let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
        match reason {
            0 => {
                state.day_time = 1000;
                v.activity = Activity::Idle;
            }
            1 => {
                v.age = -10;
            }
            2 => {
                v.activity = Activity::Panic;
            }
            _ => {
                state
                    .gamerules
                    .insert("mob_griefing".into(), "false".into());
            }
        }
        let mut changes = ChangeSet::default();
        for tick in 0..5 {
            state.tick = tick * 20;
            mining::work(&mut state, slot, &mut v, &mut changes);
        }
        assert!(changes.blocks.is_empty());
    }
}

#[test]
fn certified_miner_brain_uses_job_mapping_and_existing_navigation() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    v.data.profession = VillagerProfession::None;
    state.entities.row_mut(slot).villager = Some(v);
    state.entities.set_pos(slot, [2.5, 201.0, 3.5]);
    state
        .tickets
        .insert("worker-fixture".into(), (ChunkPos { x: 0, z: 0 }, 0));
    state.refresh_active_chunks(0);
    for _ in 0..300 {
        crate::tick::run_tick(&mut state, &[]);
    }
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(v.data.profession, VillagerProfession::Miner);
    assert!(
        v.mining_progress.cell >= 2,
        "the brain must dispatch physical mining"
    );
    assert!(
        state.entities.pos(slot)[1] < 201.0,
        "the miner must follow its excavated stairs"
    );
}

#[test]
fn miner_skips_a_permanent_hazard_and_opens_a_different_safe_branch() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    let lava = block(&state, "lava");
    state.world.set_block(4, 200, 3, lava);
    let mut changes = ChangeSet::default();
    for tick in 0..160 {
        state.tick = tick * 20;
        if let Some(target) = mining::work(&mut state, slot, &mut v, &mut changes) {
            state.entities.set_pos(
                slot,
                [
                    f64::from(target.x) + 0.5,
                    f64::from(target.y),
                    f64::from(target.z) + 0.5,
                ],
            );
        }
    }
    assert!(
        changes.blocks.len() > 3,
        "one unsafe cell must not stop all mine output"
    );
    assert_eq!(state.world.block_at(4, 200, 3), Some(lava));
    assert!(
        changes.blocks.iter().all(|change| {
            let p = change.position;
            (p.x - 4).abs() + (p.y - 200).abs() + (p.z - 3).abs() > 1
        }),
        "no excavation may expose the lava face"
    );
}

#[test]
fn miner_refuses_waterlogged_blocks_and_fluid_holding_neighbors() {
    for name in [
        "oak_stairs",
        "kelp",
        "kelp_plant",
        "seagrass",
        "tall_seagrass",
        "bubble_column",
        "powder_snow",
    ] {
        let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
        let id = block(&state, name);
        let id = if name == "oak_stairs" {
            crate::fluids::waterlogged(&state, id, true).unwrap()
        } else {
            id
        };
        state.world.set_block(4, 200, 4, id);
        let mut changes = ChangeSet::default();
        for tick in 0..3 {
            state.tick = tick * 20;
            mining::work(&mut state, slot, &mut v, &mut changes);
        }
        assert!(changes.blocks.is_empty(), "must not expose {name}");
        assert_eq!(state.world.block_at(4, 200, 4), Some(id));
    }
}

#[test]
fn miner_resets_cursor_on_bounds_change_but_not_level_only_upgrade() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    let mut changes = ChangeSet::default();
    for tick in 0..3 {
        state.tick = tick * 20;
        mining::work(&mut state, slot, &mut v, &mut changes);
    }
    assert_eq!(v.mining_progress.cell, 1);
    let id = BuildingId(v.mining_progress.building);
    state
        .settlements
        .get_mut(v.settlement.unwrap())
        .unwrap()
        .buildings
        .get_mut(&id)
        .unwrap()
        .revision += 1;
    state.day_time = 1000;
    v.activity = Activity::Idle;
    mining::work(&mut state, slot, &mut v, &mut changes);
    assert_eq!(v.mining_progress.cell, 1);
    let building = state
        .settlements
        .get_mut(v.settlement.unwrap())
        .unwrap()
        .buildings
        .get_mut(&id)
        .unwrap();
    building.bounds = ScriptWorldBounds::new([1, 191, 1], [14, 204, 14]).unwrap();
    building.revision += 1;
    mining::work(&mut state, slot, &mut v, &mut changes);
    assert_eq!(
        v.mining_progress.cell, 0,
        "a new zone changes cursor geometry"
    );
}

#[test]
fn miner_pauses_for_unloaded_neighbor_then_resumes_the_same_cell() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    let building = state
        .settlements
        .get_mut(v.settlement.unwrap())
        .unwrap()
        .buildings
        .values_mut()
        .next()
        .unwrap();
    building.bounds = ScriptWorldBounds::new([12, 192, 1], [20, 204, 14]).unwrap();
    building.work_points = vec![[13, 201, 3]];
    v.job_site = Some(BlockPos {
        x: 13,
        y: 201,
        z: 3,
    });
    state.entities.set_pos(slot, [13.5, 201.0, 3.5]);
    let mut changes = ChangeSet::default();
    for tick in 0..6 {
        state.tick = tick * 20;
        mining::work(&mut state, slot, &mut v, &mut changes);
    }
    assert_eq!(
        v.mining_progress.cell, 1,
        "unloaded space must not be treated as a permanent obstruction"
    );
    assert_eq!(changes.blocks.len(), 1);
    state
        .world
        .ensure_blocking(&rustc_hash::FxHashMap::from_iter([(
            ChunkPos { x: 1, z: 0 },
            0,
        )]));
    for x in 16..22 {
        for z in 0..16 {
            for y in 190..206 {
                state.world.set_block(
                    x,
                    y,
                    z,
                    if y <= 200 {
                        state.registry.states.stone
                    } else {
                        state.registry.states.air
                    },
                );
            }
        }
    }
    for tick in 6..9 {
        state.tick = tick * 20;
        mining::work(&mut state, slot, &mut v, &mut changes);
    }
    assert_eq!(v.mining_progress.cell, 2);
    assert_eq!(
        changes.blocks.len(),
        3,
        "loading the neighbor must release the paused excavation"
    );
}

#[test]
fn built_mine_derives_underground_work_volume_inside_settlement_sphere() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    let settlement = v.settlement.unwrap();
    let record = state.settlements.get_mut(settlement).unwrap();
    record.radius = 16;
    let mine = record.buildings.values_mut().next().unwrap();
    mine.natural_zone = false;
    mine.bounds = ScriptWorldBounds::new([1, 200, 1], [14, 204, 14]).unwrap();
    let mut changes = ChangeSet::default();
    mining::work(&mut state, slot, &mut v, &mut changes);
    let bounds = v.mining_progress.bounds.unwrap();
    assert_eq!(bounds.min, [1, 189, 1], "down 48, clamped to the sphere");
    assert_eq!(bounds.max, [14, 204, 14]);
    let record = state.settlements.get(settlement).unwrap();
    for x in [bounds.min[0], bounds.max[0]] {
        for y in [bounds.min[1], bounds.max[1]] {
            for z in [bounds.min[2], bounds.max[2]] {
                assert!(record.contains([x, y, z]));
            }
        }
    }
}

#[test]
fn natural_job_confirmation_works_with_the_real_villager_detached() {
    let (mut state, slot, v) = fixture(BuildingKind::Mine);
    let job = v.job_site.unwrap();
    state
        .world
        .set_block(job.x, job.y, job.z, state.registry.states.air);
    assert!(state.entities.row(slot).villager.is_none());
    assert!(crate::settlements::allocator::confirm_profession(
        &state,
        &state.villager_pois,
        state.entities.body(slot).id,
        v.settlement,
        job,
        VillagerProfession::Miner,
    ));
}

#[test]
fn miner_opens_a_grass_surface_for_real_dirt_and_underlying_stone() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    let grass = block(&state, "grass_block");
    for x in 1..=14 {
        for z in 1..=14 {
            state.world.set_block(x, 200, z, grass);
        }
    }
    let dirt = mining::item_id(&state, "dirt").unwrap();
    let cobble = mining::item_id(&state, "cobblestone").unwrap();
    let mut changes = ChangeSet::default();
    for step in 1..=40 {
        state.tick = step * 20;
        if let Some(target) = mining::work(&mut state, slot, &mut v, &mut changes) {
            state.entities.set_pos(
                slot,
                [
                    f64::from(target.x) + 0.5,
                    f64::from(target.y),
                    f64::from(target.z) + 0.5,
                ],
            );
        }
        if v.inventory
            .iter()
            .flatten()
            .any(|stack| stack.item_id == cobble && stack.count > 0)
        {
            break;
        }
    }
    assert!(
        v.inventory
            .iter()
            .flatten()
            .any(|stack| stack.item_id == dirt && stack.count > 0),
        "grass yields ordinary dirt, never a free grass block item"
    );
    assert!(
        v.inventory
            .iter()
            .flatten()
            .any(|stack| stack.item_id == cobble && stack.count > 0),
        "the miner descends into the real stone below the surface"
    );
    assert!(changes.blocks.iter().any(|change| change.position.y == 200));
    assert!(changes.blocks.iter().any(|change| change.position.y < 200));
}

#[test]
fn resource_workers_deliver_one_stack_without_waiting_for_all_eight_slots() {
    for kind in [BuildingKind::Mine, BuildingKind::LumberCamp] {
        let (mut state, slot, mut v) = fixture(kind);
        storage(&mut state, &v, [12, 201, 12]);
        let item = mining::item_id(
            &state,
            if kind == BuildingKind::Mine {
                "cobblestone"
            } else {
                "oak_log"
            },
        )
        .unwrap();
        v.inventory[0] = Some(ItemStack::new(item, 64));
        let mut changes = ChangeSet::default();
        let target = if kind == BuildingKind::Mine {
            mining::work(&mut state, slot, &mut v, &mut changes)
        } else {
            super::lumber::work(&mut state, slot, &mut v, &mut changes)
        };
        assert_eq!(
            target,
            Some(BlockPos {
                x: 12,
                y: 201,
                z: 12
            }),
            "{kind:?} must haul a useful batch during the work day"
        );
        assert!(
            changes.blocks.is_empty(),
            "hauling does not keep extracting"
        );
        assert_eq!(v.inventory[0].as_ref().unwrap().count, 64);
        // Declared navigation assistance: stand beside, never inside, the chest.
        state.entities.set_pos(slot, [11.5, 201.0, 12.5]);
        state.tick += 1;
        if kind == BuildingKind::Mine {
            mining::work(&mut state, slot, &mut v, &mut changes);
        } else {
            super::lumber::work(&mut state, slot, &mut v, &mut changes);
        }
        assert!(v.inventory.iter().all(Option::is_none));
        assert_eq!(
            state.block_entities[&(12, 201, 12)]
                .slots
                .iter()
                .filter(|stack| stack.item_id == item)
                .map(|stack| stack.count)
                .sum::<i32>(),
            64,
            "storage receives exactly the carried batch"
        );
    }
}

/// Seed 87122 replay: a miner whose route to its last open cell ended inside
/// the pathfinder's one-cell arrival tolerance, but just outside dig reach of
/// the next cell, never moved again (35k ticks, one dirt block mined). The real
/// brain must keep walking until the next cell is within reach.
#[test]
fn miner_stopped_diagonally_by_arrival_tolerance_still_reaches_the_next_cell() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    let settlement = v.settlement.unwrap();
    let bounds = crate::settlements::buildings::building_for_job(&state, &v)
        .unwrap()
        .bounds;
    state.world.set_block(4, 200, 3, state.registry.states.air);
    v.mining_progress = mining::Progress {
        building: (settlement.0 << 32) | 1,
        revision: 1,
        bounds: Some(bounds),
        last_open: Some([4, 200, 3]),
        lane: 0,
        level: 0,
        pass: 0,
        cell: 1,
        layer: 0,
        returning: false,
        stalled_since: 0,
    };
    // Cell (3,201,4) is diagonal to the open cell (4,200,3): arrival tolerance
    // is met, yet the next cell (5,199,3) is 3.1 blocks away.
    state.entities.set_pos(slot, [3.34, 201.0, 4.5]);
    state.entities.row_mut(slot).villager = Some(v);
    for _ in 0..400 {
        crate::tick::run_tick(&mut state, &[]);
    }
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    assert!(
        v.mining_progress.cell >= 2,
        "the miner must walk into the shaft and keep digging, cell={}",
        v.mining_progress.cell
    );
    assert_eq!(
        state.world.block_at(5, 199, 3),
        Some(state.registry.states.air)
    );
}

/// Seed 87122 replay: the fixed comb ended after ~60 cells (120 blocks) and the
/// miner idled for days beside a coal vein two blocks off its path, so the
/// settlement never got the single coal its Storage bill needs. A finished comb
/// continues one level higher until the zone is used up.
#[test]
fn miner_continues_with_a_higher_comb_after_the_first_and_reaches_ore_it_missed() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    storage(&mut state, &v, [12, 201, 12]);
    let coal = block(&state, "coal_ore");
    // Level 0 rows run at y=193; level 1 rows at y=196 (z = 6 for lane 0).
    for y in [196, 197] {
        state.world.set_block(9, y, 6, coal);
    }
    let coal_item = mining::item_id(&state, "coal").unwrap();
    let mut changes = ChangeSet::default();
    let mut coal_found = false;
    for step in 1..=4000 {
        state.tick = step * 20;
        if let Some(target) = mining::work(&mut state, slot, &mut v, &mut changes) {
            state.entities.set_pos(
                slot,
                [
                    f64::from(target.x) + 0.5,
                    f64::from(target.y),
                    f64::from(target.z) + 0.5,
                ],
            );
        }
        let stored: i32 = state.block_entities[&(12, 201, 12)]
            .slots
            .iter()
            .chain(v.inventory.iter().flatten())
            .filter(|stack| stack.item_id == coal_item)
            .map(|stack| stack.count)
            .sum();
        if stored > 0 {
            coal_found = true;
            break;
        }
    }
    assert!(
        coal_found,
        "level {} never reached the coal",
        v.mining_progress.level
    );
    assert!(v.mining_progress.level >= 1);
    let bounds = v.mining_progress.bounds.unwrap();
    assert!(changes.blocks.iter().all(|change| {
        let p = change.position;
        (0..3).all(|i| {
            let n = [p.x, p.y, p.z][i];
            n >= bounds.min[i] && n <= bounds.max[i]
        })
    }));
}

/// A zombie sitting in a cave under the quarry never reaches the miner, but
/// the village's own brain panics at any hostile within its sensor range, so a
/// miner walking up to a dig cell beside it flees forever and the mine yields
/// nothing. The miner waits clear of a hostile's reach instead and the branch
/// stays intact: digging resumes once the hostile is gone.
#[test]
fn miner_waits_clear_of_a_hostile_mob_and_resumes_when_it_is_gone() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    let zombie_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
        .unwrap();
    let id = state.spawn_mob(zombie_type, [4.5, 199.0, 3.5], 20.0);
    let zombie = state.entities.slot_of(id).unwrap();
    let run = |state: &mut TickState, v: &mut VillagerState, from: u64, changes: &mut ChangeSet| {
        for tick in from..from + 60 {
            state.tick = tick * 20;
            if let Some(target) = mining::work(state, slot, v, changes) {
                state.entities.set_pos(
                    slot,
                    [
                        f64::from(target.x) + 0.5,
                        f64::from(target.y),
                        f64::from(target.z) + 0.5,
                    ],
                );
            }
        }
    };
    let mut changes = ChangeSet::default();
    run(&mut state, &mut v, 0, &mut changes);
    assert!(
        changes.blocks.is_empty(),
        "no cell beside a hostile is dug: {:?}",
        changes
            .blocks
            .iter()
            .map(|c| c.position)
            .collect::<Vec<_>>()
    );
    let before = (v.mining_progress.lane, v.mining_progress.cell);
    state.entities.body_mut(zombie).alive = false;
    run(&mut state, &mut v, 60, &mut changes);
    assert!(
        changes.blocks.len() > 3,
        "digging resumes without the hostile"
    );
    assert_eq!(before, (0, 0), "the branch was not abandoned meanwhile");
}

/// An alternate branch starts at its own column's ground. On a slope the
/// original entrance height lies underground there, and the miner dug a
/// pocket that no route reached, sealing itself in. A column whose ground lies
/// above the zone is skipped instead.
#[test]
fn alternate_branch_never_starts_underground_in_a_column_above_the_zone() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    for x in 0..16 {
        for z in 6..16 {
            for y in 201..=203 {
                state.world.set_block(x, y, z, state.registry.states.stone);
            }
        }
    }
    let mut changes = ChangeSet::default();
    state.tick = 0;
    mining::work(&mut state, slot, &mut v, &mut changes);
    // Column z = 2 + (offset 1 + lane 5) % 12 = 8, buried under the new ground
    // that lies above the zone's top plane.
    v.mining_progress.lane = 5;
    v.mining_progress.cell = 0;
    changes = ChangeSet::default();
    for tick in 1..200 {
        state.tick = tick * 20;
        if let Some(target) = mining::work(&mut state, slot, &mut v, &mut changes) {
            state.entities.set_pos(
                slot,
                [
                    f64::from(target.x) + 0.5,
                    f64::from(target.y),
                    f64::from(target.z) + 0.5,
                ],
            );
        }
    }
    assert!(
        !changes.blocks.is_empty(),
        "the next lane with ground still opens"
    );
    let first = changes.blocks[0].position;
    assert!(
        first.z < 6,
        "no branch may start inside the buried columns, opened at {first:?}"
    );
}

/// A hillside zone leaves ground at only one entrance column. A higher level
/// must therefore climb the same lane, walking through the first level's open
/// staircase; shifting to another column (no ground there) or re-digging next
/// to the old shaft (its air counts as a cave) left the miner with no output
/// after the first level.
#[test]
fn higher_level_reuses_the_only_lane_with_ground_and_reaches_its_coal() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    // Ground rises above the zone everywhere except the entrance column.
    for x in 0..16 {
        for z in (0..16).filter(|&z| z != 3) {
            for y in 201..=210 {
                state.world.set_block(x, y, z, state.registry.states.stone);
            }
        }
    }
    storage(&mut state, &v, [12, 201, 12]);
    let coal = block(&state, "coal_ore");
    for y in [196, 197] {
        state.world.set_block(9, y, 6, coal);
    }
    let coal_item = mining::item_id(&state, "coal").unwrap();
    let mut changes = ChangeSet::default();
    let mut found = false;
    for step in 1..=6000 {
        state.tick = step * 20;
        if let Some(target) = mining::work(&mut state, slot, &mut v, &mut changes) {
            state.entities.set_pos(
                slot,
                [
                    f64::from(target.x) + 0.5,
                    f64::from(target.y),
                    f64::from(target.z) + 0.5,
                ],
            );
        }
        found = state.block_entities[&(12, 201, 12)]
            .slots
            .iter()
            .chain(v.inventory.iter().flatten())
            .any(|stack| stack.item_id == coal_item);
        if found {
            break;
        }
    }
    assert!(
        found,
        "stuck at lane {} level {} cell {}",
        v.mining_progress.lane, v.mining_progress.level, v.mining_progress.cell
    );
    assert_eq!(v.mining_progress.lane, 0);
    assert!(v.mining_progress.level >= 1);
}

/// A zombie that never leaves (stuck in a cave under the quarry) would park
/// the miner for good: after waiting, the lane closes and the miner moves on
/// to another lane, then to the next level.
#[test]
fn miner_gives_up_a_lane_after_a_hostile_outstays_its_patience() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    let zombie_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
        .unwrap();
    // Beside the far end of the staircase, out of reach of its first cells.
    state.spawn_mob(zombie_type, [13.5, 193.5, 3.5], 20.0);
    let mut changes = ChangeSet::default();
    let mut first_blocked = None;
    for step in 0..600u64 {
        state.tick = step * 20;
        let before = (v.mining_progress.lane, v.mining_progress.level);
        if let Some(target) = mining::work(&mut state, slot, &mut v, &mut changes) {
            state.entities.set_pos(
                slot,
                [
                    f64::from(target.x) + 0.5,
                    f64::from(target.y),
                    f64::from(target.z) + 0.5,
                ],
            );
        }
        if v.mining_progress.stalled_since != 0 && first_blocked.is_none() {
            first_blocked = Some(state.tick);
        }
        if (v.mining_progress.lane, v.mining_progress.level) != before {
            let waited = state.tick - first_blocked.expect("blocked before giving up");
            assert!(waited >= 2_400, "gave up after only {waited} ticks");
            return;
        }
    }
    panic!("the miner stayed parked behind a hostile that never leaves");
}

/// Routing to the solid dig cell lets the pathfinder's closest-point fallback
/// lead the miner into whatever open pocket lies nearest to it (a lower level's
/// row directly beneath), where it stands for good. A miner far from the open
/// end of its tunnel walks to that known-reachable end first.
#[test]
fn miner_far_from_its_tunnel_walks_to_the_open_end_not_the_solid_cell() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    state.tick = 0;
    mining::work(&mut state, slot, &mut v, &mut ChangeSet::default());
    let bounds = v.mining_progress.bounds.unwrap();
    assert_eq!(bounds.min[1], 192);
    // Eight cells in: the staircase has reached (8, 196, 3).
    v.mining_progress.cell = 5;
    v.mining_progress.last_open = Some([8, 196, 3]);
    state.entities.set_pos(slot, [3.5, 201.0, 3.5]);
    state.tick = 20;
    let target = mining::work(&mut state, slot, &mut v, &mut ChangeSet::default());
    assert_eq!(target, Some(BlockPos { x: 8, y: 196, z: 3 }));
}

/// A higher level reuses the lower levels' open staircase, so its corridor
/// must not be cut at the same column above the stairs that keep descending
/// (it would hang over their air): every cell the miner opens has to stay
/// walkable from the entrance.
#[test]
fn higher_level_cells_stay_walkable_from_the_entrance() {
    use crate::settlements::allocator::{PoiCache, RouteCtx, find_path_exact_ctx};
    use crate::tick_budget::Consumer;
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    storage(&mut state, &v, [12, 201, 12]);
    let mut changes = ChangeSet::default();
    let mut last = None;
    for step in 1..=6000 {
        state.tick = step * 20;
        if let Some(target) = mining::work(&mut state, slot, &mut v, &mut changes) {
            state.entities.set_pos(
                slot,
                [
                    f64::from(target.x) + 0.5,
                    f64::from(target.y),
                    f64::from(target.z) + 0.5,
                ],
            );
        }
        if v.mining_progress.level >= 1 && v.mining_progress.cell >= 25 {
            last = v.mining_progress.last_open;
            break;
        }
    }
    let last = last.expect("the miner reaches level 1");
    state.tick_budget.reset(state.tick);
    let mut cache = PoiCache::default();
    let mut path = Vec::new();
    let mut ctx = RouteCtx::tick(&mut state, Consumer::Brain);
    let found = find_path_exact_ctx(
        &mut ctx,
        &mut cache,
        [2.5, 201.0, 3.5],
        BlockPos {
            x: last[0],
            y: last[1],
            z: last[2],
        },
        0.3,
        1.95,
        &mut path,
    );
    assert!(found, "no walkable route to level 1 cell {last:?}");
}

/// After an upgrade (or beside another miner) the lane next to an already dug
/// tunnel is not a cave: that air belongs to a tunnel of this mine, which opens
/// onto its entrance. Treating every open neighbour as a cave burned every lane
/// of the L2 mine at once and the miner stood idle for days.
#[test]
fn miner_digs_a_lane_beside_an_earlier_tunnel_of_its_own_mine() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    let air = state.registry.states.air;
    // An earlier tunnel beside lane 0 (z = 3): a descending trench at z = 4.
    for x in 4..=12 {
        for y in [200, 199] {
            state.world.set_block(x, y, 4, air);
        }
    }
    let mut changes = ChangeSet::default();
    for tick in 0..120 {
        state.tick = tick * 20;
        if let Some(target) = mining::work(&mut state, slot, &mut v, &mut changes) {
            state.entities.set_pos(
                slot,
                [
                    f64::from(target.x) + 0.5,
                    f64::from(target.y),
                    f64::from(target.z) + 0.5,
                ],
            );
        }
    }
    assert!(
        changes.blocks.iter().any(|change| change.position.z == 3),
        "the lane beside the old tunnel is never dug (lane {})",
        v.mining_progress.lane
    );
}

/// Upgrading a built mine re-certifies it: a new revision, a larger footprint
/// and a moved entrance. The miner has to keep digging at L2 beside the L1
/// tunnels instead of stalling with progress from the old layout.
#[test]
fn miner_keeps_digging_after_the_built_mine_is_upgraded_and_its_entrance_moves() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    storage(&mut state, &v, [13, 201, 13]);
    let settlement = v.settlement.unwrap();
    let id = state
        .settlements
        .get(settlement)
        .unwrap()
        .buildings
        .values()
        .find(|b| b.kind == BuildingKind::Mine)
        .unwrap()
        .id;
    {
        let b = state
            .settlements
            .get_mut(settlement)
            .unwrap()
            .buildings
            .get_mut(&id)
            .unwrap();
        b.natural_zone = false;
        b.bounds = ScriptWorldBounds::new([1, 200, 1], [10, 204, 10]).unwrap();
    }
    let mut changes = ChangeSet::default();
    let mut work = |state: &mut TickState, v: &mut VillagerState, from: u64, count: u64| {
        let before = changes.blocks.len();
        for step in from..from + count {
            state.tick = step * 20;
            if let Some(target) = mining::work(state, slot, v, &mut changes) {
                state.entities.set_pos(
                    slot,
                    [
                        f64::from(target.x) + 0.5,
                        f64::from(target.y),
                        f64::from(target.z) + 0.5,
                    ],
                );
            }
        }
        changes.blocks.len() - before
    };
    assert!(work(&mut state, &mut v, 0, 300) > 10, "L1 digs");
    // The upgrade: level 2, new revision, bigger footprint, entrance moved.
    {
        let b = state
            .settlements
            .get_mut(settlement)
            .unwrap()
            .buildings
            .get_mut(&id)
            .unwrap();
        b.level = 2;
        b.revision += 1;
        b.bounds = ScriptWorldBounds::new([0, 200, 0], [13, 205, 12]).unwrap();
        b.work_points = vec![[5, 201, 5]];
    }
    let job = BlockPos { x: 5, y: 201, z: 5 };
    let table = block(&state, "crafting_table");
    state.world.set_block(job.x, job.y, job.z, table);
    v.job_site = Some(job);
    let dug = work(&mut state, &mut v, 300, 1500);
    assert!(
        dug > 10,
        "the upgraded mine digs nothing (lane {} level {} cell {})",
        v.mining_progress.lane,
        v.mining_progress.level,
        v.mining_progress.cell
    );
}

/// A dig cell the miner can never reach (here: the entity is never moved, as
/// if a pit or pocket cut the route) must not park it for good: after the
/// patience the lane closes and the next lane is tried.
#[test]
fn miner_abandons_a_lane_whose_cell_it_cannot_reach() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    let mut changes = ChangeSet::default();
    state.tick = 0;
    mining::work(&mut state, slot, &mut v, &mut changes);
    state.entities.set_pos(slot, [14.5, 201.0, 14.5]);
    let lane = v.mining_progress.lane;
    for step in 1..400u64 {
        state.tick = step * 20;
        mining::work(&mut state, slot, &mut v, &mut changes);
        if v.mining_progress.lane != lane {
            assert!(state.tick >= 2_400, "gave up after {} ticks", state.tick);
            return;
        }
    }
    panic!("the miner stayed on an unreachable lane");
}

/// Carry that never grows a full stack of one item (dirt from the grass
/// surface, a little of every stone) must still reach the chest: with the day
/// clock stopped nothing else ever sends the miner home.
#[test]
fn miner_delivers_a_mixed_load_without_a_full_stack() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    storage(&mut state, &v, [12, 201, 12]);
    let dirt = mining::item_id(&state, "dirt").unwrap();
    let cobble = mining::item_id(&state, "cobblestone").unwrap();
    v.inventory[0] = Some(ItemStack::new(dirt, 40));
    v.inventory[1] = Some(ItemStack::new(cobble, 30));
    state.tick = 100;
    let target = mining::work(&mut state, slot, &mut v, &mut ChangeSet::default());
    assert_eq!(
        target,
        Some(BlockPos {
            x: 12,
            y: 201,
            z: 12
        })
    );
    assert!(v.mining_progress.returning);
}

/// The comb covers only as deep as one staircase reaches (one block per
/// column), so a zone deeper than it is wide kept most of its stone. When every
/// level of a flight is done the next flight starts where the last one ended
/// and goes on down to the bottom of the zone.
#[test]
fn miner_starts_a_deeper_flight_after_finishing_the_first() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    state
        .settlements
        .get_mut(v.settlement.unwrap())
        .unwrap()
        .radius = 64;
    for x in 0..16 {
        for z in 0..16 {
            for y in 185..=200 {
                state.world.set_block(x, y, z, state.registry.states.stone);
            }
        }
    }
    let settlement = v.settlement.unwrap();
    let id = state
        .settlements
        .get(settlement)
        .unwrap()
        .buildings
        .values()
        .find(|b| b.kind == BuildingKind::Mine)
        .unwrap()
        .id;
    state
        .settlements
        .get_mut(settlement)
        .unwrap()
        .buildings
        .get_mut(&id)
        .unwrap()
        .bounds = ScriptWorldBounds::new([1, 185, 1], [14, 204, 14]).unwrap();
    storage(&mut state, &v, [12, 201, 12]);
    let mut changes = ChangeSet::default();
    for step in 1..=40_000u64 {
        state.tick = step * 20;
        if let Some(target) = mining::work(&mut state, slot, &mut v, &mut changes) {
            state.entities.set_pos(
                slot,
                [
                    f64::from(target.x) + 0.5,
                    f64::from(target.y),
                    f64::from(target.z) + 0.5,
                ],
            );
        }
        if changes.blocks.iter().any(|c| c.position.y <= 187) {
            assert!(v.mining_progress.pass >= 1);
            let last = v.mining_progress.last_open.expect("an open cell");
            use crate::settlements::allocator::{PoiCache, RouteCtx, find_path_exact_ctx};
            state.tick_budget.reset(state.tick);
            let mut path = Vec::new();
            let mut ctx = RouteCtx::tick(&mut state, crate::tick_budget::Consumer::Brain);
            let found = find_path_exact_ctx(
                &mut ctx,
                &mut PoiCache::default(),
                [2.5, 201.0, 3.5],
                BlockPos {
                    x: last[0],
                    y: last[1],
                    z: last[2],
                },
                0.3,
                1.95,
                &mut path,
            );
            assert!(
                found,
                "the deeper flight is cut off from the entrance at {last:?}"
            );
            return;
        }
    }
    panic!(
        "never went below the first flight (pass {} level {} lane {})",
        v.mining_progress.pass, v.mining_progress.level, v.mining_progress.lane
    );
}

/// A natural zone's certified depth is finite, but the mine must keep
/// producing: its quarry goes on below the zone, flight after flight, as far
/// as the settlement sphere allows, and every flight stays reachable.
#[test]
fn natural_mine_keeps_quarrying_below_its_certified_floor() {
    use crate::settlements::allocator::{PoiCache, RouteCtx, find_path_exact_ctx};
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    state
        .settlements
        .get_mut(v.settlement.unwrap())
        .unwrap()
        .radius = 64;
    for x in 0..16 {
        for z in 0..16 {
            for y in 150..=200 {
                state.world.set_block(x, y, z, state.registry.states.stone);
            }
        }
    }
    storage(&mut state, &v, [12, 201, 12]);
    let mut changes = ChangeSet::default();
    let floor = 192;
    for step in 1..=400_000u64 {
        state.tick = step * 20;
        if let Some(target) = mining::work(&mut state, slot, &mut v, &mut changes) {
            state.entities.set_pos(
                slot,
                [
                    f64::from(target.x) + 0.5,
                    f64::from(target.y),
                    f64::from(target.z) + 0.5,
                ],
            );
        }
        if v.mining_progress.pass >= 3 && v.mining_progress.cell >= 5 {
            assert!(
                changes.blocks.iter().all(|c| {
                    (1..=14).contains(&c.position.x) && (1..=14).contains(&c.position.z)
                }),
                "the quarry stays inside the zone's footprint"
            );
            let lowest = changes.blocks.iter().map(|c| c.position.y).min().unwrap();
            assert!(lowest < floor - 6, "lowest block {lowest}");
            let last = v.mining_progress.last_open.expect("an open cell");
            state.tick_budget.reset(state.tick);
            let mut path = Vec::new();
            let mut ctx = RouteCtx::tick(&mut state, crate::tick_budget::Consumer::Brain);
            let found = find_path_exact_ctx(
                &mut ctx,
                &mut PoiCache::default(),
                [2.5, 201.0, 3.5],
                BlockPos {
                    x: last[0],
                    y: last[1],
                    z: last[2],
                },
                0.3,
                1.95,
                &mut path,
            );
            assert!(
                found,
                "flight {} is cut off at {last:?}",
                v.mining_progress.pass
            );
            return;
        }
    }
    panic!(
        "never reached a fourth flight (pass {} level {} lane {})",
        v.mining_progress.pass, v.mining_progress.level, v.mining_progress.lane
    );
}

/// A mine with nothing left to dig must not sit on what it carries: the stock
/// the miner mined has to reach the chest.
#[test]
fn exhausted_miner_delivers_what_it_carries() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    storage(&mut state, &v, [12, 201, 12]);
    let cobble = mining::item_id(&state, "cobblestone").unwrap();
    state.tick = 100;
    mining::work(&mut state, slot, &mut v, &mut ChangeSet::default());
    v.mining_progress.pass = 200;
    v.inventory[0] = Some(ItemStack::new(cobble, 5));
    state.tick = 200;
    let target = mining::work(&mut state, slot, &mut v, &mut ChangeSet::default());
    assert_eq!(
        target,
        Some(BlockPos {
            x: 12,
            y: 201,
            z: 12
        })
    );
    state.entities.set_pos(slot, [12.5, 201.0, 11.5]);
    mining::work(&mut state, slot, &mut v, &mut ChangeSet::default());
    assert!(v.inventory.iter().all(Option::is_none));
}
