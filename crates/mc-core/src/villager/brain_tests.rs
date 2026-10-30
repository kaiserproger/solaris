use super::*;
use crate::boundary::EntityId;
use crate::settlements::allocator::{
    PICKUP_REACH, POI_REFRESH_CHUNKS_PER_TICK, Poi, PoiCache, PoiKind, PoiValidation,
    bed_has_baby_space, distance, find_path, find_path_exact, standing, validate_pois,
};
use mc_physics::BlockSampler;
use rustc_hash::FxHashMap;
use std::sync::Arc;

#[path = "door_tests.rs"]
mod door_tests;

#[path = "breeding_tests.rs"]
mod breeding_tests;

#[path = "job_memory_tests.rs"]
mod job_memory_tests;

#[path = "route_continuation_tests.rs"]
mod route_continuation_tests;

fn test_state() -> TickState {
    let registry = Arc::new(CoreRegistry::load(None).expect("registry"));
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(
        mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks)),
    );
    let mut world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [8, 201, 8],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let want = rustc_hash::FxHashMap::from_iter([(crate::boundary::ChunkPos { x: 0, z: 0 }, 0)]);
    world.ensure_blocking(&want);
    for x in 0..16 {
        for z in 0..16 {
            world.set_block(x, 200, z, registry.states.stone);
            for y in 201..206 {
                world.set_block(x, y, z, registry.states.air);
            }
        }
    }
    world.fold_overlay();
    let mut state = TickState::new(registry, world, 2, 512);
    state.tickets.insert(
        "villager-fixture".into(),
        (crate::boundary::ChunkPos { x: 0, z: 0 }, 0),
    );
    state.refresh_active_chunks(0);
    state
}
fn block(state: &TickState, name: &str, props: &[(&str, &str)]) -> u32 {
    let id = mc_data::Identifier::parse(name).unwrap();
    let default = state.registry.blocks.block(&id).unwrap().default;
    let mut full = state
        .registry
        .blocks
        .by_id(default)
        .unwrap()
        .properties
        .clone();
    for (key, value) in props {
        if let Some(entry) = full.iter_mut().find(|(k, _)| k == key) {
            entry.1 = (*value).to_owned();
        }
    }
    state
        .registry
        .blocks
        .by_name_and_props(&id, &full)
        .unwrap()
        .0
}
fn item(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse(name).unwrap())
        .unwrap()
}
fn villager(state: &mut TickState, pos: [f64; 3]) -> EntitySlot {
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let id = state.spawn_mob(type_id, pos, 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).villager = Some(Box::new(VillagerState::default()));
    slot
}
fn bed(state: &mut TickState, x: i32, z: i32) -> BlockPos {
    let foot = block(
        state,
        "minecraft:red_bed",
        &[("part", "foot"), ("facing", "east")],
    );
    let head = block(
        state,
        "minecraft:red_bed",
        &[("part", "head"), ("facing", "east")],
    );
    state.world.set_block(x - 1, 201, z, foot);
    state.world.set_block(x, 201, z, head);
    BlockPos { x, y: 201, z }
}
// Direct brain fixtures stand in for the global root and unrun owner passes.
fn begin_brain_tick(state: &mut TickState) {
    state.tick_budget.reset(state.tick);
    for dimension in crate::dimensions::Dimension::ALL {
        for consumer in [Consumer::Allocator, Consumer::Brain] {
            if dimension != state.dimension || consumer != Consumer::Brain {
                state.tick_budget.finish(dimension, consumer);
            }
        }
    }
}
fn set_node_allowance(state: &mut TickState, nodes: usize) {
    // An explicit independent test fixture, not a runtime same-tick refill.
    state.tick_budget = crate::tick_budget::Budget::default();
    state.tick_budget.reset(state.tick);
    for dimension in crate::dimensions::Dimension::ALL {
        for consumer in [Consumer::Allocator, Consumer::Brain] {
            state.tick_budget.finish(dimension, consumer);
        }
    }
    let available = state.tick_budget.remaining(Kind::PathNode);
    assert!(state.tick_budget.try_charge(
        state.dimension,
        Consumer::Brain,
        Kind::PathNode,
        available - nodes,
    ));
}
fn step(state: &mut TickState) -> ChangeSet {
    state.tick_budget.reset(state.tick);
    let mut changes = ChangeSet::default();
    crate::settlements::allocator::tick(state, &mut changes);
    begin_brain_tick(state);
    tick(state, &mut changes);
    state.tick += 1;
    changes
}

#[test]
fn resident_inactive_brain_freezes_age_and_poi_acquisition_until_activation() {
    let mut state = test_state();
    bed(&mut state, 8, 8);
    state.world.fold_overlay();
    let slot = villager(&mut state, [9.5, 201.0, 8.5]);
    state.entities.row_mut(slot).villager.as_mut().unwrap().age = 10;
    state.tickets.clear();
    state.refresh_active_chunks(1);
    assert!(state.world.has_chunk(0, 0));
    assert!(!state.world.is_active(0, 0));
    step(&mut state);
    let villager = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(villager.age, 10);
    assert!(villager.home.is_none());
    assert!(villager.job_site.is_none());

    state.tickets.insert(
        "villager-fixture".into(),
        (crate::boundary::ChunkPos { x: 0, z: 0 }, 0),
    );
    state.refresh_active_chunks(state.tick + 1);
    step(&mut state);
    assert_eq!(state.entities.row(slot).villager.as_ref().unwrap().age, 9);
}
fn food_count(state: &TickState, slot: EntitySlot, id: u32) -> i32 {
    state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .unwrap()
        .inventory
        .iter()
        .flatten()
        .filter(|s| s.item_id == id)
        .map(|s| s.count)
        .sum()
}

#[test]
fn exact_adult_and_baby_schedule_boundaries() {
    for (time, adult, baby) in [
        (0, Activity::Rest, Activity::Rest),
        (10, Activity::Idle, Activity::Idle),
        (2000, Activity::Work, Activity::Idle),
        (3000, Activity::Work, Activity::Play),
        (6000, Activity::Work, Activity::Idle),
        (9000, Activity::Meet, Activity::Idle),
        (10000, Activity::Meet, Activity::Play),
        (11000, Activity::Idle, Activity::Play),
        (12000, Activity::Rest, Activity::Rest),
    ] {
        assert_eq!(schedule(0, Profession::None, time), adult);
        assert_eq!(schedule(-1, Profession::None, time), baby);
        assert_eq!(schedule(0, Profession::None, time + 24000), adult);
    }
}
#[test]
fn workstation_claim_is_exclusive_and_acquires_its_profession() {
    let mut state = test_state();
    state.day_time = 3000;
    let workstation = BlockPos { x: 8, y: 201, z: 8 };
    let id = block(&state, "minecraft:composter", &[]);
    state.world.set_block(8, 201, 8, id);
    state.world.fold_overlay();
    let first = villager(&mut state, [7.5, 201.0, 8.5]);
    let second = villager(&mut state, [9.5, 201.0, 8.5]);
    for _ in 0..40 {
        step(&mut state);
    }
    let claims = [first, second].map(|s| state.entities.row(s).villager.as_ref().unwrap().job_site);
    assert_eq!(
        claims.iter().filter(|&&p| p == Some(workstation)).count(),
        1
    );
    let claimed = if claims[0].is_some() { first } else { second };
    assert_eq!(
        state
            .entities
            .row(claimed)
            .villager
            .as_ref()
            .unwrap()
            .data
            .profession,
        Profession::Farmer
    );
}

fn control_state() -> TickState {
    let mut state = test_state();
    state.tickets.insert(
        "village-control".into(),
        (crate::boundary::ChunkPos { x: 0, z: 0 }, 0),
    );
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    state
        .gamerules
        .insert("advance_time".into(), "false".into());
    state
}

#[test]
fn village_control_walks_to_composter_and_publishes_farmer_metadata() {
    let mut state = control_state();
    state.day_time = 2000;
    let workstation = BlockPos {
        x: 10,
        y: 201,
        z: 10,
    };
    let composter = block(&state, "minecraft:composter", &[]);
    state
        .world
        .set_block(workstation.x, workstation.y, workstation.z, composter);
    state.world.fold_overlay();
    let start = [2.5, 201.0, 2.5];
    let slot = villager(&mut state, start);
    let id = state.entities.body(slot).id;
    let mut farmer_update = false;
    for _ in 0..200 {
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        let view = output
            .snapshot
            .entities
            .iter()
            .find(|view| view.id == id)
            .unwrap();
        farmer_update |= output.changes.entities_appearance_changed.contains(&id)
            && view.appearance.extra.contains(&(
                19,
                crate::boundary::EntityDataValue::VillagerData {
                    kind: super::super::VillagerKind::Plains as i32,
                    profession: Profession::Farmer.profession_id(),
                    level: 1,
                },
            ));
    }
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(v.job_site, Some(workstation));
    assert_eq!(v.data.profession, Profession::Farmer);
    assert!(
        farmer_update,
        "farmer VillagerData must be in the published appearance delta"
    );
    assert_ne!(
        state.entities.pos(slot),
        start,
        "claiming requires reaching the job site"
    );
}

#[test]
fn village_control_fixture_layout_acquires_farmer_without_stalling_at_path_endpoint() {
    let mut state = control_state();
    state.day_time = 2000;
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(4, 201, 4, composter);
    bed(&mut state, 1, 0);
    let bell = block(&state, "minecraft:bell", &[]);
    state.world.set_block(8, 201, 8, bell);
    state.world.fold_overlay();
    let slot = villager(&mut state, [2.5, 201.0, 2.5]);
    for _ in 0..100 {
        crate::tick::run_tick(&mut state, &[]);
    }
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(v.job_site, Some(BlockPos { x: 4, y: 201, z: 4 }));
    assert_eq!(
        v.data.profession,
        Profession::Farmer,
        "position={:?}",
        state.entities.pos(slot)
    );
}

#[test]
fn village_control_reachable_adjacent_endpoint_acquires_farmer() {
    let mut state = control_state();
    state.day_time = 2000;
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(4, 201, 4, composter);
    state.world.fold_overlay();
    // Translated final position from the failing public control's first villager:
    // [19.322834405812458, 64.0, -44.942913985468834], job [20, 64, -44].
    let slot = villager(&mut state, [3.322834405812458, 201.0, 3.057086014531166]);
    for _ in 0..40 {
        crate::tick::run_tick(&mut state, &[]);
    }
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(v.job_site, Some(BlockPos { x: 4, y: 201, z: 4 }));
    assert_eq!(v.data.profession, Profession::Farmer);
}

#[test]
fn village_control_workers_keep_publishing_movement_after_arrival() {
    let mut state = control_state();
    state.day_time = 2000;
    let workstation = BlockPos { x: 4, y: 201, z: 4 };
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(4, 201, 4, composter);
    state.world.fold_overlay();
    // Same relative spawn/workstation layout as the public village control.
    let slot = villager(&mut state, [2.5, 201.0, 2.5]);
    let id = state.entities.body(slot).id;
    let mut previous = state.entities.pos(slot);
    let mut late_moves = 0;
    for tick in 0..200 {
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        let view = output
            .snapshot
            .entities
            .iter()
            .find(|view| view.id == id)
            .unwrap();
        if tick >= 100 && distance(previous, view.position) > 1e-6 {
            late_moves += 1;
        }
        previous = view.position;
    }
    assert!(
        late_moves > 0,
        "workers must not stand motionless for the whole work phase"
    );
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(v.job_site, Some(workstation));
    assert_eq!(v.data.profession, Profession::Farmer);
}

#[test]
fn village_motion_survives_unrelated_world_revision_churn() {
    let mut state = control_state();
    state.day_time = 2000;
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(4, 201, 4, composter);
    state.world.fold_overlay();
    let slot = villager(&mut state, [2.5, 201.0, 2.5]);
    let id = state.entities.body(slot).id;
    for _ in 0..200 {
        crate::tick::run_tick(&mut state, &[]);
    }
    let mut previous = state.entities.pos(slot);
    let mut moves = [0; 3];
    let mut revision_changes = 0;
    let mut revision = state.world.revision_now();
    for (window, ticks) in [400, 200, 400].into_iter().enumerate() {
        for tick in 0..ticks {
            if window < 2 {
                // Chunk admission, lighting and edits all advance this global
                // revision. This write is far above the unchanged walking cells.
                let next = if tick % 2 == 0 {
                    state.registry.states.stone
                } else {
                    state.registry.states.air
                };
                state.world.set_block(15, 300, 15, next);
                state.world.fold_overlay();
            }
            let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
            let view = output
                .snapshot
                .entities
                .iter()
                .find(|view| view.id == id)
                .unwrap();
            if distance(previous, view.position) > 1e-6 {
                moves[window] += 1;
            }
            previous = view.position;
            revision_changes += usize::from(revision != state.world.revision_now());
            revision = state.world.revision_now();
        }
    }
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    eprintln!(
        "village motion: moves={moves:?}, revision_changes={revision_changes}, position={previous:?}, route_chunks={}, world_revision={revision}, repath_at={}, tick={}",
        v.navigation_chunks.len(),
        v.navigation_repath_at,
        state.tick
    );
    assert!(revision_changes >= 600);
    assert!(moves[2] > 0, "quiet recovery must exercise navigation");
    assert!(
        moves[0] > 0 && moves[1] > 0,
        "revision churn must not freeze work strolls: {moves:?}"
    );
}

#[test]
fn village_control_idle_wandering_reaches_published_entity_views() {
    let mut state = control_state();
    state.day_time = 1000;
    let start = [8.5, 201.0, 8.5];
    let slot = villager(&mut state, start);
    let id = state.entities.body(slot).id;
    let mut previous = start;
    let mut moves = 0;
    for _ in 0..100 {
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        let view = output
            .snapshot
            .entities
            .iter()
            .find(|view| view.id == id)
            .unwrap();
        if distance(previous, view.position) > 1e-6 {
            moves += 1;
        }
        previous = view.position;
    }
    assert!(
        moves > 0,
        "idle navigation must reach the edge's snapshot delta source"
    );
    assert_ne!(previous, start);
}
#[test]
fn lost_workstation_releases_untraded_but_retains_traded_profession() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let home = BlockPos { x: 9, y: 201, z: 8 };
    let mut v = VillagerState::default();
    v.data.profession = Profession::Farmer;
    v.job_site = Some(home);
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    validate_pois(&state, &cache, state.entities.body(slot).id, &mut v);
    assert_eq!(v.job_site, None);
    assert_eq!(v.data.profession, Profession::None);
    v.data.profession = Profession::Farmer;
    v.job_site = Some(home);
    v.xp = 1;
    validate_pois(&state, &cache, state.entities.body(slot).id, &mut v);
    assert_eq!(v.job_site, None);
    assert_eq!(v.data.profession, Profession::Farmer);
}
#[test]
fn nitwits_and_babies_never_acquire_a_profession() {
    let mut state = test_state();
    state.day_time = 3000;
    let id = block(&state, "minecraft:composter", &[]);
    state.world.set_block(8, 201, 8, id);
    state.world.fold_overlay();
    let nitwit = villager(&mut state, [7.5, 201.0, 8.5]);
    let baby = villager(&mut state, [9.5, 201.0, 8.5]);
    state
        .entities
        .row_mut(nitwit)
        .villager
        .as_mut()
        .unwrap()
        .data
        .profession = Profession::Nitwit;
    state.entities.row_mut(baby).villager.as_mut().unwrap().age = -24000;
    for _ in 0..40 {
        step(&mut state);
    }
    assert_eq!(
        state
            .entities
            .row(nitwit)
            .villager
            .as_ref()
            .unwrap()
            .data
            .profession,
        Profession::Nitwit
    );
    assert_eq!(
        state
            .entities
            .row(baby)
            .villager
            .as_ref()
            .unwrap()
            .data
            .profession,
        Profession::None
    );
    assert!(
        state
            .entities
            .row(baby)
            .villager
            .as_ref()
            .unwrap()
            .job_site
            .is_none()
    );
}
#[test]
fn pickup_sensor_filters_distant_items_and_preserves_slot_order_at_chunk_edges() {
    let mut state = test_state();
    let bread = item(&state, "minecraft:bread");
    let positions = [
        [16.1, 201.0, 16.1],
        [-16.1, 201.0, -16.1],
        [15.9, 201.0, 15.9],
        [-0.1, 201.0, -0.1],
        [0.0, 201.0, 0.0],
        [-16.0, 201.0, -16.0],
        [1.425, 201.0, 0.0],
        [0.0, 201.0, -1.425],
    ];
    for pos in positions {
        villager(&mut state, pos);
    }
    let slots: Vec<_> = positions
        .into_iter()
        .map(|pos| {
            let id = crate::tick::spawn_item_entity(&mut state, pos, ItemStack::new(bread, 1));
            state.entities.slot_of(id).unwrap()
        })
        .collect();
    let mut remote = Vec::new();
    for _ in 0..1457 {
        let id = crate::tick::spawn_item_entity(
            &mut state,
            [1000.0, 201.0, 1000.0],
            ItemStack::new(bread, 1),
        );
        remote.push(state.entities.slot_of(id).unwrap());
    }
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    for x in [-16.1, -16.0, -0.1, 0.0, 15.9, 16.1] {
        for z in [-16.1, -16.0, -0.1, 0.0, 15.9, 16.1] {
            let nearby = cache.items_near([x, 201.0, z]);
            assert!(
                nearby.len() <= slots.len(),
                "distant drops entered the sensor"
            );
            assert!(nearby.windows(2).all(|pair| pair[0].0 < pair[1].0));
            for &slot in &slots {
                let pos = state.entities.pos(slot);
                if (pos[0] - x).abs() <= PICKUP_REACH && (pos[2] - z).abs() <= PICKUP_REACH {
                    assert!(nearby.contains(&slot), "lost item at a chunk boundary");
                }
            }
        }
    }
    // Rebuild from current positions, discarding departed/empty buckets.
    state.entities.set_pos(remote[0], [-0.1, 201.0, 0.0]);
    for &slot in &remote[1..] {
        state.entities.body_mut(slot).alive = false;
    }
    cache.refresh(&state);
    assert!(cache.items_near([0.0, 201.0, 0.0]).contains(&remote[0]));
    assert!(!cache.items.contains_key(&(62, 62)));
}

#[test]
fn pickup_has_one_custodian_across_two_villagers() {
    let mut state = test_state();
    state.day_time = 1000;
    let first = villager(&mut state, [8.0, 201.0, 8.0]);
    let second = villager(&mut state, [8.5, 201.0, 8.0]);
    let bread = item(&state, "minecraft:bread");
    let drop =
        crate::tick::spawn_item_entity(&mut state, [8.0, 201.0, 8.0], ItemStack::new(bread, 3));
    let slot = state.entities.slot_of(drop).unwrap();
    state.entities.body_mut(slot).pickup_delay = 0;
    let changes = step(&mut state);
    assert_eq!(
        food_count(&state, first, bread) + food_count(&state, second, bread),
        3
    );
    assert!(state.entities.slot_of_any(drop).is_none());
    assert_eq!(
        changes
            .entity_events
            .iter()
            .filter(|e| matches!(e,EntityEvent::Pickup {item,..} if *item==drop))
            .count(),
        1
    );
    assert_eq!(changes.entities_removed, vec![drop]);
    step(&mut state);
    assert_eq!(
        food_count(&state, first, bread) + food_count(&state, second, bread),
        3
    );
}
#[test]
fn partial_pickup_preserves_exact_remainder_without_duplication() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.0, 201.0, 8.0]);
    let bread = item(&state, "minecraft:bread");
    let potato = item(&state, "minecraft:potato");
    let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
    v.inventory = std::array::from_fn(|_| Some(ItemStack::new(potato, 64)));
    v.inventory[0] = Some(ItemStack::new(bread, 63));
    let drop =
        crate::tick::spawn_item_entity(&mut state, [8.0, 201.0, 8.0], ItemStack::new(bread, 3));
    let item_slot = state.entities.slot_of(drop).unwrap();
    state.entities.body_mut(item_slot).pickup_delay = 0;
    let mut v = state.entities.row_mut(slot).villager.take().unwrap();
    let mut changes = ChangeSet::default();
    let mut food = FoodTable::default();
    food.refresh(&state.registry.items);
    pickup(&mut state, &food, &[item_slot], slot, &mut v, &mut changes);
    state.entities.row_mut(slot).villager = Some(v);
    assert_eq!(food_count(&state, slot, bread), 64);
    assert_eq!(
        state.entities.row(item_slot).item.as_ref().unwrap().count,
        2
    );
    assert!(state.entities.body(item_slot).alive);
}
#[test]
fn courtship_birth_claims_a_third_bed_and_charges_each_parent() {
    let mut state = test_state();
    state.tick = 300;
    state.day_time = 1000;
    let first = villager(&mut state, [8.0, 201.0, 8.0]);
    let second = villager(&mut state, [8.5, 201.0, 8.0]);
    let a_home = bed(&mut state, 4, 4);
    let b_home = bed(&mut state, 8, 4);
    let child_home = bed(&mut state, 12, 4);
    state.world.fold_overlay();
    let a_id = state.entities.body(first).id;
    let b_id = state.entities.body(second).id;
    let settlement = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "birthtown".into(),
            "owner".into(),
            [8, 201, 8],
            48,
        )
        .unwrap();
    crate::settlements::bind(&mut state, settlement, a_id).unwrap();
    crate::settlements::bind(&mut state, settlement, b_id).unwrap();
    let bread = item(&state, "minecraft:bread");
    for (slot, partner, home) in [(first, b_id, a_home), (second, a_id, b_home)] {
        let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
        v.home = Some(home);
        v.inventory[0] = Some(ItemStack::new(bread, 3));
        v.partner = Some(partner);
        v.courtship_until = Some(300);
        v.courtship_started_tick = Some(0);
    }
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    // The movement phase may have exhausted the shared node allowance.
    state
        .tick_budget
        .exhaust_for_test(crate::tick_budget::Kind::PathNode);
    let mut changes = ChangeSet::default();
    courtship(&mut state, &mut cache, &mut changes);
    assert!(
        changes.entities_spawned.is_empty(),
        "courtship shares the exhausted pool"
    );
    for parent in [first, second] {
        assert_eq!(food_count(&state, parent, bread), 3);
        assert!(
            state
                .entities
                .row(parent)
                .villager
                .as_ref()
                .unwrap()
                .partner
                .is_some()
        );
    }
    state.tick += 1;
    begin_brain_tick(&mut state);
    courtship(&mut state, &mut cache, &mut changes);
    assert_eq!(changes.entities_spawned.len(), 1);
    for parent in [first, second] {
        let v = state.entities.row(parent).villager.as_ref().unwrap();
        assert_eq!(v.age, 6000);
        assert_eq!(v.food_level, 0);
        assert_eq!(food_count(&state, parent, bread), 0);
        assert!(v.partner.is_none());
    }
    let baby = state.entities.slot_of(changes.entities_spawned[0]).unwrap();
    let v = state.entities.row(baby).villager.as_ref().unwrap();
    assert_eq!(v.age, -24000);
    assert_eq!(v.home, Some(child_home));
    assert_eq!(
        v.settlement,
        Some(settlement),
        "native births inherit membership"
    );
    let family = v
        .household
        .as_ref()
        .expect("native birth forms the parents' household");
    assert_eq!(
        state
            .entities
            .row(first)
            .villager
            .as_ref()
            .unwrap()
            .household
            .as_ref(),
        Some(family)
    );
    assert_eq!(
        state
            .entities
            .row(second)
            .villager
            .as_ref()
            .unwrap()
            .household
            .as_ref(),
        Some(family)
    );
    assert_eq!(v.birth_sequence, Some(state.tick * 65_536 + 1));
    let origin = v.birth_origin.as_ref().expect("native birth provenance");
    assert_eq!(origin.started_tick, 0);
    assert_eq!(origin.birth_tick, state.tick);
    assert!(
        state
            .plugins
            .domain_entries("__native_npcs")
            .all(|(key, _)| key != "npc/births" && !key.starts_with("courtship/"))
    );
    assert!(state.entities.row(baby).appearance.baby);
    let child_id = state.entities.body(baby).id;
    assert!(cache.newborn_homes.contains(&(child_id, child_home)));
    state.entities.remove(baby);
    cache.refresh(&state);
    assert!(
        !cache
            .homes
            .get(&child_home)
            .is_some_and(|owners| owners.contains(&child_id))
    );
}
#[test]
fn courtship_without_bed_digests_food_but_does_not_spawn_or_cool_down() {
    let mut state = test_state();
    state.tick = 300;
    let first = villager(&mut state, [8.0, 201.0, 8.0]);
    let second = villager(&mut state, [8.5, 201.0, 8.0]);
    let a = state.entities.body(first).id;
    let b = state.entities.body(second).id;
    let carrot = item(&state, "minecraft:carrot");
    for (slot, partner) in [(first, b), (second, a)] {
        let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
        v.inventory[0] = Some(ItemStack::new(carrot, 12));
        v.partner = Some(partner);
        v.courtship_until = Some(300);
        v.courtship_started_tick = Some(0);
    }
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    let mut changes = ChangeSet::default();
    courtship(&mut state, &mut cache, &mut changes);
    assert!(changes.entities_spawned.is_empty());
    for parent in [first, second] {
        assert_eq!(state.entities.row(parent).villager.as_ref().unwrap().age, 0);
        assert_eq!(food_count(&state, parent, carrot), 0);
    }
}
#[test]
fn baby_grows_to_adult_and_updates_metadata() {
    let mut state = test_state();
    state.day_time = 1000;
    let slot = villager(&mut state, [8.0, 201.0, 8.0]);
    state.entities.row_mut(slot).villager.as_mut().unwrap().age = -1;
    super::super::sync_appearance(&mut state, slot);
    assert!(state.entities.row(slot).appearance.baby);
    step(&mut state);
    assert_eq!(state.entities.row(slot).villager.as_ref().unwrap().age, 0);
    assert!(!state.entities.row(slot).appearance.baby);
}
#[test]
fn baby_crossing_to_adult_during_budget_skip_updates_appearance() {
    let mut state = test_state();
    state.tick = 1;
    state.day_time = 1000;
    for _ in 0..DECISIONS_PER_TICK + 17 {
        let slot = villager(&mut state, [8.5, 201.0, 8.5]);
        state.entities.row_mut(slot).villager.as_mut().unwrap().age = -1;
        super::super::sync_appearance(&mut state, slot);
    }
    step(&mut state);
    let mut skipped = 0;
    for slot in state.entities.slots() {
        if state.villager_pois.admitted(slot) {
            continue;
        }
        skipped += 1;
        assert_eq!(state.entities.row(slot).villager.as_ref().unwrap().age, 0);
        assert!(
            !state.entities.row(slot).appearance.baby,
            "budget-skipped adult kept baby appearance"
        );
    }
    assert!(skipped > 0);
}
#[test]
fn navigation_routes_around_a_wall_instead_of_steering_through_it() {
    let mut state = test_state();
    let stone = state.registry.states.stone;
    for z in 4..=10 {
        for y in 201..204 {
            state.world.set_block(8, y, z, stone);
        }
    }
    state.world.fold_overlay();
    let mut cache = PoiCache::default();
    let mut path = Vec::new();
    state.tick_budget.reset(state.tick);
    assert!(find_path(
        &mut state,
        &mut cache,
        [6.5, 201.0, 7.5],
        BlockPos {
            x: 10,
            y: 201,
            z: 7
        },
        0.3,
        1.95,
        &mut path
    ));
    assert!(path.iter().any(|p| p.z < 4 || p.z > 10));
    assert!(
        path.iter()
            .all(|p| standing(&state.world, &state.registry, *p, 0.3, 1.95).is_some())
    );
}
#[test]
fn gossip_caps_decay_and_transfer_match_local_oracle() {
    let player = uuid::Uuid::from_u128(1);
    let mut v = VillagerState::default();
    for _ in 0..10 {
        record_gossip(&mut v, player, "minor_negative", 25);
    }
    assert_eq!(v.gossips[0].value, 200);
    v.last_gossip_decay = 1;
    decay_gossip(&mut v, 24001);
    assert_eq!(v.gossips[0].value, 180);
    let mut receiver = VillagerState::default();
    transfer_gossip(&mut receiver, &v, 42);
    assert_eq!(receiver.gossips[0].value, 160);
    record_gossip(&mut v, player, "major_positive", 20);
    v.last_gossip_decay = 1;
    decay_gossip(&mut v, 24001);
    assert_eq!(
        v.gossips
            .iter()
            .find(|g| g.kind == "major_positive")
            .unwrap()
            .value,
        20
    );
}

#[test]
fn rest_sleeps_in_owned_bed_and_morning_wakes_metadata() {
    let mut state = test_state();
    let home = bed(&mut state, 8, 8);
    state.world.fold_overlay();
    let slot = villager(&mut state, [9.5, 201.0, 8.5]);
    state.entities.row_mut(slot).villager.as_mut().unwrap().home = Some(home);
    state.day_time = 13000;
    step(&mut state);
    assert!(state.entities.row(slot).villager.as_ref().unwrap().sleeping);
    assert_eq!(state.entities.row(slot).appearance.pose, 2);
    assert!(state.entities.row(slot).appearance.extra.iter().any(|(_, value)|
        matches!(value, crate::boundary::EntityDataValue::OptionalBlockPos(Some(p)) if *p == [8, 201, 8])));
    state.day_time = 1000;
    step(&mut state);
    assert!(!state.entities.row(slot).villager.as_ref().unwrap().sleeping);
    assert_eq!(state.entities.row(slot).appearance.pose, 0);
    assert_eq!(state.entities.body(slot).height, 1.95);
}

#[test]
fn resting_villager_moves_onto_nearby_indoor_stair() {
    let mut state = test_state();
    let stair = block(&state, "minecraft:oak_stairs", &[("half", "bottom")]);
    state.world.set_block(9, 201, 8, stair);
    state
        .world
        .set_block(9, 204, 8, state.registry.states.stone);
    state.world.fold_overlay();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    state.day_time = 18000;
    step(&mut state);
    let position = state.entities.pos(slot);
    assert_eq!((position[0], position[2]), (9.5, 8.5));
    let id = state.entities.body(slot).id;
    let seat = BlockPos { x: 9, y: 201, z: 8 };
    assert_eq!(
        state
            .entities
            .views()
            .get(id)
            .unwrap()
            .appearance
            .villager_seat,
        Some(seat)
    );
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    assert!(serde_json::to_value(v).unwrap().get("seat").is_none());
    assert!(
        crate::villager::persistence::journal_image(v)
            .seat
            .is_none()
    );
    assert!(crate::villager::persistence::journal_eq(
        v,
        &crate::villager::persistence::journal_image(v)
    ));
    state.day_time = 1000;
    step(&mut state);
    assert_eq!(
        state
            .entities
            .views()
            .get(id)
            .unwrap()
            .appearance
            .villager_seat,
        None
    );
    state.day_time = 18000;
    step(&mut state);
    assert_eq!(
        state
            .entities
            .views()
            .get(id)
            .unwrap()
            .appearance
            .villager_seat,
        Some(seat)
    );
    state.day_time = 3000;
    step(&mut state);
    assert_eq!(
        state
            .entities
            .views()
            .get(id)
            .unwrap()
            .appearance
            .villager_seat,
        None
    );
    state.day_time = 18000;
    step(&mut state);
    assert_eq!(
        state
            .entities
            .views()
            .get(id)
            .unwrap()
            .appearance
            .villager_seat,
        Some(seat)
    );
    state.entities.set_pos(slot, [13.5, 201.0, 8.5]);
    step(&mut state);
    assert_eq!(
        state
            .entities
            .views()
            .get(id)
            .unwrap()
            .appearance
            .villager_seat,
        None
    );
}

#[test]
fn resting_villager_walks_to_its_own_bed_past_a_seat() {
    let mut state = test_state();
    let home = bed(&mut state, 15, 8);
    let stair = block(&state, "minecraft:oak_stairs", &[("half", "bottom")]);
    state.world.set_block(9, 201, 8, stair);
    state
        .world
        .set_block(9, 204, 8, state.registry.states.stone);
    state.world.fold_overlay();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    state.entities.row_mut(slot).villager.as_mut().unwrap().home = Some(home);
    state.day_time = 18000;
    step(&mut state);
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(v.seat, None, "its own bed comes before a nearby seat");
    assert!(
        !v.navigation_path.is_empty(),
        "the resident is routed towards its bed"
    );
}

#[test]
fn stair_under_a_solid_block_is_not_a_seat() {
    let mut state = test_state();
    let stair = block(&state, "minecraft:oak_stairs", &[("half", "bottom")]);
    state.world.set_block(9, 201, 8, stair);
    state
        .world
        .set_block(9, 202, 8, state.registry.states.stone);
    state.world.fold_overlay();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    state.day_time = 18000;
    step(&mut state);
    let id = state.entities.body(slot).id;
    assert_eq!(
        state
            .entities
            .views()
            .get(id)
            .unwrap()
            .appearance
            .villager_seat,
        None
    );
    let position = state.entities.pos(slot);
    assert_ne!((position[0], position[2]), (9.5, 8.5));
}

#[test]
fn foreign_dimension_pois_do_not_steer_overworld_villagers() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let id = state.entities.body(slot).id;
    let home = bed(&mut state, 8, 8);
    let meeting = BlockPos {
        x: 10,
        y: 201,
        z: 8,
    };
    let job = BlockPos {
        x: 11,
        y: 201,
        z: 8,
    };
    let bell = block(&state, "minecraft:bell", &[]);
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(meeting.x, meeting.y, meeting.z, bell);
    state.world.set_block(job.x, job.y, job.z, composter);
    let mut v = VillagerState {
        home: Some(home),
        home_dimension: "minecraft:the_nether".to_owned(),
        meeting_point: Some(meeting),
        meeting_point_dimension: "minecraft:the_end".to_owned(),
        job_site: Some(job),
        job_site_dimension: "minecraft:the_nether".to_owned(),
        ..Default::default()
    };
    v.data.profession = Profession::Farmer;
    v.navigation_path.push(home);
    v.navigation_target = Some(home);
    capture_route(&state, &mut v, centre(home), true);
    v.navigation_repath_at = state.tick + 40;
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    validate_pois(&state, &cache, id, &mut v);
    assert!(v.home.is_none());
    assert!(v.meeting_point.is_none());
    assert!(v.job_site.is_none());
    assert_eq!(v.data.profession, Profession::None);
    assert!(v.navigation_path.is_empty());
    assert!(v.navigation_target.is_none());
    assert!(v.navigation_chunks.is_empty());
    assert_eq!(v.navigation_repath_at, 0);

    // A valid same-world memory survives, including in the Nether namespace.
    state.dimension = crate::dimensions::Dimension::Nether;
    v.home = Some(home);
    v.meeting_point = Some(meeting);
    v.job_site = Some(job);
    v.home_dimension = state.dimension.name().into();
    v.meeting_point_dimension = state.dimension.name().into();
    v.job_site_dimension = state.dimension.name().into();
    cache.refresh(&state);
    validate_pois(&state, &cache, id, &mut v);
    assert_eq!(v.home, Some(home));
    assert_eq!(v.meeting_point, Some(meeting));
    assert_eq!(v.job_site, Some(job));

    // Unloaded local cells are not evidence that a retained POI vanished.
    let unloaded = BlockPos {
        x: 1600,
        y: 201,
        z: 1600,
    };
    v.home = Some(unloaded);
    validate_pois(&state, &cache, id, &mut v);
    assert_eq!(v.home, Some(unloaded));
}

#[test]
fn birth_bed_requires_two_air_blocks_above_both_halves() {
    let mut state = test_state();
    let home = bed(&mut state, 8, 8);
    assert!(bed_has_baby_space(&state, home));
    state
        .world
        .set_block(7, 203, 8, state.registry.states.stone);
    assert!(!bed_has_baby_space(&state, home));
}

#[test]
fn merchant_menu_stops_navigation_and_work_until_live_menu_closes() {
    let mut state = test_state();
    state.tick = 1000;
    state.day_time = 3000;
    let job = BlockPos { x: 8, y: 201, z: 8 };
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(job.x, job.y, job.z, composter);
    state.world.fold_overlay();
    let slot = villager(&mut state, [7.5, 201.0, 8.5]);
    let id = state.entities.body(slot).id;
    let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
    v.data.profession = Profession::Farmer;
    v.job_site = Some(job);
    let mut player = crate::player::PlayerState::new(
        crate::boundary::ConnectionId(42),
        crate::commands::GameMode::Survival,
    );
    let mut menu = crate::inventory::Menu::new(1, crate::inventory::MenuKind::Merchant);
    menu.merchant = Some(crate::inventory::MerchantMenu {
        entity: id,
        selected: 0,
        last_restock: 0,
        reputation: 0,
        hero: None,
    });
    player.open = Some(menu);
    let customer = EntityId(99);
    state.players.insert(customer, player);
    let pos = state.entities.pos(slot);
    step(&mut state);
    assert_eq!(state.entities.pos(slot), pos);
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(v.last_work, 0);
    assert_eq!(v.job_site, Some(job));
    state.players.get_mut(&customer).unwrap().open = None;
    step(&mut state);
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .last_work,
        1001
    );
}

fn checkpoint_control_state(state: &mut TickState, config: &crate::CoreConfig) {
    use crate::native_checkpoint as bridge;
    let store = bridge::tests::fixture_store(config).unwrap();
    let (_, captured, block_entities) = crate::core::checkpoint_data_for(state, state.tick);
    let entities = bridge::encode_entities(
        state.dimension,
        &captured,
        &state.registry.items,
        state.tick,
    )
    .unwrap();
    let capture = state.world.capture_dirty(&block_entities);
    let mut image = store.read_checkpoint().unwrap();
    let mut manifest: crate::journal::CheckpointManifest =
        serde_json::from_slice(&image.globals["checkpoint"].payload).unwrap();
    manifest.tick = state.tick;
    manifest.tickets = crate::core::checkpoint_tickets(state, state.tick);
    manifest.gamerules = state
        .gamerules
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    for child in manifest.dimensions.values_mut() {
        child.tick = state.tick;
    }
    image.globals.insert(
        "checkpoint".into(),
        bridge::record(state.tick, serde_json::to_vec(&manifest).unwrap()),
    );
    bridge::refresh_metadata(&mut image.globals, &manifest).unwrap();
    for global in image.globals.values_mut() {
        global.captured_tick = state.tick;
    }
    let entity_iter = entities.iter().map(Ok);
    store
        .commit_checkpoint_stream(
            mc_world::native::NativeCheckpoint {
                tick: state.tick,
                previous_tick: image.tick,
                chunks: &[],
                entities: &[],
                players: &[],
                globals: &image.globals,
            },
            |write| {
                capture
                    .visit_native(|snapshot| {
                        write(snapshot)
                            .map_err(|error| crate::config::CoreError::Storage(error.to_string()))
                    })
                    .map_err(|error| mc_world::native::NativeError::Source(error.to_string()))
            },
            entity_iter,
            &state.registry.blocks,
            &state.registry.items,
        )
        .unwrap();
}

#[test]
fn village_control_claim_and_profession_survive_native_checkpoint_wal_and_anvil() {
    let directory = tempfile::tempdir().unwrap();
    let config = crate::CoreConfig::new(directory.path().join("world"), 42);
    let mut state =
        crate::core::recover_dimension_state(&config, crate::dimensions::Dimension::Overworld)
            .unwrap();
    state.tickets.insert(
        "village-control".into(),
        (crate::boundary::ChunkPos { x: 0, z: 0 }, 0),
    );
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    state
        .gamerules
        .insert("advance_time".into(), "false".into());
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    for x in 0..16 {
        for z in 0..16 {
            state
                .world
                .set_block(x, 200, z, state.registry.states.stone);
            for y in 201..206 {
                state.world.set_block(x, y, z, state.registry.states.air);
            }
        }
    }
    let workstation = BlockPos { x: 4, y: 201, z: 4 };
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(4, 201, 4, composter);
    state.world.fold_overlay();
    let slot = villager(&mut state, [3.322834405812458, 201.0, 3.057086014531166]);
    let id = state.entities.body(slot).id;
    let uuid = state.entities.row(slot).uuid;
    // Checkpoint before acquisition; seed the journal's unchanged-state cache.
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .job_site
            .is_none()
    );
    checkpoint_control_state(&mut state, &config);
    let checkpoint_pos = state.entities.pos(slot);
    let mut writer = crate::journal::JournalWriter::open(&config.world_dir, 1).unwrap();
    state.day_time = 2000;
    for _ in 0..100 {
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        let entries = crate::core::journal_entries_for_tick(&mut state, &output);
        writer.append(output.tick, output.tick, &entries).unwrap();
    }
    writer.flush().unwrap();
    drop(writer);
    let expected = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(expected.job_site, Some(workstation));
    assert_eq!(expected.data.profession, Profession::Farmer);
    assert_ne!(state.entities.pos(slot), checkpoint_pos);
    let mut restored =
        crate::core::recover_dimension_state(&config, crate::dimensions::Dimension::Overworld)
            .unwrap();
    let restored_slot = restored.entities.slot_of(id).unwrap();
    let v = restored
        .entities
        .row(restored_slot)
        .villager
        .as_ref()
        .unwrap();
    assert_eq!(v.job_site, Some(workstation));
    assert_eq!(v.data.profession, Profession::Farmer);
    assert_eq!(v.job_site_dimension, "minecraft:overworld");
    assert_eq!(
        restored.entities.pos(restored_slot),
        checkpoint_pos,
        "ordinary mob motion remains checkpoint-durable (ADR 0013)",
    );
    assert_eq!(restored.entities.row(restored_slot).uuid, uuid);
    assert_eq!(
        restored.entities.len(),
        1,
        "WAL must replace the checkpoint identity, not duplicate it"
    );
    // A second checkpoint at the replayed tick must retain the same claim.
    checkpoint_control_state(&mut restored, &config);
    drop(restored);
    let mut restored =
        crate::core::recover_dimension_state(&config, crate::dimensions::Dimension::Overworld)
            .unwrap();
    let want = restored.desired_chunks();
    restored.world.ensure_blocking(&want);
    assert_eq!(restored.world.block_at(4, 201, 4), Some(composter));
    assert_eq!(
        restored
            .entities
            .row(restored.entities.slot_of(id).unwrap())
            .villager
            .as_ref()
            .unwrap()
            .job_site,
        Some(workstation),
    );
    restored.day_time = 2000;
    let competitor = villager(&mut restored, [2.5, 201.0, 2.5]);
    for _ in 0..40 {
        crate::tick::run_tick(&mut restored, &[]);
    }
    assert!(
        restored
            .entities
            .row(competitor)
            .villager
            .as_ref()
            .unwrap()
            .job_site
            .is_none()
    );
    assert_eq!(
        restored
            .entities
            .slots()
            .filter(|slot| {
                restored
                    .entities
                    .row(*slot)
                    .villager
                    .as_ref()
                    .unwrap()
                    .job_site
                    == Some(workstation)
            })
            .count(),
        1,
        "recovery must preserve exclusive POI ownership",
    );
    drop(restored);
    let registry = Arc::clone(&state.registry);
    let export_tick = state.tick;
    drop(state);
    let anvil = directory.path().join("anvil");
    crate::world_conversion::export_anvil(
        &config,
        &config.world_dir,
        &anvil,
        &directory.path().join("export-receipt.json"),
    )
    .unwrap();
    let regions = mc_world::anvil::entity_regions::read_entity_regions(
        &crate::dimensions::Dimension::Overworld
            .directory(&anvil)
            .join("entities"),
        &registry.items,
    )
    .unwrap();
    let entities: Vec<_> = regions.values().flat_map(|chunk| &chunk.entities).collect();
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0].uuid, uuid);
    let exported = super::super::persistence::villager_from_fields(
        &registry.items,
        &entities[0].extra_fields,
        export_tick,
    )
    .unwrap();
    assert_eq!(exported.data.profession, Profession::Farmer);
    assert_eq!(exported.job_site, Some(workstation));
    assert_eq!(exported.job_site_dimension, "minecraft:overworld");
    let fields = &entities[0].extra_fields;
    let mc_nbt::Tag::Compound(data) = &fields
        .iter()
        .find(|(key, _)| key == "VillagerData")
        .unwrap()
        .1
    else {
        panic!("VillagerData must be a native compound");
    };
    assert!(data.contains(&(
        "profession".into(),
        mc_nbt::Tag::String("minecraft:farmer".into())
    )));
    let mc_nbt::Tag::Compound(brain) = &fields.iter().find(|(key, _)| key == "Brain").unwrap().1
    else {
        panic!("Brain must be a native compound");
    };
    let mc_nbt::Tag::Compound(memories) =
        &brain.iter().find(|(key, _)| key == "memories").unwrap().1
    else {
        panic!("Brain memories must be a native compound");
    };
    assert_eq!(
        memories
            .iter()
            .filter(|(key, _)| key == "minecraft:job_site")
            .count(),
        1
    );
}

fn brain_benchmark_state(count: i32, prefix_mobs: i32) -> TickState {
    struct Flat(u32, u32);
    impl mc_world::ChunkGenerator for Flat {
        fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
            let mut chunk = mc_world::Chunk::empty(
                pos,
                mc_world::BlockStateId(self.0),
                mc_data::Identifier::parse("minecraft:plains").unwrap(),
            );
            for x in 0..16 {
                for z in 0..16 {
                    chunk.set_block(x, 200, z, mc_world::BlockStateId(self.1));
                }
            }
            chunk.status = "minecraft:full".into();
            chunk
        }
    }
    let registry = Arc::new(CoreRegistry::load(None).unwrap());
    let world = crate::world::ChunkWorld::new(
        42,
        Arc::new(Flat(registry.states.air, registry.states.stone)),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [2, 201, 2],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 1, 512);
    if prefix_mobs != 0 {
        let cow = state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse("minecraft:cow").unwrap())
            .unwrap();
        for _ in 0..prefix_mobs {
            state.spawn_mob(cow, [5_000.5, 201.0, 5_000.5], 20.0);
        }
    }
    let mut want = FxHashMap::default();
    for i in 0..count {
        let x = (i % 100) * 4 + 2;
        let z = (i / 100) * 4 + 2;
        want.insert(
            ChunkPos {
                x: x / 16,
                z: z / 16,
            },
            0,
        );
    }
    state.world.ensure_blocking(&want);
    activate_fixture_chunks(&mut state, want.keys().copied());
    let composter = block(&state, "minecraft:composter", &[]);
    for i in 0..count {
        let x = (i % 100) * 4 + 2;
        let z = (i / 100) * 4 + 2;
        let home = bed(&mut state, x, z);
        let job = BlockPos {
            x,
            y: 201,
            z: z + 1,
        };
        state.world.set_block(job.x, job.y, job.z, composter);
        let slot = villager(&mut state, [f64::from(x) + 1.5, 201.0, f64::from(z) + 0.5]);
        let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
        v.home = Some(home);
        v.job_site = (i % 20 != 0).then_some(job);
        v.data.profession = Profession::Farmer;
    }
    state.world.fold_overlay();
    state.day_time = 3000;
    state.tick = 1;
    state
}

/// Advisor measurement only: the override and timers do not exist in servers.
#[test]
#[ignore = "manual uncapped 50k individual-decision phase profile"]
fn villager_all_brains_profile() {
    const COUNT: usize = 50_000;
    let mut state = brain_benchmark_state(COUNT as i32, 0);
    let warmed_ticks = COUNT.div_ceil(DECISIONS_PER_TICK) + 80;
    for _ in 0..warmed_ticks {
        step(&mut state);
    }
    let cache = &mut state.villager_pois;
    cache.measure_decisions = true;
    cache.selected.reserve(COUNT);
    cache.decision_ns.reserve(COUNT);
    for samples in &mut cache.decision_phases {
        samples.reserve(COUNT);
    }
    let begin = std::time::Instant::now();
    step(&mut state);
    let total = begin.elapsed().as_micros();
    let cache = &mut state.villager_pois;
    assert_eq!(cache.selected.len(), COUNT);
    assert_eq!(cache.decision_ns.len(), COUNT);
    cache.decision_ns.sort_unstable();
    println!(
        "villager_all_brains count={COUNT} warmed_ticks={warmed_ticks} total_us={total} decision_p50_ns={} decision_p95_ns={} decision_p99_ns={} decision_max_ns={} path_budget_exhausted_villagers={} social_us={}",
        cache.decision_ns[COUNT / 2],
        cache.decision_ns[(COUNT * 95).div_ceil(100) - 1],
        cache.decision_ns[(COUNT * 99).div_ceil(100) - 1],
        cache.decision_ns[COUNT - 1],
        cache.path_denied.len(),
        cache.phase_us[3],
    );
    for (index, phase) in [
        "validate_pois_bed_pair",
        "sleepers",
        "threat",
        "pickup_items_near",
        "share_food_bodies_near",
        "move_to_find_path",
        "trades_work",
        "gossip_decay",
    ]
    .into_iter()
    .enumerate()
    {
        let samples = &mut cache.decision_phases[index];
        assert_eq!(samples.len(), COUNT);
        samples.sort_unstable();
        println!(
            "villager_all_brains_phase phase={phase} calls={} total_ns={} p50_ns={} p99_ns={} max_ns={}",
            cache.decision_calls[index],
            samples.iter().sum::<u128>(),
            samples[COUNT / 2],
            samples[(COUNT * 99).div_ceil(100) - 1],
            samples[COUNT - 1],
        );
    }
    // Keep even subsequent ignored tests on the ordinary hard admission path.
    cache.measure_decisions = false;
}

/// In-process owner-thread workload, excluding world construction/publication.
/// Run release/ignored with --nocapture; keep the same fixture for A/B receipts.
#[test]
#[ignore = "manual 1k/10k villager brain performance receipt"]
fn villager_brain_scale_benchmark() {
    for count in [1_000, 10_000, 50_000] {
        let mut state = brain_benchmark_state(count, 0);
        let mut times = Vec::new();
        let mut phases: [Vec<u128>; 7] = std::array::from_fn(|_| Vec::new());
        for _ in 0..241 {
            let begin = std::time::Instant::now();
            step(&mut state);
            times.push(begin.elapsed().as_micros());
            for (samples, cost) in phases.iter_mut().zip(state.villager_pois.phase_us) {
                samples.push(cost);
            }
        }
        let cold = times.remove(0);
        times.sort_unstable();
        println!(
            "villager_brain count={count} samples={} cold_us={cold} p50_us={} p95_us={} p99_us={} max_us={}",
            times.len(),
            times[times.len() / 2],
            times[(times.len() * 95).div_ceil(100) - 1],
            times[(times.len() * 99).div_ceil(100) - 1],
            times[times.len() - 1],
        );
        for (phase, mut samples) in [
            "refresh",
            "admission",
            "evaluation",
            "social",
            "acquisition",
            "full",
            "skipped",
        ]
        .into_iter()
        .zip(phases)
        {
            samples.remove(0);
            samples.sort_unstable();
            println!(
                "villager_brain_phase count={count} phase={phase} p50_us={} p95_us={} p99_us={} max_us={}",
                samples[samples.len() / 2],
                samples[(samples.len() * 95).div_ceil(100) - 1],
                samples[(samples.len() * 99).div_ceil(100) - 1],
                samples[samples.len() - 1]
            );
        }
    }
}

// Admission/eviction and one resident activity boundary per tick, prepared
// outside the allocator/brain timer just as the root prepares chunk residency.
fn churn_allocator_scene(state: &mut TickState) {
    let marker = ChunkPos {
        x: 1_000 + (state.tick % 2) as i32,
        z: 1_000,
    };
    state
        .world
        .ensure_blocking(&FxHashMap::from_iter([(marker, 0)]));
    let want = state
        .world
        .chunks()
        .filter(|chunk| chunk.pos.z != 1_000 || chunk.pos == marker)
        .map(|chunk| (chunk.pos, 0))
        .collect();
    state.world.evict_outside(&want);
    let target = ChunkPos { x: 0, z: 0 };
    let mut active: FxHashSet<_> = state.world.active_positions().collect();
    if !active.remove(&target) {
        active.insert(target);
    }
    state.world.set_active_chunks(active, state.tick);
}

/// Exercise collision-backed execution independently of admission, not just
/// the stationary decision fast path. Seeded paths are ordinary committed
/// routes; no test-only motion or decision bypass is enabled.
#[test]
#[ignore = "manual idle/moving 10k/50k brain gate, 240 measured samples"]
fn villager_brain_activity_scale_benchmark() {
    for count in [10_000, 50_000] {
        for mode in ["idle", "moving", "churn", "mixed_churn"] {
            if matches!(mode, "churn" | "mixed_churn") && count != 50_000 {
                continue;
            }
            let prefix_mobs = if mode == "mixed_churn" { 320 } else { 0 };
            let mut state = brain_benchmark_state(count, prefix_mobs);
            let mut churn_mobs: std::collections::VecDeque<_> = (0..prefix_mobs)
                .map(|i| state.entities.body(EntitySlot(i as u32)).id)
                .collect();
            let mut movers = Vec::new();
            for i in 0..count {
                let slot = EntitySlot((i + prefix_mobs) as u32);
                if mode == "idle" {
                    let home = state
                        .entities
                        .row(slot)
                        .villager
                        .as_ref()
                        .unwrap()
                        .home
                        .unwrap();
                    state.entities.set_pos(
                        slot,
                        [f64::from(home.x) + 0.5, 201.6875, f64::from(home.z) + 0.5],
                    );
                    let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
                    v.activity = Activity::Rest;
                    v.sleeping = true;
                    v.last_slept = Some(state.tick);
                } else if i % 5 == 0 {
                    let origin = state.entities.pos(slot);
                    let x = origin[0].floor() as i32;
                    let z = origin[2].floor() as i32 - 1;
                    let direction = if x < 200 { 1 } else { -1 };
                    let start = [origin[0], origin[1], f64::from(z) + 0.5];
                    state.entities.set_pos(slot, start);
                    let mut v = state.entities.row_mut(slot).villager.take().unwrap();
                    v.activity = Activity::Idle;
                    v.data.profession = Profession::Nitwit;
                    v.job_site = None;
                    v.navigation_target = Some(BlockPos {
                        x: x + direction * 96,
                        y: 201,
                        z,
                    });
                    v.navigation_repath_at = u64::MAX;
                    v.navigation_path = (1..=96)
                        .rev()
                        .map(|step| BlockPos {
                            x: x + direction * step,
                            y: 201,
                            z,
                        })
                        .collect();
                    capture_route(&state, &mut v, start, true);
                    state.entities.row_mut(slot).villager = Some(v);
                    movers.push((state.entities.body(slot).id, start));
                }
            }
            if mode == "idle" {
                state.day_time = 0;
            }
            let mut last_service =
                vec![state.tick; state.entities.wire_id_high_water() as usize + 1];
            for _ in 0..80 {
                if matches!(mode, "churn" | "mixed_churn") {
                    churn_allocator_scene(&mut state);
                }
                if let Some(id) = churn_mobs.pop_front() {
                    state.remove_entity(state.entities.slot_of(id).unwrap());
                }
                step(&mut state);
                for &slot in &state.villager_pois.selected {
                    last_service[state.entities.body(slot).id.0 as usize] = state.tick - 1;
                }
            }
            for (id, position) in &mut movers {
                *position = state.entities.pos(state.entities.slot_of(*id).unwrap());
            }
            let mut times = Vec::with_capacity(240);
            let mut phases: [Vec<u128>; 7] = std::array::from_fn(|_| Vec::with_capacity(240));
            let mut allocator_us = Vec::with_capacity(240);
            let mut roster_us = Vec::with_capacity(240);
            let mut roster_rebuilds = 0;
            let mut minimum_moving = usize::MAX;
            let mut max_service_gap = 0;
            for _ in 0..240 {
                if matches!(mode, "churn" | "mixed_churn") {
                    churn_allocator_scene(&mut state);
                }
                if let Some(id) = churn_mobs.pop_front() {
                    state.remove_entity(state.entities.slot_of(id).unwrap());
                }
                let begin = std::time::Instant::now();
                step(&mut state);
                times.push(begin.elapsed().as_micros());
                roster_us.push(state.villager_pois.roster_us);
                roster_rebuilds += state.villager_pois.roster_rebuilds;
                allocator_us.push(
                    times
                        .last()
                        .unwrap()
                        .saturating_sub(state.villager_pois.brain_us),
                );
                for (samples, elapsed) in phases.iter_mut().zip(state.villager_pois.phase_us) {
                    samples.push(elapsed);
                }
                let serviced_at = state.tick - 1;
                for &slot in &state.villager_pois.selected {
                    let previous = &mut last_service[state.entities.body(slot).id.0 as usize];
                    max_service_gap = max_service_gap.max(serviced_at - *previous);
                    *previous = serviced_at;
                }
                let mut moved = 0;
                for (id, previous) in &mut movers {
                    let current = state.entities.pos(state.entities.slot_of(*id).unwrap());
                    moved += usize::from(current[0] != previous[0] || current[2] != previous[2]);
                    *previous = current;
                }
                minimum_moving = minimum_moving.min(moved);
            }
            if mode == "moving" {
                assert!(
                    minimum_moving >= count as usize / 5,
                    "the gate must actually execute at least 20% moving residents every measured tick"
                );
            }
            let max_queued_wait = state.tick
                - 1
                - state
                    .villager_pois
                    .resident_data
                    .iter()
                    .map(|brain| last_service[brain.id.0 as usize])
                    .min()
                    .unwrap();
            let far_target = 32;
            times.sort_unstable();
            println!(
                "villager_brain_activity count={count} mode={mode} warmed_ticks=80 samples={} min_moving={minimum_moving} max_service_gap={max_service_gap} max_queued_wait={max_queued_wait} far_target={far_target} p50_us={} p95_us={} p99_us={} max_us={}",
                times.len(),
                times[times.len() / 2],
                times[(times.len() * 95).div_ceil(100) - 1],
                times[(times.len() * 99).div_ceil(100) - 1],
                times[times.len() - 1]
            );
            roster_us.sort_unstable();
            println!(
                "villager_brain_roster count={count} mode={mode} samples={} rebuilds={roster_rebuilds} p50_us={} p95_us={} p99_us={} max_us={}",
                roster_us.len(),
                roster_us[roster_us.len() / 2],
                roster_us[(roster_us.len() * 95).div_ceil(100) - 1],
                roster_us[(roster_us.len() * 99).div_ceil(100) - 1],
                roster_us[roster_us.len() - 1],
            );
            if mode == "mixed_churn" {
                assert_eq!(
                    roster_rebuilds, 0,
                    "prefix removals only change resident slots"
                );
            }
            allocator_us.sort_unstable();
            println!(
                "villager_brain_activity_phase count={count} mode={mode} phase=allocator_and_budget samples={} p50_us={} p95_us={} p99_us={} max_us={}",
                allocator_us.len(),
                allocator_us[allocator_us.len() / 2],
                allocator_us[(allocator_us.len() * 95).div_ceil(100) - 1],
                allocator_us[(allocator_us.len() * 99).div_ceil(100) - 1],
                allocator_us[allocator_us.len() - 1]
            );
            for (name, index) in [
                ("refresh", 0),
                ("prepare", 1),
                ("acquire", 4),
                ("full_decisions", 5),
                ("physical_continuation", 6),
                ("social", 3),
            ] {
                let samples = &mut phases[index];
                samples.sort_unstable();
                println!(
                    "villager_brain_activity_phase count={count} mode={mode} phase={name} samples={} p50_us={} p95_us={} p99_us={} max_us={}",
                    samples.len(),
                    samples[samples.len() / 2],
                    samples[(samples.len() * 95).div_ceil(100) - 1],
                    samples[(samples.len() * 99).div_ceil(100) - 1],
                    samples[samples.len() - 1]
                );
            }
        }
    }
}

#[test]
#[ignore = "manual exhausted-A* node allowance cost, 200 samples per rung"]
fn villager_path_budget_scale_benchmark() {
    let mut state = brain_benchmark_state(10_000, 0);
    let start = [203.5, 201.0, 202.5];
    let target = BlockPos {
        x: 270,
        y: 201,
        z: 270,
    };
    let mut cache = PoiCache::default();
    let mut path = Vec::new();
    for nodes in [512, 768, 1_024, 1_536] {
        let mut times = Vec::with_capacity(200);
        for sample in 0..210 {
            set_node_allowance(&mut state, nodes);
            let begin = std::time::Instant::now();
            let found = find_path(&mut state, &mut cache, start, target, 0.3, 1.95, &mut path);
            let elapsed = begin.elapsed().as_nanos();
            assert!(
                !found,
                "target deliberately lies beyond the bounded search radius"
            );
            assert_eq!(
                state.tick_budget.remaining(Kind::PathNode),
                0,
                "exercise the entire node allowance"
            );
            if sample >= 10 {
                times.push(elapsed);
            }
        }
        times.sort_unstable();
        println!(
            "villager_path_budget nodes={nodes} samples={} p50_ns={} p95_ns={} p99_ns={} max_ns={}",
            times.len(),
            times[times.len() / 2],
            times[(times.len() * 95).div_ceil(100) - 1],
            times[(times.len() * 99).div_ceil(100) - 1],
            times[times.len() - 1],
        );
    }
}

/// MIX-01 population through the complete core tick and WAL-image capture.
/// Flat terrain isolates simulation from world generation and network fanout.
/// Retain distant clearing drops as in the public fixture: item sensors must
/// not turn every villager into a scan of every dropped item in the world.
#[test]
#[ignore = "manual 25-quarter/10k villager full-tick performance receipt"]
fn villager_full_tick_scale_benchmark() {
    use crate::boundary::{ConnectionId, PlayerInput};
    let registry = Arc::new(crate::tick_tests::full_registry());
    let world = crate::horde_bench::flat_world(&registry);
    let mut state = TickState::new(registry, world, 2, 512);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    let joins: Vec<_> = (1..=100)
        .map(|id| PlayerInput::Join {
            connection: ConnectionId(id),
            uuid: uuid::Uuid::from_u128(u128::from(id)),
            name: format!("VillagerBench{id}"),
            permission_level: 0,
        })
        .collect();
    crate::tick::run_tick(&mut state, &joins);
    for quarter in 0..25 {
        let x = 384 + (quarter % 5) * 128;
        let z = 384 + (quarter / 5) * 128;
        for player in 0..4 {
            let id = state
                .connections
                .entity_of(ConnectionId((quarter * 4 + player + 1) as u64))
                .unwrap();
            let slot = state.entities.slot_of(id).unwrap();
            state
                .entities
                .set_pos(slot, [f64::from(x) + 88.5, 64.0, f64::from(z) + 56.5]);
        }
        state.tickets.insert(
            format!("villager-bench-{quarter}"),
            (
                crate::boundary::ChunkPos {
                    x: x / 16 + 3,
                    z: z / 16 + 3,
                },
                4,
            ),
        );
    }
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    let composter = block(&state, "minecraft:composter", &[]);
    let bed_foot = block(
        &state,
        "minecraft:red_bed",
        &[("part", "foot"), ("facing", "east")],
    );
    let bed_head = block(
        &state,
        "minecraft:red_bed",
        &[("part", "head"), ("facing", "east")],
    );
    for quarter in 0..25 {
        let ox = 384 + (quarter % 5) * 128;
        let oz = 384 + (quarter / 5) * 128;
        for i in 0..400 {
            let x = ox + (i % 20) * 4;
            let z = oz + (i / 20) * 4;
            state.world.set_block(x + 1, 64, z + 2, bed_foot);
            state.world.set_block(x + 2, 64, z + 2, bed_head);
            state.world.set_block(x, 64, z, composter);
            let slot = villager(&mut state, [f64::from(x) + 1.5, 64.0, f64::from(z) + 1.5]);
            let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
            v.home = Some(BlockPos {
                x: x + 2,
                y: 64,
                z: z + 2,
            });
            v.job_site = Some(BlockPos { x, y: 64, z });
            v.data.profession = Profession::Farmer;
        }
        for i in 0..200 {
            state
                .world
                .set_block(ox + (i % 20) * 4, 64, oz + 84 + (i / 20) * 4, composter);
        }
    }
    let stone = item(&state, "minecraft:stone");
    for _ in 0..1457 {
        crate::tick::spawn_item_entity(&mut state, [8.5, 64.0, 8.5], ItemStack::new(stone, 1));
    }
    state.world.fold_overlay();
    state.day_time = 2000;
    let mut tick_us = Vec::new();
    let mut brain_us = Vec::new();
    let mut journal_us = Vec::new();
    let mut full_images = Vec::new();
    let mut entry_counts = Vec::new();
    let mut touched_rows = Vec::new();
    for iteration in 0..1120 {
        let begin = std::time::Instant::now();
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        let tick = begin.elapsed().as_micros();
        let begin = std::time::Instant::now();
        let entries = crate::core::journal_entries_for_tick(&mut state, &output);
        std::hint::black_box(&entries);
        let journal = begin.elapsed().as_micros();
        if iteration == 999 {
            state.entities.take_touch_sites();
        }
        if iteration >= 1000 {
            tick_us.push(tick);
            brain_us.push(state.villager_pois.brain_us);
            journal_us.push(journal);
            full_images.push(
                entries
                    .iter()
                    .filter(|entry| matches!(entry, crate::journal::JournalEntry::Entity { .. }))
                    .count(),
            );
            entry_counts.push(entries.len());
            touched_rows.push(
                crate::journal_touched::LAST_TOUCHED.load(std::sync::atomic::Ordering::Relaxed),
            );
        }
    }
    for (phase, mut times) in [
        ("tick", tick_us),
        ("brain", brain_us),
        ("journal", journal_us),
    ] {
        times.sort_unstable();
        println!(
            "villager_full_tick phase={phase} quarters=25 villagers=10000 beds=10000 composters=15000 players=100 distant_items=1457 samples={} p50_us={} p95_us={} p99_us={} max_us={}",
            times.len(),
            times[60],
            times[113],
            times[118],
            times[119],
        );
    }
    full_images.sort_unstable();
    entry_counts.sort_unstable();
    println!(
        "villager_full_tick journal_entries_p50={} journal_entries_max={} full_entity_images_p50={} full_entity_images_max={}",
        entry_counts[60], entry_counts[119], full_images[60], full_images[119],
    );
    touched_rows.sort_unstable();
    println!(
        "villager_full_tick touched_rows_p50={} touched_rows_max={}",
        touched_rows[60], touched_rows[119],
    );
    for ((file, line), [calls, villagers, first]) in state.entities.take_touch_sites() {
        println!(
            "row_mut_site file={file} line={line} samples=120 calls={calls} villagers={villagers} first_touches={first}"
        );
    }
}

#[test]
fn poi_refresh_is_bounded_fair_and_never_offers_stale_pending_sites() {
    let registry = Arc::new(crate::tick_tests::full_registry());
    let world = crate::horde_bench::flat_world(&registry);
    let mut state = TickState::new(registry, world, 2, 512);
    let count = POI_REFRESH_CHUNKS_PER_TICK * 2 + 3;
    let want = FxHashMap::from_iter(
        (0..count).map(|x| (crate::boundary::ChunkPos { x: x as i32, z: 0 }, 0)),
    );
    state.world.ensure_blocking(&want);
    let composter = block(&state, "minecraft:composter", &[]);
    for pos in want.keys() {
        state.world.set_block(pos.x * 16, 64, 0, composter);
    }
    state.world.fold_overlay();
    let mut cache = PoiCache::default();
    state.world.swap_poi_revisions(&mut cache.revised_chunks);
    cache.refresh(&state);
    assert_eq!(
        cache.chunks.values().filter(|entry| !entry.queued).count(),
        POI_REFRESH_CHUNKS_PER_TICK,
    );
    assert_eq!(
        cache.pending_chunks.len(),
        count - POI_REFRESH_CHUNKS_PER_TICK
    );
    let pending = *cache.pending_chunks.front().unwrap();
    let refreshed = *cache
        .chunks
        .iter()
        .find(|(_, entry)| !entry.queued)
        .unwrap()
        .0;
    for pos in [pending, refreshed] {
        state
            .world
            .set_block(pos.x * 16, 64, 0, state.registry.states.air);
    }
    state.world.fold_overlay();
    state.world.swap_poi_revisions(&mut cache.revised_chunks);
    cache.refresh(&state);
    assert!(
        cache.chunks[&refreshed].queued,
        "a fresh change must not jump the FIFO"
    );
    assert!(
        cache.chunks[&refreshed].pois.is_empty(),
        "stale job must not be offered"
    );
    for _ in 0..count.div_ceil(POI_REFRESH_CHUNKS_PER_TICK) {
        state.world.swap_poi_revisions(&mut cache.revised_chunks);
        cache.refresh(&state);
    }
    assert!(cache.pending_chunks.is_empty());
    for (pos, entry) in &cache.chunks {
        assert!(!entry.queued);
        assert_eq!(entry.revision, state.world.chunk_revision(pos.x, pos.z));
        assert_eq!(
            entry.pois.len(),
            usize::from(![pending, refreshed].contains(pos))
        );
    }
}

#[test]
fn poi_claim_index_releases_dead_and_converting_owners_and_tracks_same_tick_claims() {
    let mut state = test_state();
    let first = villager(&mut state, [7.5, 201.0, 8.5]);
    let second = villager(&mut state, [9.5, 201.0, 8.5]);
    let owner = state.entities.body(first).id;
    let other = state.entities.body(second).id;
    let home = bed(&mut state, 8, 8);
    state
        .entities
        .row_mut(first)
        .villager
        .as_mut()
        .unwrap()
        .home = Some(home);
    let poi = Poi {
        pos: home,
        kind: PoiKind::Home,
    };
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    assert!(!cache.claimed(owner, poi));
    assert!(cache.claimed(other, poi));
    state
        .entities
        .row_mut(first)
        .villager
        .as_mut()
        .unwrap()
        .conversion_time = Some(-1);
    cache.refresh(&state);
    assert!(!cache.claimed(other, poi));
    state
        .entities
        .row_mut(first)
        .villager
        .as_mut()
        .unwrap()
        .conversion_time = None;
    state.entities.body_mut(first).alive = false;
    cache.refresh(&state);
    assert!(!cache.claimed(other, poi));
    let mut v = VillagerState {
        home: Some(home),
        ..Default::default()
    };
    cache.update_claims(other, [None; 3], [v.home, v.job_site, v.meeting_point]);
    assert!(cache.claimed(owner, poi));
    v.home = None;
    cache.update_claims(
        other,
        [Some(home), None, None],
        [v.home, v.job_site, v.meeting_point],
    );
    assert!(!cache.claimed(owner, poi));
}

#[test]
fn poi_cache_tracks_changed_chunks_and_searches_only_local_candidates() {
    let mut state = test_state();
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(8, 201, 8, composter);
    state.world.fold_overlay();
    let mut cache = PoiCache::default();
    state.world.swap_poi_revisions(&mut cache.revised_chunks);
    cache.refresh(&state);
    cache.nearby([8.5, 201.0, 8.5], EntityId(-1), |kind| {
        matches!(kind, PoiKind::Job(_))
    });
    assert_eq!(cache.candidates.len(), 1);
    cache.nearby([1000.5, 201.0, 8.5], EntityId(-1), |_| true);
    assert!(cache.candidates.is_empty());
    state.world.set_block(8, 201, 8, state.registry.states.air);
    state.world.fold_overlay();
    state.world.swap_poi_revisions(&mut cache.revised_chunks);
    cache.refresh(&state);
    cache.nearby([8.5, 201.0, 8.5], EntityId(-1), |_| true);
    assert!(cache.candidates.is_empty());
}

#[test]
fn missing_pois_are_not_searched_every_twenty_ticks() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let id = state.entities.body(slot).id;
    for _ in 0..20 {
        step(&mut state);
    }
    let due = state.villager_pois.search_at[&id];
    for _ in 0..40 {
        step(&mut state);
    }
    assert_eq!(state.villager_pois.search_at[&id], due);
    assert!(due >= 200);
}

#[test]
fn acquisition_due_queue_retries_budget_denials_without_cooldown_or_slot_starvation() {
    let mut state = test_state();
    state.day_time = 2000;
    for _ in 0..70 {
        let slot = villager(&mut state, [8.5, 201.0, 8.5]);
        state
            .entities
            .row_mut(slot)
            .villager
            .as_mut()
            .unwrap()
            .data
            .profession = Profession::Nitwit;
    }
    let slot = villager(&mut state, [2.5, 201.0, 8.5]);
    let id = state.entities.body(slot).id;
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(10, 201, 8, composter);
    state.world.fold_overlay();
    state.tick = 20;
    let mut cache = std::mem::take(&mut state.villager_pois);
    cache.refresh(&state);
    cache
        .search_at
        .values_mut()
        .for_each(|due| *due = state.tick);
    state.villager_pois = cache;
    state.tick_budget.reset(state.tick);
    // Exhaust the shared decision allowance: a denied villager keeps its
    // original retry deadline instead of advancing it.
    state
        .tick_budget
        .exhaust_for_test(crate::tick_budget::Kind::Decision);
    let mut changes = ChangeSet::default();
    crate::settlements::allocator::tick(&mut state, &mut changes);
    let cache = &state.villager_pois;
    assert_eq!(
        cache
            .search_at
            .values()
            .filter(|&&due| due > state.tick)
            .count(),
        64
    );
    let initial_deadline = cache.search_at[&id];
    crate::settlements::allocator::tick(&mut state, &mut changes);
    assert_eq!(
        state.villager_pois.search_at[&id], initial_deadline,
        "budget denial must not advance the retry deadline"
    );
    assert!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .job_site
            .is_none()
    );
    // A fresh global tick refills the shared allowance; the claim lands.
    state.tick += 1;
    state.tick_budget.reset(state.tick);
    crate::settlements::allocator::tick(&mut state, &mut changes);
    assert_eq!(
        state.entities.row(slot).villager.as_ref().unwrap().job_site,
        Some(BlockPos {
            x: 10,
            y: 201,
            z: 8
        })
    );
    assert!(state.villager_pois.search_at[&id] > initial_deadline);
}

#[test]
fn empty_allocator_search_updates_brain_deadline_without_marking_a_row() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let id = state.entities.body(slot).id;
    state.tick = 20;
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    cache.search_at.insert(id, state.tick);
    state.villager_pois = cache;
    state.entities.take_touched();
    crate::settlements::allocator::tick(&mut state, &mut ChangeSet::default());
    let cache = &state.villager_pois;
    assert!(cache.search_at[&id] > state.tick);
    assert_eq!(
        cache.resident_data[cache.resident_index[&id]].search_due, cache.search_at[&id],
        "the single refresh must still publish cache-only retry changes to the brain"
    );
    assert!(state.entities.take_touched().is_empty());
}

#[test]
fn full_brain_budget_caps_decisions_but_advances_every_resident_schedule() {
    let mut state = test_state();
    state.tick = 1;
    state.day_time = 9_000;
    for _ in 0..DECISIONS_PER_TICK + 17 {
        let slot = villager(&mut state, [8.5, 201.0, 8.5]);
        state.entities.row_mut(slot).villager.as_mut().unwrap().age = 100;
    }
    step(&mut state);
    assert_eq!(
        state
            .villager_pois
            .full_brains
            .iter()
            .filter(|&&full| full)
            .count(),
        DECISIONS_PER_TICK
    );
    for slot in state.entities.slots() {
        let v = state.entities.row(slot).villager.as_ref().unwrap();
        assert_eq!(v.age, 99, "budget denial must not pause aging");
        assert_eq!(
            v.activity,
            if state.villager_pois.admitted(slot) {
                Activity::Meet
            } else {
                Activity::Idle
            }
        );
        if !state.villager_pois.admitted(slot) {
            assert!(v.navigation_path.is_empty());
            assert!(v.navigation_target.is_none());
            assert_eq!(state.entities.pos(slot), [8.5, 201.0, 8.5]);
        }
    }
}

#[test]
fn full_brain_admission_spends_only_the_global_remaining_allowance() {
    let mut state = test_state();
    for _ in 0..DECISIONS_PER_TICK + 17 {
        villager(&mut state, [8.5, 201.0, 8.5]);
    }
    begin_brain_tick(&mut state);
    assert!(state.tick_budget.try_charge(
        state.dimension,
        Consumer::Brain,
        Kind::Decision,
        DECISIONS_PER_TICK - 3
    ));
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    cache.prepare_brains(&mut state);
    assert_eq!(cache.selected.len(), 3);
    assert_eq!(state.tick_budget.remaining(Kind::Decision), 0);
    // Re-entering this dimension does not replenish a globally spent tick.
    cache.prepare_brains(&mut state);
    assert!(cache.selected.is_empty());
    assert_eq!(state.tick_budget.remaining(Kind::Decision), 0);
    state.tick += 1;
    begin_brain_tick(&mut state);
    cache.prepare_brains(&mut state);
    assert_eq!(cache.selected.len(), DECISIONS_PER_TICK);
    assert_eq!(state.tick_budget.remaining(Kind::Decision), 0);
}

#[test]
fn newly_populated_nether_brains_receive_actual_service_after_overworld_saturation() {
    use crate::dimensions::Dimension;
    let mut overworld = test_state();
    for _ in 0..DECISIONS_PER_TICK * 2 {
        let slot = villager(&mut overworld, [8.5, 201.0, 8.5]);
        let v = overworld.entities.row_mut(slot).villager.as_mut().unwrap();
        v.activity = Activity::Panic;
        v.panic_until = u64::MAX;
    }
    let mut nether = test_state();
    nether.dimension = Dimension::Nether;
    for _ in 0..8 {
        let slot = villager(&mut nether, [8.5, 201.0, 8.5]);
        let v = nether.entities.row_mut(slot).villager.as_mut().unwrap();
        v.activity = Activity::Panic;
        v.panic_until = u64::MAX;
    }
    for current in 0..14 {
        overworld.tick = current;
        overworld.tick_budget.reset(current);
        for dimension in Dimension::ALL {
            overworld.tick_budget.finish(dimension, Consumer::Allocator);
            if dimension == Dimension::End || (dimension == Dimension::Nether && current < 10) {
                overworld.tick_budget.finish(dimension, Consumer::Brain);
            }
        }
        crate::settlements::allocator::tick(&mut overworld, &mut ChangeSet::default());
        tick(&mut overworld, &mut ChangeSet::default());
        assert!(!overworld.villager_pois.selected.is_empty());
        if current >= 10 {
            nether.tick = current;
            std::mem::swap(&mut overworld.tick_budget, &mut nether.tick_budget);
            crate::settlements::allocator::tick(&mut nether, &mut ChangeSet::default());
            tick(&mut nether, &mut ChangeSet::default());
            if current >= 11 {
                assert!(
                    !nether.villager_pois.selected.is_empty(),
                    "real ready backlog must receive a share by the following tick"
                );
            }
            std::mem::swap(&mut overworld.tick_budget, &mut nether.tick_budget);
        }
    }
}

#[test]
fn full_brain_rotation_is_id_fair_with_bounded_near_and_far_waits() {
    let mut state = test_state();
    state.tick = 1;
    let population = DECISIONS_PER_TICK * 2 + 17;
    for _ in 0..population {
        villager(&mut state, [8.5, 201.0, 8.5]);
    }
    // Force dense-slot compaction: rotation must follow ids, not a slot stride.
    let removed = state.entities.body(EntitySlot(7)).id;
    state.entities.remove(EntitySlot(7));
    let replacement = villager(&mut state, [8.5, 201.0, 8.5]);
    assert_ne!(state.entities.body(replacement).id, removed);
    let mut cache = PoiCache::default();
    let rounds = population.div_ceil(DECISIONS_PER_TICK) as u64;
    for period in [1, 32] {
        for brain in &mut cache.resident_data {
            brain.cadence = BrainCadence::default();
        }
        let mut last = FxHashMap::default();
        for index in 0..state.entities.len() {
            let slot = EntitySlot(index as u32);
            state
                .entities
                .row_mut(slot)
                .villager
                .as_mut()
                .unwrap()
                .panic_until = if period == 1 { u64::MAX } else { 0 };
            last.insert(state.entities.body(slot).id, state.tick);
        }
        let bound = period + rounds;
        let mut seen = FxHashSet::default();
        for _ in 0..bound * 3 {
            cache.refresh(&state);
            begin_brain_tick(&mut state);
            cache.prepare_brains(&mut state);
            for &slot in &cache.selected {
                let id = state.entities.body(slot).id;
                assert!(cache.admitted(slot));
                assert!(state.tick - last[&id] <= bound);
                last.insert(id, state.tick);
                seen.insert(id);
            }
            assert!(last.values().all(|&tick| state.tick - tick <= bound));
            state.tick += 1;
        }
        assert_eq!(seen.len(), population);
    }
}

#[test]
fn full_brain_visible_waits_at_mix_density_and_interactions_preempt_regular_admission() {
    use crate::boundary::{ConnectionId, PlayerInput};

    let registry = Arc::new(crate::tick_tests::full_registry());
    for population in [10_000usize, 50_000] {
        let world = crate::horde_bench::flat_world(&registry);
        let mut state = TickState::new(Arc::clone(&registry), world, 2, 512);
        state.gamerules.insert("spawn_mobs".into(), "false".into());
        let joins: Vec<_> = (1..=100)
            .map(|id| PlayerInput::Join {
                connection: ConnectionId(id),
                uuid: uuid::Uuid::from_u128(u128::from(id)),
                name: format!("AdmissionBench{id}"),
                permission_level: 0,
            })
            .collect();
        crate::tick::run_tick(&mut state, &joins);
        state.day_time = 2000;
        let mut want = FxHashMap::default();
        let mut cache = PoiCache::default();
        for i in 0..population {
            // Same 25 quarters and 20x20 four-block grid as MIX-01 above.
            // The 50k rung places five residents at each 10k fixture position.
            let quarter = (i / (population / 25)) as i32;
            let cell = (i % 400) as i32;
            let x = 384 + (quarter % 5) * 128 + (cell % 20) * 4;
            let z = 384 + (quarter / 5) * 128 + (cell / 20) * 4;
            let slot = villager(&mut state, [f64::from(x) + 1.5, 64.0, f64::from(z) + 1.5]);
            let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
            v.activity = schedule(v.age, v.data.profession, state.day_time);
            cache
                .search_at
                .insert(state.entities.body(slot).id, u64::MAX);
            want.insert(
                ChunkPos {
                    x: x / 16,
                    z: z / 16,
                },
                0,
            );
        }
        state.world.ensure_blocking(&want);
        activate_fixture_chunks(&mut state, want.keys().copied());
        for (layout, offset) in [("mix100", 0), ("far32", 2048)] {
            for quarter in 0..25 {
                for player in 0..4 {
                    let id = state
                        .connections
                        .entity_of(ConnectionId(quarter * 4 + player + 1))
                        .unwrap();
                    let slot = state.entities.slot_of(id).unwrap();
                    state.entities.set_pos(
                        slot,
                        [
                            f64::from(384 + (quarter as i32 % 5) * 128 + offset) + 88.5,
                            64.0,
                            f64::from(384 + (quarter as i32 / 5) * 128 + offset) + 56.5,
                        ],
                    );
                }
            }
            cache.changed.clear();
            state.entities.swap_brain_changes(&mut cache.changed);
            cache.refresh(&state);
            for brain in &mut cache.resident_data {
                brain.cadence = BrainCadence::default();
                brain.wake = false;
            }
            let start = state.tick;
            let mut last: Vec<Option<u64>> = vec![None; state.entities.len()];
            let mut near = vec![false; state.entities.len()];
            let mut counts = [0; 2];
            let mut max_gap = [0; 2];
            let mut max_first = [0; 2];
            let expected_near = if offset == 0 {
                population * 128 / 400
            } else {
                0
            };
            let near_share = expected_near.div_ceil(20).min(DECISIONS_PER_TICK * 3 / 4);
            let outer_share = DECISIONS_PER_TICK - near_share;
            let near_bound = expected_near.div_ceil(near_share.max(1)) as u64 + 1;
            let outer_bound = 64 + population.div_ceil(outer_share) as u64;
            for iteration in 0..outer_bound * 4 {
                cache.changed.clear();
                state.entities.swap_brain_changes(&mut cache.changed);
                cache.refresh(&state);
                begin_brain_tick(&mut state);
                cache.prepare_brains(&mut state);
                assert!(cache.selected.len() <= DECISIONS_PER_TICK);
                if iteration == 0 {
                    for (index, &slot) in cache.residents.iter().enumerate() {
                        let chunk = cache.resident_data[index].chunk;
                        near[slot.0 as usize] = cache.player_distance[&(chunk.x, chunk.z)] <= 2;
                        counts[usize::from(!near[slot.0 as usize])] += 1;
                    }
                }
                for &slot in &cache.selected {
                    let index = slot.0 as usize;
                    let group = usize::from(!near[index]);
                    if let Some(previous) = last[index] {
                        max_gap[group] = max_gap[group].max(state.tick - previous);
                    } else {
                        max_first[group] = max_first[group].max(state.tick - start + 1);
                    }
                    last[index] = Some(state.tick);
                }
                for &slot in &cache.residents {
                    let index = slot.0 as usize;
                    let bound = if near[index] { near_bound } else { outer_bound };
                    assert!(state.tick - last[index].unwrap_or(start) <= bound);
                }
                cache.motion_revision = Some(state.world.revision_now());
                state.tick += 1;
            }
            assert!(
                cache
                    .residents
                    .iter()
                    .all(|slot| last[slot.0 as usize].is_some())
            );
            assert_eq!(
                counts[0],
                if offset == 0 {
                    population * 128 / 400
                } else {
                    0
                }
            );
            let near_target_applicable = counts[0].div_ceil(20) <= DECISIONS_PER_TICK * 3 / 4;
            if near_target_applicable {
                assert!(max_gap[0] <= 20);
                assert!(max_first[0] <= 20, "include first-service queueing");
            } else {
                // Overload degrades cadence, never the hard tick capacity.
                assert!(max_gap[0] > 20);
            }
            let outer_target = 32;
            let outer_target_applicable = counts[1].div_ceil(outer_share) < outer_target;
            if outer_target_applicable {
                assert!(
                    max_gap[1] <= outer_target as u64,
                    "far heartbeat includes queueing"
                );
                assert!(
                    max_first[1] <= outer_target as u64,
                    "include first-service queueing"
                );
            }
            println!(
                "villager_brain_wait count={population} layout={layout} players=100 near_share={near_share} outer_share={outer_share} near_target_applicable={near_target_applicable} outer_target={outer_target} outer_target_applicable={outer_target_applicable} near_count={} outer_count={} near_max_gap={} outer_max_gap={} near_max_first={} outer_max_first={} near_20_tick_target_met={}",
                counts[0],
                counts[1],
                max_gap[0],
                max_gap[1],
                max_first[0],
                max_first[1],
                counts[0] == 0 || max_gap[0] <= 20,
            );
        }
        // Four new interaction events must bypass the already-due regular
        // population, not merely move to its ordinary distance-one queue.
        let actors: Vec<_> = cache.residents.iter().rev().take(4).copied().collect();
        let merchant = state.entities.body(actors[0]).id;
        let mut menu = crate::inventory::Menu::new(1, crate::inventory::MenuKind::Merchant);
        menu.merchant = Some(crate::inventory::MerchantMenu {
            entity: merchant,
            selected: 0,
            last_restock: 0,
            reputation: 0,
            hero: None,
        });
        state.players.values_mut().next().unwrap().open = Some(menu);
        state.entities.body_mut(actors[1]).hurt_at = Some(state.tick);
        state
            .entities
            .row_mut(actors[2])
            .villager
            .as_mut()
            .unwrap()
            .home = Some(BlockPos {
            x: 385,
            y: 64,
            z: 386,
        });
        state
            .entities
            .row_mut(actors[3])
            .villager
            .as_mut()
            .unwrap()
            .job_site = Some(BlockPos {
            x: 384,
            y: 64,
            z: 384,
        });
        cache.changed.clear();
        state.entities.swap_brain_changes(&mut cache.changed);
        cache.refresh(&state);
        begin_brain_tick(&mut state);
        cache.prepare_brains(&mut state);
        assert!(actors.iter().all(|&slot| cache.admitted(slot)));
        assert!(cache.selected.len() <= DECISIONS_PER_TICK);

        // A door edit wakes a nearby resident even before its old sparse
        // deadline. Isolate the edit so this is one event, not a cap-sized burst.
        let isolated = villager(&mut state, [-1015.5, 64.0, -1015.5]);
        cache
            .search_at
            .insert(state.entities.body(isolated).id, u64::MAX);
        let v = state.entities.row_mut(isolated).villager.as_mut().unwrap();
        v.activity = schedule(v.age, v.data.profession, state.day_time);
        let mut extra = FxHashMap::default();
        extra.insert(ChunkPos { x: -64, z: -64 }, 0);
        state.world.ensure_blocking(&extra);
        activate_fixture_chunks(&mut state, extra.keys().copied());
        cache.changed.clear();
        state.entities.swap_brain_changes(&mut cache.changed);
        cache.refresh(&state);
        cache.motion_revision = Some(state.world.revision_now());
        let index = cache.resident_index[&state.entities.body(isolated).id];
        cache.resident_data[index].cadence = BrainCadence {
            period: 32,
            next: state.tick + 100,
            was_busy: false,
        };
        cache.resident_data[index].wake = false;
        state.tick += 1;
        let door = block(&state, "minecraft:oak_door", &[]);
        state.world.set_block(-1014, 64, -1015, door);
        cache.changed.clear();
        state.entities.swap_brain_changes(&mut cache.changed);
        cache.refresh(&state);
        begin_brain_tick(&mut state);
        cache.prepare_brains(&mut state);
        assert!(cache.admitted(isolated));
    }
}

#[test]
fn trade_combat_panic_pursuit_and_committed_paths_never_downgrade() {
    let mut state = test_state();
    state.tick = 100;
    let slots: Vec<_> = (0..6)
        .map(|_| villager(&mut state, [8.5, 201.0, 8.5]))
        .collect();
    for &slot in &slots {
        state
            .entities
            .row_mut(slot)
            .villager
            .as_mut()
            .unwrap()
            .activity = Activity::Rest;
    }
    let merchant = state.entities.body(slots[0]).id;
    let mut player = crate::player::PlayerState::new(
        crate::boundary::ConnectionId(42),
        crate::commands::GameMode::Survival,
    );
    let mut menu = crate::inventory::Menu::new(1, crate::inventory::MenuKind::Merchant);
    menu.merchant = Some(crate::inventory::MerchantMenu {
        entity: merchant,
        selected: 0,
        last_restock: 0,
        reputation: 0,
        hero: None,
    });
    player.open = Some(menu);
    state.players.insert(EntityId(99), player);
    state.entities.body_mut(slots[1]).hurt_at = Some(state.tick);
    state
        .entities
        .row_mut(slots[2])
        .villager
        .as_mut()
        .unwrap()
        .panic_until = 200;
    state
        .entities
        .row_mut(slots[3])
        .villager
        .as_mut()
        .unwrap()
        .partner = Some(merchant);
    state
        .entities
        .row_mut(slots[4])
        .villager
        .as_mut()
        .unwrap()
        .navigation_path = vec![BlockPos { x: 9, y: 201, z: 8 }];
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    for &slot in &slots {
        let index = cache.resident_index[&state.entities.body(slot).id];
        cache.resident_data[index].cadence = BrainCadence {
            period: 32,
            next: 132,
            was_busy: false,
        };
    }
    begin_brain_tick(&mut state);
    cache.prepare_brains(&mut state);
    for &slot in &slots[..5] {
        let index = cache.resident_index[&state.entities.body(slot).id];
        assert_eq!(cache.resident_data[index].cadence.period, 1);
        assert!(
            cache.admitted(slot),
            "reactive promotion must not wait for the old deadline"
        );
    }
    let index = cache.resident_index[&state.entities.body(slots[5]).id];
    assert_eq!(cache.resident_data[index].cadence.period, 32);
    assert!(!cache.admitted(slots[5]));
    assert_eq!(brain_period(1, 3), 1);
    assert_eq!(brain_period(1, 4), 2);
    assert_eq!(brain_period(2, 3), 2);
    assert_eq!(brain_period(2, 2), 1);
    assert_eq!(brain_period(16, 9), 16);
    assert_eq!(brain_period(16, 10), 32);
    assert_eq!(brain_period(32, 9), 32);
    assert_eq!(brain_period(32, 8), 16);
}

#[test]
fn route_chunk_revisions_ignore_unrelated_admission_and_edits() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let target = BlockPos {
        x: 11,
        y: 201,
        z: 8,
    };
    let mut v = VillagerState::default();
    let mut cache = PoiCache::default();
    move_to(&mut state, &mut cache, slot, &mut v, Some(target), false);
    let deadline = v.navigation_repath_at;
    let revisions = v.navigation_chunks.clone();
    assert!(!route_changed(&state, &v));
    state
        .world
        .ensure_blocking(&FxHashMap::from_iter([(ChunkPos { x: 8, z: 8 }, 0)]));
    state
        .world
        .set_block(128, 201, 128, state.registry.states.stone);
    assert!(!route_changed(&state, &v), "unrelated pending edits");
    state.world.fold_overlay();
    assert!(!route_changed(&state, &v), "unrelated committed edits");
    state.tick += 1;
    set_node_allowance(&mut state, 0);
    move_to(&mut state, &mut cache, slot, &mut v, Some(target), false);
    assert_eq!(v.navigation_chunks, revisions);
    assert_eq!(v.navigation_repath_at, deadline);
    assert!(!v.navigation_path.is_empty());
}

#[test]
fn route_chunk_revisions_reject_pending_support_edits_without_renewing_deadline() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let target = BlockPos {
        x: 11,
        y: 201,
        z: 8,
    };
    let mut v = VillagerState::default();
    let mut cache = PoiCache::default();
    move_to(&mut state, &mut cache, slot, &mut v, Some(target), false);
    let deadline = v.navigation_repath_at;
    state.world.set_block(9, 200, 8, state.registry.states.air);
    assert!(route_changed(&state, &v));
    state.tick += 1;
    move_to(&mut state, &mut cache, slot, &mut v, Some(target), false);
    assert_eq!(v.navigation_repath_at, deadline);
    state.world.fold_overlay();
    assert!(route_changed(&state, &v));
    move_to(&mut state, &mut cache, slot, &mut v, Some(target), false);
    assert!(!route_changed(&state, &v));
    assert_eq!(v.navigation_repath_at, deadline);
}

#[test]
fn committed_and_skipped_motion_resolve_next_geometry_after_a_route_edit() {
    for skipped in [false, true] {
        for committed_edit in [false, true] {
            let mut state = test_state();
            let slot = villager(&mut state, [8.5, 201.0, 8.5]);
            let target = BlockPos {
                x: 11,
                y: 201,
                z: 8,
            };
            let mut v = VillagerState::default();
            let mut cache = PoiCache::default();
            // Warm the route and its next standing point through normal motion.
            move_to(&mut state, &mut cache, slot, &mut v, Some(target), false);
            let next = *v.navigation_path.last().unwrap();
            assert!(standing(&state.world, &state.registry, next, 0.3, 1.95).is_some());
            state
                .world
                .set_block(next.x, next.y - 1, next.z, state.registry.states.air);
            if committed_edit {
                state.world.fold_overlay();
            }
            assert!(route_changed(&state, &v));
            assert!(standing(&state.world, &state.registry, next, 0.3, 1.95).is_none());
            let before = state.entities.pos(slot);
            if skipped {
                state.entities.row_mut(slot).villager = Some(Box::new(v));
                move_skipped(&mut state, slot, true, false, Activity::Idle);
            } else {
                move_committed(&mut state, slot, &mut v, Activity::Idle);
            }
            let after = state.entities.pos(slot);
            assert_eq!(
                [after[0], after[2]],
                [before[0], before[2]],
                "route support edit: skipped={skipped}, committed={committed_edit}"
            );
        }
    }
}

#[test]
fn route_chunk_revisions_cover_boundary_halo_and_unloaded_target() {
    let state = test_state();
    let mut v = VillagerState {
        navigation_target: Some(BlockPos {
            x: 15,
            y: 201,
            z: 8,
        }),
        ..VillagerState::default()
    };
    capture_route(&state, &mut v, [8.5, 201.0, 8.5], true);
    assert!(
        v.navigation_chunks
            .iter()
            .any(|&(chunk, _)| chunk == ChunkPos { x: 1, z: 0 })
    );
    assert!(route_changed(&state, &v), "unknown halo fails closed");
}

#[test]
fn standing_memo_preserves_paths_budgets_and_resets_between_searches() {
    fn compare(state: &TickState, memo: &mut PoiCache, width: f64, height: f64) {
        for exact in [false, true] {
            for nodes in [1, 12, 512, crate::tick_budget::PATH_NODES_PER_TICK] {
                let mut plain = PoiCache {
                    disable_standing_memo: true,
                    ..PoiCache::default()
                };
                let search = |cache: &mut PoiCache, path: &mut Vec<BlockPos>| {
                    let mut budget = crate::tick_budget::Budget::default();
                    for dimension in crate::dimensions::Dimension::ALL {
                        for consumer in [Consumer::Allocator, Consumer::Brain] {
                            budget.finish(dimension, consumer);
                        }
                    }
                    assert!(budget.try_charge(
                        state.dimension,
                        Consumer::Brain,
                        Kind::PathNode,
                        crate::tick_budget::PATH_NODES_PER_TICK - nodes
                    ));
                    let mut ctx = allocator::RouteCtx {
                        world: &state.world,
                        registry: &state.registry,
                        dimension: state.dimension,
                        consumer: Consumer::Allocator,
                        budget: &mut budget,
                    };
                    let found = allocator::find_path_ctx(
                        &mut ctx,
                        cache,
                        [3.5, 201.0, 8.5],
                        BlockPos {
                            x: 11,
                            y: 201,
                            z: 8,
                        },
                        width,
                        height,
                        path,
                        exact,
                        48,
                        16,
                        true,
                    );
                    (found, budget.remaining(Kind::PathNode))
                };
                let mut cached_path = Vec::new();
                let mut plain_path = Vec::new();
                assert_eq!(
                    search(memo, &mut cached_path),
                    search(&mut plain, &mut plain_path)
                );
                assert_eq!(cached_path, plain_path);
                assert_eq!(memo.nodes.len(), plain.nodes.len());
                assert!(plain.standing_cache.is_empty());
            }
        }
    }
    let mut state = test_state();
    let mut memo = PoiCache::default();
    let slab = block(&state, "minecraft:stone_slab", &[("type", "bottom")]);
    let stairs = block(&state, "minecraft:stone_stairs", &[("facing", "east")]);
    state.world.set_block(5, 200, 8, slab);
    state.world.set_block(6, 200, 8, stairs);
    state
        .world
        .set_block(7, 201, 8, state.registry.states.stone);
    compare(&state, &mut memo, 0.3, 1.95);
    let old_capacity = memo.standing_cache.capacity();
    assert!(old_capacity > 0);
    // Edit after the previous search, then alternate baby/adult dimensions.
    // The low ceiling admits a baby but blocks an adult at the edited cell.
    state.world.set_block(7, 201, 8, state.registry.states.air);
    state
        .world
        .set_block(7, 202, 8, state.registry.states.stone);
    let cell = BlockPos { x: 7, y: 201, z: 8 };
    assert!(standing(&state.world, &state.registry, cell, 0.15, 0.975).is_some());
    assert!(standing(&state.world, &state.registry, cell, 0.3, 1.95).is_none());
    compare(&state, &mut memo, 0.15, 0.975);
    compare(&state, &mut memo, 0.3, 1.95);
    assert!(memo.standing_cache.capacity() >= old_capacity);
    // An impenetrable loaded wall exercises unsuccessful/closest-path returns.
    for z in 0..16 {
        for y in 201..206 {
            state.world.set_block(8, y, z, state.registry.states.stone);
        }
    }
    compare(&state, &mut memo, 0.3, 1.95);
}

#[test]
fn short_wander_ignores_astar_budget_and_blocked_stroll_retries_after_forty_ticks() {
    let mut state = test_state();
    let start = [5.5, 201.0, 8.5];
    let slot = villager(&mut state, start);
    set_node_allowance(&mut state, 0);
    let target = BlockPos {
        x: 11,
        y: 201,
        z: 8,
    };
    let mut v = VillagerState::default();
    wander_to(&mut state, slot, &mut v, Some(target), Activity::Idle);
    assert_eq!(v.navigation_path.len(), 6);
    assert_eq!(v.navigation_path.first(), Some(&target));
    assert_eq!(
        v.navigation_path.last(),
        Some(&BlockPos { x: 6, y: 201, z: 8 })
    );
    assert!(state.entities.pos(slot)[0] > start[0]);
    assert_eq!(state.tick_budget.remaining(Kind::PathNode), 0);
    assert!(state.villager_pois.nodes.is_empty());
    assert_eq!(v.navigation_repath_at, 40);

    state.entities.set_pos(slot, start);
    state.entities.set_vel(slot, [0.0; 3]);
    for y in 201..205 {
        state.world.set_block(8, y, 8, state.registry.states.stone);
    }
    v = VillagerState::default();
    wander_to(&mut state, slot, &mut v, Some(target), Activity::Idle);
    assert!(
        v.navigation_path.is_empty(),
        "discard even the unblocked prefix"
    );
    assert_eq!(state.entities.pos(slot), start);
    assert_eq!(v.navigation_target, Some(target));
    assert_eq!(v.navigation_repath_at, 40);
    state.tick = 1;
    wander_to(&mut state, slot, &mut v, Some(target), Activity::Idle);
    assert!(v.navigation_path.is_empty());
    assert_eq!(state.entities.pos(slot), start);
    assert_eq!(
        v.navigation_repath_at, 40,
        "do not renew the retry every tick"
    );
}

#[test]
fn villager_walks_onto_one_block_rise_and_settles_at_target() {
    let mut state = test_state();
    for x in 8..12 {
        for z in 1..15 {
            state
                .world
                .set_block(x, 201, z, state.registry.states.stone);
        }
    }
    let slot = villager(&mut state, [5.5, 201.0, 8.5]);
    state.entities.set_ground(slot, true);
    let target = BlockPos {
        x: 10,
        y: 202,
        z: 8,
    };
    let mut v = VillagerState::default();
    for tick in 0..100 {
        state.tick = tick;
        wander_to(&mut state, slot, &mut v, Some(target), Activity::Idle);
    }
    let pos = state.entities.pos(slot);
    assert!(pos[0] > 10.0, "must cross the ledge, not bump it: {pos:?}");
    assert!((pos[1] - 202.0).abs() < 1.0e-7, "{pos:?}");
    assert!(state.entities.ground(slot), "must land on the raised floor");
    assert!(v.navigation_path.is_empty(), "must finish the route");
}

#[test]
fn canonical_ticks_finish_work_approach_through_a_raised_door() {
    let mut state = test_state();
    state.day_time = 3000;
    for x in 8..12 {
        for z in 1..15 {
            state
                .world
                .set_block(x, 201, z, state.registry.states.stone);
        }
    }
    for z in 0..16 {
        for y in 202..=204 {
            state.world.set_block(8, y, z, state.registry.states.stone);
        }
    }
    for (y, half) in [(202, "lower"), (203, "upper")] {
        let door = block(
            &state,
            "minecraft:oak_door",
            &[("half", half), ("facing", "west"), ("open", "false")],
        );
        state.world.set_block(8, y, 8, door);
    }
    let stairs = block(&state, "minecraft:oak_stairs", &[("facing", "east")]);
    state.world.set_block(7, 201, 8, stairs);
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(10, 202, 8, composter);
    let slot = villager(&mut state, [5.5, 201.0, 8.5]);
    for _ in 0..400 {
        crate::tick::run_tick(&mut state, &[]);
        let pos = state.entities.pos(slot);
        if pos[0] > 8.0 && pos[1] >= 202.0 {
            break;
        }
    }
    let pos = state.entities.pos(slot);
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    assert!(
        pos[0] > 8.0 && pos[1] >= 202.0,
        "native tick must finish its valid workstation approach: pos={pos:?}"
    );
    assert_eq!(v.data.profession, Profession::Farmer);
}

#[test]
fn short_wander_matches_astar_step_drop_and_shaped_clearance() {
    let mut state = test_state();
    let start = [7.5, 201.0, 8.5];
    let slot = villager(&mut state, start);
    let slab = block(&state, "minecraft:stone_slab", &[("type", "bottom")]);
    for (dy, support) in [
        (0, state.registry.states.stone),
        (1, state.registry.states.stone),
        (-1, state.registry.states.stone),
        (-2, state.registry.states.stone),
        (-3, state.registry.states.stone),
        (1, slab),
    ] {
        for y in 196..206 {
            state.world.set_block(8, y, 8, state.registry.states.air);
        }
        state.world.set_block(8, 200 + dy, 8, support);
        state.entities.set_pos(slot, start);
        state.entities.set_vel(slot, [0.0; 3]);
        let target = BlockPos {
            x: 8,
            y: 201 + dy,
            z: 8,
        };
        let mut cache = PoiCache::default();
        let mut path = Vec::new();
        assert!(find_path_exact(
            &mut state, &mut cache, start, target, 0.3, 1.95, &mut path
        ));
        assert_eq!(path, vec![target]);
        let mut v = VillagerState::default();
        wander_to(&mut state, slot, &mut v, Some(target), Activity::Idle);
        assert_eq!(v.navigation_path, path, "dy={dy}, support={support}");
    }
    // Jump headroom: a destination may be standable while the starting
    // column cannot be raised far enough to cross its edge.
    state
        .world
        .set_block(8, 201, 8, state.registry.states.stone);
    state
        .world
        .set_block(7, 203, 8, state.registry.states.stone);
    state.entities.set_pos(slot, start);
    state.entities.set_vel(slot, [0.0; 3]);
    let mut v = VillagerState::default();
    wander_to(
        &mut state,
        slot,
        &mut v,
        Some(BlockPos { x: 8, y: 202, z: 8 }),
        Activity::Idle,
    );
    assert!(v.navigation_path.is_empty());
    assert_eq!(state.entities.pos(slot), start);
    // Missing destination chunk is an obstacle, never an invitation to load.
    state.entities.set_pos(slot, [15.5, 201.0, 8.5]);
    state.entities.set_vel(slot, [0.0; 3]);
    v = VillagerState::default();
    wander_to(
        &mut state,
        slot,
        &mut v,
        Some(BlockPos {
            x: 16,
            y: 201,
            z: 8,
        }),
        Activity::Idle,
    );
    assert!(v.navigation_path.is_empty());
    assert_eq!(state.entities.pos(slot)[0], 15.5);
}

#[test]
fn skipped_brain_stops_invalid_route_without_dirtying_row_or_renewing_target() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let target = BlockPos {
        x: 11,
        y: 201,
        z: 8,
    };
    let mut v = VillagerState::default();
    let mut cache = PoiCache::default();
    move_to(&mut state, &mut cache, slot, &mut v, Some(target), false);
    assert!(!v.navigation_path.is_empty());
    let deadline = v.navigation_repath_at;
    state.entities.row_mut(slot).villager = Some(Box::new(v));
    let mut changed = FxHashSet::default();
    state.entities.swap_brain_changes(&mut changed);
    state.world.set_block(9, 200, 8, state.registry.states.air);
    move_skipped(&mut state, slot, true, false, Activity::Idle);
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    assert!(
        !v.navigation_path.is_empty(),
        "only full admission replaces the route"
    );
    assert!(!v.navigation_chunks.is_empty());
    assert!(
        state.entities.brain_changes().is_empty(),
        "skipped route execution must not dirty the row"
    );
    assert_eq!(v.navigation_target, Some(target));
    assert_eq!(v.navigation_repath_at, deadline);
}

#[test]
fn skipped_panic_path_consumes_reached_steps_without_cold_row_dirty_marks() {
    let mut state = test_state();
    state.tick = 1;
    state.day_time = 3_000;
    let start = [8.5, 201.0, 8.5];
    let slot = villager(&mut state, start);
    let mut v = VillagerState {
        activity: Activity::Panic,
        panic_until: u64::MAX,
        navigation_path: (9..=11)
            .rev()
            .map(|x| BlockPos { x, y: 201, z: 8 })
            .collect(),
        navigation_target: Some(BlockPos {
            x: 11,
            y: 201,
            z: 8,
        }),
        navigation_repath_at: u64::MAX,
        ..VillagerState::default()
    };
    capture_route(&state, &mut v, start, true);
    state.entities.row_mut(slot).villager = Some(Box::new(v));
    let mut changed = FxHashSet::default();
    state.entities.swap_brain_changes(&mut changed);
    for _ in 0..40 {
        move_skipped(&mut state, slot, false, false, Activity::Work);
        state.tick += 1;
    }
    assert!(
        state.entities.pos(slot)[0] > 10.5,
        "skipped physics must advance past the first cell"
    );
    assert!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .navigation_path
            .is_empty()
    );
    assert!(state.entities.brain_changes().is_empty());
}

fn poi_validation_fixture() -> (TickState, PoiCache, VillagerState) {
    let mut state = test_state();
    let want = FxHashMap::from_iter((0..=4).map(|x| (ChunkPos { x, z: 0 }, 0)));
    state.world.ensure_blocking(&want);
    let home = bed(&mut state, 16, 8);
    let meeting = BlockPos {
        x: 32,
        y: 201,
        z: 8,
    };
    let job = BlockPos {
        x: 48,
        y: 201,
        z: 8,
    };
    let bell = block(&state, "minecraft:bell", &[]);
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(meeting.x, meeting.y, meeting.z, bell);
    state.world.set_block(job.x, job.y, job.z, composter);
    state.world.fold_overlay();
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    let mut v = VillagerState {
        home: Some(home),
        meeting_point: Some(meeting),
        job_site: Some(job),
        ..Default::default()
    };
    v.data.profession = Profession::Farmer;
    validate_pois(&state, &cache, EntityId(-1), &mut v);
    assert_eq!(v.home, Some(home));
    assert_eq!(v.meeting_point, Some(meeting));
    assert_eq!(v.job_site, Some(job));
    assert_eq!(v.poi_validation.geometry_checks, 3);
    (state, cache, v)
}

#[test]
fn poi_validation_reuses_stable_geometry_and_ignores_unrelated_chunks() {
    let (mut state, cache, mut v) = poi_validation_fixture();
    for _ in 0..12 {
        state.tick += 1;
        validate_pois(&state, &cache, EntityId(-1), &mut v);
    }
    assert_eq!(v.poi_validation.geometry_checks, 3);
    state
        .world
        .set_block(79, 201, 8, state.registry.states.stone);
    validate_pois(&state, &cache, EntityId(-1), &mut v);
    assert_eq!(
        v.poi_validation.geometry_checks, 3,
        "unrelated pending edit"
    );
    state.world.fold_overlay();
    validate_pois(&state, &cache, EntityId(-1), &mut v);
    assert_eq!(v.poi_validation.geometry_checks, 3, "unrelated folded edit");
    assert!(v.home.is_some() && v.meeting_point.is_some() && v.job_site.is_some());
}

#[test]
fn poi_validation_invalidates_pending_head_foot_job_and_meeting_edits() {
    let (mut state, cache, mut v) = poi_validation_fixture();
    for pos in [
        v.home.unwrap(),
        BlockPos {
            x: 15,
            y: 201,
            z: 8,
        },
        v.meeting_point.unwrap(),
        v.job_site.unwrap(),
    ] {
        let original = state.world.block_at(pos.x, pos.y, pos.z).unwrap();
        let chunk = ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        };
        let revision = state.world.chunk_revision(chunk.x, chunk.z);
        let mut changed = v.clone();
        let checks = v.poi_validation.geometry_checks;
        state
            .world
            .set_block(pos.x, pos.y, pos.z, state.registry.states.air);
        assert_eq!(state.world.chunk_revision(chunk.x, chunk.z), revision);
        validate_pois(&state, &cache, EntityId(-1), &mut changed);
        assert_eq!(changed.poi_validation.geometry_checks, checks + 1);
        if pos == v.meeting_point.unwrap() {
            assert!(changed.meeting_point.is_none());
            assert_eq!(changed.home, v.home);
            assert_eq!(changed.job_site, v.job_site);
        } else if pos == v.job_site.unwrap() {
            assert!(changed.job_site.is_none());
            assert_eq!(changed.data.profession, Profession::None);
            assert_eq!(changed.home, v.home);
            assert_eq!(changed.meeting_point, v.meeting_point);
        } else {
            assert!(changed.home.is_none());
            assert_eq!(changed.job_site, v.job_site);
            assert_eq!(changed.meeting_point, v.meeting_point);
        }
        state.world.set_block(pos.x, pos.y, pos.z, original);
        state.world.fold_overlay();
        let memories = (v.home, v.meeting_point, v.job_site);
        validate_pois(&state, &cache, EntityId(-1), &mut v);
        assert_eq!((v.home, v.meeting_point, v.job_site), memories);
    }
}

#[test]
fn poi_validation_keeps_claims_profession_dimensions_and_memory_authoritative() {
    let (mut state, mut cache, v) = poi_validation_fixture();
    let mut changed = v.clone();
    changed.data.profession = Profession::Librarian;
    validate_pois(&state, &cache, EntityId(-1), &mut changed);
    assert_eq!(
        changed.job_site, v.job_site,
        "untraded villagers may change job"
    );
    assert_eq!(changed.poi_validation.geometry_checks, 3);
    for trade_kind in 0..3 {
        let mut changed = v.clone();
        changed.data.profession = Profession::Librarian;
        match trade_kind {
            0 => changed.xp = 1,
            1 => changed.data.level = 2,
            _ => {
                super::super::trades::generate(&mut changed, &state.registry);
                changed.offers[0].uses = 1;
            }
        }
        validate_pois(&state, &cache, EntityId(-1), &mut changed);
        assert!(changed.job_site.is_none());
        assert_eq!(changed.data.profession, Profession::Librarian);
        assert_eq!(changed.poi_validation.geometry_checks, 3);
    }
    let mut changed = v.clone();
    changed.meeting_point = Some(BlockPos {
        x: 33,
        y: 201,
        z: 8,
    });
    state.world.set_block(33, 201, 8, state.registry.states.air);
    state.world.fold_overlay();
    validate_pois(&state, &cache, EntityId(-1), &mut changed);
    assert!(
        changed.meeting_point.is_none(),
        "exact coordinates, not chunk alone"
    );
    let mut changed = v.clone();
    changed.home_dimension = "minecraft:the_nether".into();
    validate_pois(&state, &cache, EntityId(-1), &mut changed);
    assert!(changed.home.is_none());
    state.dimension = crate::dimensions::Dimension::Nether;
    let mut changed = v.clone();
    changed.home_dimension = state.dimension.name().into();
    changed.meeting_point_dimension = state.dimension.name().into();
    changed.job_site_dimension = state.dimension.name().into();
    validate_pois(&state, &cache, EntityId(-1), &mut changed);
    assert_eq!(changed.poi_validation.geometry_checks, 6);
    assert_eq!(
        (changed.home, changed.meeting_point, changed.job_site),
        (v.home, v.meeting_point, v.job_site)
    );
    state.dimension = crate::dimensions::Dimension::Overworld;
    cache.update_claims(
        EntityId(-2),
        [None; 3],
        [v.home, v.job_site, v.meeting_point],
    );
    let mut changed = v.clone();
    validate_pois(&state, &cache, EntityId(-1), &mut changed);
    assert!(changed.home.is_none() && changed.job_site.is_none());
    assert_eq!(changed.meeting_point, v.meeting_point);
    // The same geometry can be used by a new claimant, but custody is never
    // inherited from the entity whose transient cache was cloned.
    let mut changed = v.clone();
    validate_pois(&state, &cache, EntityId(-2), &mut changed);
    assert_eq!(changed.home, v.home);
    assert_eq!(changed.job_site, v.job_site);
}

#[test]
fn poi_validation_preserves_unknown_heads_and_bed_feet_then_revalidates_reloads() {
    let (mut state, cache, mut v) = poi_validation_fixture();
    let flushed: Vec<_> = state
        .world
        .chunks()
        .map(|chunk| (chunk.pos, state.world.storage_revision(chunk.pos)))
        .collect();
    state.world.mark_flushed(&flushed);
    let keep = FxHashMap::from_iter((1..=4).map(|x| (ChunkPos { x, z: 0 }, 0)));
    assert!(
        state
            .world
            .evict_outside(&keep)
            .contains(&ChunkPos { x: 0, z: 0 })
    );
    let mut foot_unknown = v.clone();
    validate_pois(&state, &cache, EntityId(-1), &mut foot_unknown);
    assert_eq!(
        foot_unknown.home, v.home,
        "unloaded foot geometry is unknown, not evidence of bed destruction"
    );
    state.world.evict_outside(&Default::default());
    let memories = (v.home, v.meeting_point, v.job_site);
    validate_pois(&state, &cache, EntityId(-1), &mut v);
    assert_eq!((v.home, v.meeting_point, v.job_site), memories);
    assert_eq!(v.poi_validation.geometry_checks, 6);
    validate_pois(&state, &cache, EntityId(-1), &mut v);
    assert_eq!(
        v.poi_validation.geometry_checks, 9,
        "unknown is not a geometry proof"
    );
    let want = FxHashMap::from_iter((0..=4).map(|x| (ChunkPos { x, z: 0 }, 0)));
    state.world.ensure_blocking(&want);
    validate_pois(&state, &cache, EntityId(-1), &mut v);
    assert_eq!((v.home, v.meeting_point, v.job_site), memories);
    assert_eq!(v.poi_validation.geometry_checks, 12);
    validate_pois(&state, &cache, EntityId(-1), &mut v);
    assert_eq!(v.poi_validation.geometry_checks, 12);
}

#[test]
fn poi_validation_runtime_metadata_is_reset_in_journal_and_serde_images() {
    let (state, cache, v) = poi_validation_fixture();
    let image = super::super::persistence::journal_image(&v);
    assert_eq!(image.poi_validation, PoiValidation::default());
    assert!(super::super::persistence::journal_eq(&v, &image));
    let value = serde_json::to_value(&v).unwrap();
    assert!(value.get("poi_validation").is_none());
    assert!(value.get("trade_work").is_none());
    let mut loaded: VillagerState = serde_json::from_value(value).unwrap();
    assert_eq!(loaded.poi_validation, PoiValidation::default());
    validate_pois(&state, &cache, EntityId(-1), &mut loaded);
    assert_eq!(loaded.poi_validation.geometry_checks, 3);
    assert_eq!(
        (loaded.home, loaded.meeting_point, loaded.job_site),
        (v.home, v.meeting_point, v.job_site)
    );
}

#[test]
fn poi_validation_rechecks_folded_relevant_changes_before_forgetting_memory() {
    let (mut state, cache, mut v) = poi_validation_fixture();
    let home = v.home.unwrap();
    state
        .world
        .set_block(home.x, home.y, home.z, state.registry.states.air);
    state.world.fold_overlay();
    validate_pois(&state, &cache, EntityId(-1), &mut v);
    assert!(v.home.is_none());
    assert!(v.meeting_point.is_some() && v.job_site.is_some());
    assert_eq!(v.poi_validation.geometry_checks, 4);
    assert!(v.poi_validation.sites[0].is_none());
}

fn threat_sensor_mob(state: &mut TickState, name: &str, pos: [f64; 3]) -> EntitySlot {
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(name).unwrap())
        .unwrap();
    let id = state.spawn_mob(type_id, pos, 20.0);
    state.entities.slot_of(id).unwrap()
}

fn threat_sensor_full_scan(cache: &PoiCache, pos: [f64; 3]) -> Option<[f64; 3]> {
    cache
        .threats
        .iter()
        .filter_map(|&(at, range)| {
            let d = distance(pos, at);
            (d <= range * range).then_some((d, at))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, at)| at)
}

#[test]
fn threat_sensor_filters_distant_chunks_and_finds_nearest_across_boundaries() {
    let mut state = test_state();
    villager(&mut state, [15.75, 201.0, -0.25]);
    villager(&mut state, [-0.25, 201.0, -16.25]);
    let local = [
        [16.25, 201.0, -0.25],
        [14.75, 201.0, -0.25],
        [0.25, 201.0, -16.25],
        [-1.25, 201.0, -16.25],
    ];
    for pos in local {
        threat_sensor_mob(&mut state, "minecraft:zombie", pos);
    }
    for index in 0..1_024 {
        threat_sensor_mob(
            &mut state,
            "minecraft:pillager",
            [1_024.0 + f64::from(index) * 16.0, 201.0, 1_024.0],
        );
    }
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    assert_eq!(cache.threats.len(), local.len());
    assert_eq!(cache.threat_chunks.len(), local.len());
    assert_eq!(
        nearest_threat(&cache, [15.75, 201.0, -0.25]),
        Some(local[0])
    );
    assert_eq!(
        nearest_threat(&cache, [-0.25, 201.0, -16.25]),
        Some(local[2])
    );
    assert_eq!(nearest_threat(&cache, [128.0, 201.0, 128.0]), None);
    for x in [-16.1, -16.0, -0.1, 0.0, 15.9, 16.0, 16.1] {
        for z in [-16.1, -16.0, -0.1, 0.0, 15.9, 16.0, 16.1] {
            let pos = [x, 201.0, z];
            assert_eq!(
                nearest_threat(&cache, pos),
                threat_sensor_full_scan(&cache, pos)
            );
        }
    }
}

#[test]
fn threat_sensor_preserves_exact_ranges_and_three_dimensional_distance() {
    let mut state = test_state();
    let mut cache = PoiCache::default();
    let origin = [16.0, 201.0, 16.0];
    villager(&mut state, origin);
    for (name, range) in [
        ("zombie", 8.0),
        ("zombie_villager", 8.0),
        ("husk", 8.0),
        ("drowned", 8.0),
        ("vex", 8.0),
        ("vindicator", 10.0),
        ("zoglin", 10.0),
        ("evoker", 12.0),
        ("illusioner", 12.0),
        ("ravager", 12.0),
        ("pillager", 15.0),
    ] {
        assert_eq!(threat_range(name), Some(range));
        let slot = threat_sensor_mob(&mut state, &format!("minecraft:{name}"), origin);
        for (offset, inside) in [
            ([range, 0.0, 0.0], true),
            ([-range, 0.0, 0.0], true),
            ([0.0, range, 0.0], true),
            ([0.0, 0.0, range], true),
            ([range - 0.000_001, 0.0, 0.0], true),
            ([range + 0.000_001, 0.0, 0.0], false),
            ([0.0, range + 0.000_001, 0.0], false),
            ([range * 0.75, 0.0, range * 0.75], false),
        ] {
            let at = [
                origin[0] + offset[0],
                origin[1] + offset[1],
                origin[2] + offset[2],
            ];
            state.entities.set_pos(slot, at);
            cache.refresh(&state);
            assert_eq!(
                nearest_threat(&cache, origin),
                inside.then_some(at),
                "{name}: {offset:?}"
            );
        }
        state.entities.remove(slot);
    }
}

#[test]
fn threat_sensor_ties_follow_original_sensor_order_across_chunks() {
    let mut state = test_state();
    let origin = [16.0, 201.0, 16.0];
    villager(&mut state, origin);
    for at in [
        [17.0, 201.0, 16.0],
        [15.0, 201.0, 16.0],
        [16.0, 201.0, 17.0],
        [16.0, 201.0, 15.0],
    ] {
        threat_sensor_mob(&mut state, "minecraft:zombie", at);
    }
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    let expected = threat_sensor_full_scan(&cache, origin);
    assert_eq!(expected, Some(cache.threats[0].0));
    assert_eq!(nearest_threat(&cache, origin), expected);
    // Reverse sensor indices without changing geometry or chunk traversal:
    // the tie must still follow the global sequence, not the first chunk.
    cache.threats.reverse();
    let count = cache.threats.len();
    for indices in cache.threat_chunks.values_mut() {
        for index in indices.iter_mut() {
            *index = count - 1 - *index;
        }
    }
    let reversed = threat_sensor_full_scan(&cache, origin);
    assert_ne!(expected, reversed);
    assert_eq!(nearest_threat(&cache, origin), reversed);
}

#[test]
fn threat_sensor_refresh_reuses_buckets_and_discards_moved_or_removed_threats() {
    let mut state = test_state();
    villager(&mut state, [0.0, 201.0, 0.0]);
    villager(&mut state, [80.0, 201.0, 0.0]);
    let moved = threat_sensor_mob(&mut state, "minecraft:pillager", [1.0, 201.0, 0.0]);
    let moved_id = state.entities.body(moved).id;
    let removed = threat_sensor_mob(&mut state, "minecraft:zombie", [-1.0, 201.0, 0.0]);
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    let bucket_capacity = cache.threat_chunks[&(0, 0)].capacity();
    let map_capacity = cache.threat_chunks.capacity();
    let threats_capacity = cache.threats.capacity();
    cache.refresh(&state);
    assert_eq!(cache.threat_chunks[&(0, 0)].capacity(), bucket_capacity);
    assert_eq!(cache.threat_chunks.capacity(), map_capacity);
    assert_eq!(cache.threats.capacity(), threats_capacity);
    state.entities.set_pos(moved, [80.0, 201.0, 0.0]);
    state.entities.remove(removed);
    cache.refresh(&state);
    assert_eq!(nearest_threat(&cache, [0.0, 201.0, 0.0]), None);
    assert_eq!(
        nearest_threat(&cache, [80.0, 201.0, 0.0]),
        Some([80.0, 201.0, 0.0])
    );
    assert!(!cache.threat_chunks.contains_key(&(0, 0)));
    assert!(!cache.threat_chunks.contains_key(&(-1, 0)));
    assert_eq!(cache.threat_chunks.len(), 1);
    assert_eq!(cache.threats.len(), 1);
    state
        .entities
        .remove(state.entities.slot_of(moved_id).unwrap());
    cache.refresh(&state);
    assert!(cache.threats.is_empty());
    assert!(cache.threat_chunks.is_empty());
    assert_eq!(cache.threat_max_range, 0.0);
    assert_eq!(nearest_threat(&cache, [80.0, 201.0, 0.0]), None);
}

#[test]
fn numeric_food_table_matches_name_oracle_for_every_registered_item() {
    let synthetic = mc_data::items::ItemRegistry::from_report(
        &[
            ("minecraft:carrot", 17),
            ("minecraft:potato", 31),
            ("minecraft:beetroot", 63),
            ("minecraft:bread", 101),
            ("example:bread", 120),
            ("minecraft:wheat", 201),
            ("minecraft:wheat_seeds", 251),
            ("minecraft:beetroot_seeds", 301),
            ("example:carrot", 351),
            ("example:stone", u32::MAX),
            ("example:potato", 401),
            ("minecraft:stone", 401),
        ]
        .map(|(name, protocol_id)| mc_data::items::ItemReport {
            id: mc_data::Identifier::parse(name).unwrap(),
            protocol_id,
        }),
    );
    for items in [
        Arc::new(mc_data::items::solaris_required_items()),
        Arc::new(synthetic),
    ] {
        let mut food = FoodTable::default();
        food.refresh(&items);
        for (_, id) in items.iter() {
            let name = items.name_of(id).unwrap();
            let expected = match name.path() {
                "bread" => 4,
                "carrot" | "potato" | "beetroot" => 1,
                _ => 0,
            };
            assert_eq!(food_points(&food, id), expected, "{name}: {id}");
            assert_eq!(
                food.entries
                    .get(id as usize)
                    .is_some_and(|entry| entry.farmer_pickup),
                matches!(name.path(), "wheat" | "wheat_seeds" | "beetroot_seeds"),
                "{name}: {id}"
            );
        }
        assert_eq!(food_points(&food, 0), 0);
        assert_eq!(food_points(&food, u32::MAX), 0);
    }
}

#[test]
fn numeric_food_table_refreshes_after_registry_extension_and_replacement() {
    let mut state = test_state();
    let bread = item(&state, "minecraft:bread");
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    assert_eq!(food_points(&cache.food, bread), 4);
    let allocation = cache.food.entries.as_ptr();
    cache.refresh(&state);
    assert_eq!(cache.food.entries.as_ptr(), allocation);

    let mut report: Vec<_> = state
        .registry
        .items
        .iter()
        .map(|(id, protocol_id)| mc_data::items::ItemReport {
            id: id.clone(),
            protocol_id,
        })
        .collect();
    let added = report.iter().map(|entry| entry.protocol_id).max().unwrap() + 17;
    report.push(mc_data::items::ItemReport {
        id: mc_data::Identifier::parse("example:bread").unwrap(),
        protocol_id: added,
    });
    let registry = Arc::make_mut(&mut state.registry);
    *Arc::make_mut(&mut registry.items) = mc_data::items::ItemRegistry::from_report(&report);
    assert_ne!(cache.food.items.as_ptr(), Arc::as_ptr(&registry.items));
    cache.refresh(&state);
    assert_eq!(food_points(&cache.food, bread), 4);
    assert_eq!(food_points(&cache.food, added), 4);

    Arc::make_mut(&mut state.registry).items =
        Arc::new(mc_data::items::ItemRegistry::from_report(&[
            mc_data::items::ItemReport {
                id: mc_data::Identifier::parse("minecraft:stone").unwrap(),
                protocol_id: bread,
            },
            mc_data::items::ItemReport {
                id: mc_data::Identifier::parse("example:carrot").unwrap(),
                protocol_id: added,
            },
        ]));
    cache.refresh(&state);
    assert_eq!(food_points(&cache.food, bread), 0);
    assert_eq!(food_points(&cache.food, added), 1);
    assert_eq!(food_points(&cache.food, u32::MAX), 0);
}

#[test]
fn numeric_food_totals_and_birth_keep_inventory_order_and_unknown_items() {
    let items = Arc::new(mc_data::items::solaris_required_items());
    let id = |name| {
        items
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .unwrap()
    };
    let bread = id("minecraft:bread");
    let carrot = id("minecraft:carrot");
    let potato = id("minecraft:potato");
    let mut food = FoodTable::default();
    food.refresh(&items);
    let mut v = VillagerState {
        food_level: 1,
        ..VillagerState::default()
    };
    v.inventory[0] = Some(ItemStack::new(u32::MAX, 5));
    v.inventory[1] = Some(ItemStack::new(bread, 2));
    v.inventory[2] = Some(ItemStack::new(carrot, 3));
    v.inventory[3] = Some(ItemStack::new(potato, -2));
    assert_eq!(total_food(&food, &v), 12);
    assert!(willing(&food, &v));
    eat_for_birth(&food, &mut v);
    assert_eq!(v.food_level, 0);
    assert_eq!(v.inventory[0], Some(ItemStack::new(u32::MAX, 5)));
    assert!(v.inventory[1].is_none());
    assert!(v.inventory[2].is_none());
    assert_eq!(v.inventory[3], Some(ItemStack::new(potato, -2)));
    assert!(!willing(&food, &v));
}

#[test]
fn numeric_food_pickup_and_sharing_preserve_unknown_items_and_farmer_seeds() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.0, 201.0, 8.0]);
    let recipient = villager(&mut state, [9.0, 201.0, 8.0]);
    let bread = item(&state, "minecraft:bread");
    let seeds = item(&state, "minecraft:wheat_seeds");
    let mut food = FoodTable::default();
    food.refresh(&state.registry.items);
    let unknown =
        crate::tick::spawn_item_entity(&mut state, [8.0, 201.0, 8.0], ItemStack::new(u32::MAX, 5));
    let seed_drop =
        crate::tick::spawn_item_entity(&mut state, [8.0, 201.0, 8.0], ItemStack::new(seeds, 3));
    let unknown_slot = state.entities.slot_of(unknown).unwrap();
    let seed_slot = state.entities.slot_of(seed_drop).unwrap();
    state.entities.body_mut(unknown_slot).pickup_delay = 0;
    state.entities.body_mut(seed_slot).pickup_delay = 0;
    let mut v = state.entities.row_mut(slot).villager.take().unwrap();
    v.data.profession = Profession::Farmer;
    let mut changes = ChangeSet::default();
    pickup(
        &mut state,
        &food,
        &[unknown_slot, seed_slot],
        slot,
        &mut v,
        &mut changes,
    );
    assert!(state.entities.body(unknown_slot).alive);
    assert_eq!(
        state
            .entities
            .row(unknown_slot)
            .item
            .as_ref()
            .unwrap()
            .count,
        5
    );
    assert!(!state.entities.body(seed_slot).alive);
    assert_eq!(v.inventory[0], Some(ItemStack::new(seeds, 3)));
    assert_eq!(changes.entity_events.len(), 1);

    state.tick = 20;
    v.inventory[0] = Some(ItemStack::new(u32::MAX, 64));
    v.inventory[1] = Some(ItemStack::new(bread, 64));
    share_food(&mut state, &food, slot, &mut v, &mut changes);
    assert_eq!(v.inventory[0], Some(ItemStack::new(u32::MAX, 64)));
    assert_eq!(v.inventory[1], Some(ItemStack::new(bread, 32)));
    assert_eq!(food_count(&state, recipient, bread), 0);
    assert_eq!(changes.entities_spawned.len(), 1);
    let shared = state.entities.slot_of(changes.entities_spawned[0]).unwrap();
    let stack = state.entities.row(shared).item.as_ref().unwrap();
    assert_eq!((stack.item_id, stack.count), (bread, 32));
}

#[test]
fn stable_idle_residents_do_not_reenter_dense_execution_or_rebuild_due_bands() {
    let mut state = test_state();
    state.tick = 1;
    state.day_time = 9_000;
    for _ in 0..DECISIONS_PER_TICK * 3 + 17 {
        let slot = villager(&mut state, [8.5, 201.0, 8.5]);
        state.entities.set_ground(slot, true);
        let id = state.entities.body(slot).id;
        state.villager_pois.search_at.insert(id, u64::MAX);
    }
    state.villager_pois.motion_revision = Some(state.world.revision_now());
    let mut idle_gap_seen = false;
    for _ in 0..12 {
        step(&mut state);
        let cache = &state.villager_pois;
        assert_eq!(cache.execution_indices.len(), cache.selected.len());
        assert!(cache.selected.len() <= DECISIONS_PER_TICK);
        idle_gap_seen |= cache.selected.is_empty();
        assert!(cache.profile_updates.is_empty());
        assert!(!cache.rebuild_schedule);
        assert!(cache.brain_due.len() <= cache.residents.len() * 3);
    }
    assert!(
        idle_gap_seen,
        "a non-due, motionless population requires no row execution"
    );
}

#[test]
fn far_service_deadline_includes_queueing() {
    let expected_period = 32;
    let mut state = test_state();
    state.tick = 1;
    state.day_time = 0;
    for _ in 0..8 {
        let slot = villager(&mut state, [8.5, 201.0, 8.5]);
        state.entities.set_ground(slot, true);
        state
            .entities
            .row_mut(slot)
            .villager
            .as_mut()
            .unwrap()
            .activity = Activity::Rest;
    }
    let mut cache = PoiCache {
        motion_revision: Some(state.world.revision_now()),
        ..PoiCache::default()
    };
    for slot in state.entities.slots() {
        cache
            .search_at
            .insert(state.entities.body(slot).id, u64::MAX);
    }
    let mut last = vec![None; state.entities.len()];
    let mut maximum = 0;
    for _ in 0..140 {
        cache.refresh(&state);
        begin_brain_tick(&mut state);
        cache.prepare_brains(&mut state);
        for &slot in &cache.selected {
            let previous = &mut last[slot.0 as usize];
            if let Some(previous) = *previous {
                maximum = maximum.max(state.tick - previous);
            }
            *previous = Some(state.tick);
            let index = cache.resident_index[&state.entities.body(slot).id];
            assert_eq!(cache.resident_data[index].cadence.period, expected_period);
        }
        state.tick += 1;
    }
    assert_eq!(
        maximum,
        expected_period - 1,
        "reserve one tick for queue service"
    );
}

#[test]
fn claims_acquired_after_snapshot_are_released_on_death_conversion_or_removal() {
    for disposition in 0..3 {
        let mut state = test_state();
        let slot = villager(&mut state, [8.5, 201.0, 8.5]);
        let id = state.entities.body(slot).id;
        let home = BlockPos { x: 8, y: 201, z: 8 };
        let job = BlockPos { x: 9, y: 201, z: 8 };
        let mut cache = PoiCache::default();
        cache.refresh(&state);
        let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
        v.home = Some(home);
        v.job_site = Some(job);
        cache.update_claims(id, [None; 3], [Some(home), Some(job), None]);
        let index = cache.resident_index[&id];
        assert_eq!(
            cache.resident_data[index].claims,
            [Some(home), Some(job), None]
        );
        match disposition {
            0 => state.entities.body_mut(slot).alive = false,
            1 => {
                state
                    .entities
                    .row_mut(slot)
                    .villager
                    .as_mut()
                    .unwrap()
                    .conversion_time = Some(-1)
            }
            _ => {
                state.entities.remove(slot);
            }
        }
        cache.refresh(&state);
        assert!(
            !cache
                .homes
                .get(&home)
                .is_some_and(|owners| owners.contains(&id))
        );
        assert!(
            !cache
                .jobs
                .get(&job)
                .is_some_and(|owners| owners.contains(&id))
        );
    }
}

#[test]
fn unloaded_villager_keeps_persisted_bed_and_workstation_custody() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let id = state.entities.body(slot).id;
    let home = BlockPos { x: 8, y: 201, z: 8 };
    let job = BlockPos { x: 9, y: 201, z: 8 };
    let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
    v.home = Some(home);
    v.job_site = Some(job);
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    state.entities.set_pos(slot, [2048.5, 201.0, 8.5]);
    cache.refresh(&state);
    assert!(!cache.resident_data[cache.resident_index[&id]].active);
    assert!(cache.homes[&home].contains(&id));
    assert!(cache.jobs[&job].contains(&id));
}

#[test]
fn ordinary_lifetime_and_nonconverting_passes_do_not_dirty_brain_profiles() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let previous = state.entities.row(slot).age;
    let mut changed = FxHashSet::default();
    state.entities.swap_brain_changes(&mut changed);
    let mut changes = ChangeSet::default();
    crate::natural_spawn::advance_ages(&mut state, &mut changes);
    super::super::conversion::tick(&mut state);
    assert_eq!(state.entities.row(slot).age, previous.wrapping_add(1));
    assert!(state.entities.brain_changes().is_empty());
    assert!(changes.entities_removed.is_empty());
}

#[test]
fn threat_sensor_tracks_in_place_zombie_villager_infection_and_cure() {
    let mut state = test_state();
    let pos = [8.5, 201.0, 8.5];
    let slot = villager(&mut state, pos);
    let original = state.entities.body(slot).kind;
    let zombie = EntityKind::Mob {
        type_id: state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse("minecraft:zombie_villager").unwrap())
            .unwrap(),
    };
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    assert_eq!(nearest_threat(&cache, pos), None);
    for (kind, expected) in [(zombie, Some(pos)), (original, None)] {
        let mut changed = FxHashSet::default();
        state.entities.swap_brain_changes(&mut changed);
        state.entities.set_kind(slot, kind);
        cache.refresh(&state);
        assert_eq!(nearest_threat(&cache, pos), expected);
    }
}

#[test]
fn player_chunk_crossing_only_reclassifies_changed_distance_classes() {
    use crate::boundary::{ConnectionId, PlayerInput};
    let mut state = test_state();
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::from_u128(1),
            name: "DistanceClass".into(),
            permission_level: 0,
        }],
    );
    state.day_time = 0;
    let player = state
        .entities
        .slot_of(*state.players.keys().next().unwrap())
        .unwrap();
    state.entities.set_pos(player, [320.5, 201.0, 8.5]);
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    state.entities.set_ground(slot, true);
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .activity = Activity::Rest;
    let id = state.entities.body(slot).id;
    let mut cache = PoiCache::default();
    cache.search_at.insert(id, u64::MAX);
    cache.refresh(&state);
    begin_brain_tick(&mut state);
    cache.prepare_brains(&mut state);
    let version = cache.resident_data[cache.resident_index[&id]].due_version;
    let mut changed = FxHashSet::default();
    state.entities.swap_brain_changes(&mut changed);
    state.tick += 1;
    state.entities.set_pos(player, [304.5, 201.0, 8.5]);
    cache.refresh(&state);
    begin_brain_tick(&mut state);
    cache.prepare_brains(&mut state);
    assert_eq!(
        cache.resident_data[cache.resident_index[&id]].due_version,
        version
    );
    state.entities.set_pos(player, [32.5, 201.0, 8.5]);
    cache.refresh(&state);
    begin_brain_tick(&mut state);
    cache.prepare_brains(&mut state);
    assert!(cache.resident_data[cache.resident_index[&id]].near);
    assert!(cache.admitted(slot));
}

#[test]
fn newly_arriving_threat_preempts_existing_protected_work() {
    let mut state = test_state();
    state.tick = 1;
    state.day_time = 0;
    let mut cache = PoiCache::default();
    for _ in 0..DECISIONS_PER_TICK * 2 {
        let slot = villager(&mut state, [8.5, 201.0, 8.5]);
        state.entities.set_ground(slot, true);
        let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
        v.activity = Activity::Rest;
        v.panic_until = u64::MAX;
        cache
            .search_at
            .insert(state.entities.body(slot).id, u64::MAX);
    }
    let pos = [168.5, 201.0, 8.5];
    state
        .world
        .ensure_blocking(&FxHashMap::from_iter([(ChunkPos { x: 10, z: 0 }, 0)]));
    activate_fixture_chunks(&mut state, [ChunkPos { x: 10, z: 0 }]);
    let target = villager(&mut state, pos);
    state.entities.set_ground(target, true);
    state
        .entities
        .row_mut(target)
        .villager
        .as_mut()
        .unwrap()
        .activity = Activity::Rest;
    cache
        .search_at
        .insert(state.entities.body(target).id, u64::MAX);
    cache.refresh(&state);
    begin_brain_tick(&mut state);
    cache.prepare_brains(&mut state);
    assert!(!cache.admitted(target));
    threat_sensor_mob(&mut state, "minecraft:zombie_villager", [169.5, 201.0, 8.5]);
    state.tick += 1;
    cache.refresh(&state);
    begin_brain_tick(&mut state);
    cache.prepare_brains(&mut state);
    assert!(cache.admitted(target));
    assert!(cache.selected.len() <= DECISIONS_PER_TICK);
}

#[test]
fn bounded_gossip_window_reaches_actors_beyond_the_first_priority_prefix() {
    let mut state = test_state();
    state.tick = 1_200;
    for _ in 0..128 {
        villager(&mut state, [8.5, 201.0, 8.5]);
    }
    state
        .world
        .ensure_blocking(&FxHashMap::from_iter([(ChunkPos { x: 6, z: 0 }, 0)]));
    activate_fixture_chunks(&mut state, [ChunkPos { x: 6, z: 0 }]);
    let first = villager(&mut state, [104.5, 201.0, 8.5]);
    let second = villager(&mut state, [105.0, 201.0, 8.5]);
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    begin_brain_tick(&mut state);
    cache.prepare_brains(&mut state);
    interact_gossip(&mut state, &cache);
    assert_eq!(
        state
            .entities
            .row(first)
            .villager
            .as_ref()
            .unwrap()
            .last_gossip,
        state.tick
    );
    assert_eq!(
        state
            .entities
            .row(second)
            .villager
            .as_ref()
            .unwrap()
            .last_gossip,
        state.tick
    );
}

#[test]
fn golem_gossip_summoning_requires_five_recently_sleeping_villagers() {
    for (count, slept, expected) in [(4, true, 0), (5, false, 0), (5, true, 1)] {
        let mut state = test_state();
        state.tick = 1_200;
        let mut actors = Vec::new();
        for n in 0..count {
            let slot = villager(&mut state, [6.5 + f64::from(n % 2), 201.0, 6.5]);
            let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
            v.activity = Activity::Meet;
            v.last_slept = slept.then_some(1_000);
            v.last_gossip = state.tick;
            actors.push(slot);
        }
        let mut cache = PoiCache::default();
        cache.refresh(&state);
        let mut changes = ChangeSet::default();
        // Try successive bounded decision windows: placement is deterministic,
        // but the first attempt intersects the summoner's body.
        for _ in 0..12 {
            begin_brain_tick(&mut state);
            cache.prepare_brains(&mut state);
            golems(&mut state, &mut cache, &mut changes);
            state.tick += 100;
            if !changes.entities_spawned.is_empty() {
                break;
            }
        }
        assert_eq!(changes.entities_spawned.len(), expected, "{count}, {slept}");
        if expected != 0 {
            assert!(actors.iter().all(|&slot| {
                state
                    .entities
                    .row(slot)
                    .villager
                    .as_ref()
                    .unwrap()
                    .golem_detected_until
                    .is_some()
            }));
            golems(&mut state, &mut cache, &mut changes);
            assert_eq!(changes.entities_spawned.len(), 1, "no duplicate summon");
        }
    }
}

#[test]
fn golem_detection_uses_axis_aligned_range_and_ignores_dead_golems() {
    for alive in [true, false] {
        let mut state = test_state();
        state.tick = 200;
        let resident = villager(&mut state, [0.5, 201.0, 0.5]);
        let type_id = state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse("minecraft:iron_golem").unwrap())
            .unwrap();
        let id = state.spawn_mob(type_id, [15.5, 201.0, 15.5], 100.0);
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.body_mut(slot).alive = alive;
        let mut cache = PoiCache::default();
        cache.refresh(&state);
        begin_brain_tick(&mut state);
        cache.prepare_brains(&mut state);
        golems(&mut state, &mut cache, &mut ChangeSet::default());
        assert_eq!(
            state
                .entities
                .row(resident)
                .villager
                .as_ref()
                .unwrap()
                .golem_detected_until,
            alive.then_some(799)
        );
    }
}

fn motion_geometry_fixture() -> (TickState, EntitySlot, VillagerState) {
    let mut state = test_state();
    let start = [8.5, 201.0, 8.5];
    let slot = villager(&mut state, start);
    state.entities.set_ground(slot, true);
    let mut v = VillagerState {
        navigation_target: Some(BlockPos {
            x: 11,
            y: 201,
            z: 8,
        }),
        navigation_path: (9..=11)
            .rev()
            .map(|x| BlockPos { x, y: 201, z: 8 })
            .collect(),
        ..VillagerState::default()
    };
    capture_route(&state, &mut v, start, true);
    (state, slot, v)
}

#[test]
fn motion_geometry_hits_and_route_edits_match_live_collision() {
    for committed_edit in [false, true] {
        let (mut state, slot, mut v) = motion_geometry_fixture();
        let mut geometry = std::mem::take(&mut v.motion_geometry);
        let cell = *v.navigation_path.last().unwrap();
        let unchanged = geometry.begin(&state, &v, 0.3, 1.95);
        assert!(unchanged);
        let next = geometry.standing(&state, &v, cell, unchanged);
        let first = resolve_body_motion(&state, slot, next, 5.0, &v, &mut geometry, unchanged);
        assert!(!geometry.blocks.is_empty());
        assert!(
            geometry.blocks.len() <= 20,
            "only the actual swept/step envelope is sampled"
        );
        assert_eq!(geometry.geometry_misses, 1);
        assert_eq!(geometry.geometry_hits, 0);
        assert_eq!(geometry.standing(&state, &v, cell, unchanged), next);
        let hit = resolve_body_motion(&state, slot, next, 5.0, &v, &mut geometry, unchanged);
        assert_eq!(geometry.geometry_hits, 1);
        assert_eq!(geometry.standing_hits, 1);
        assert_eq!(first, hit);
        let mut live = MotionGeometryCache::default();
        assert_eq!(
            hit,
            resolve_body_motion(&state, slot, next, 5.0, &v, &mut live, false),
        );

        // This block is in the warmed swept envelope, not merely on a distant
        // route node. Both pending and folded edits must change real clipping.
        state
            .world
            .set_block(9, 201, 8, state.registry.states.stone);
        if committed_edit {
            state.world.fold_overlay();
        }
        assert!(route_changed(&state, &v));
        let unchanged = geometry.begin(&state, &v, 0.3, 1.95);
        assert!(!unchanged);
        assert!(geometry.blocks.is_empty());
        assert!(geometry.standing.is_none());
        assert_eq!(geometry.standing(&state, &v, cell, unchanged), None);
        let edited = resolve_body_motion(&state, slot, next, 5.0, &v, &mut geometry, unchanged);
        assert_eq!(
            geometry.geometry_hits, 1,
            "invalid routes cannot hit stored geometry"
        );
        assert_eq!(
            edited,
            resolve_body_motion(&state, slot, next, 5.0, &v, &mut live, false),
        );
        assert!(
            edited.as_ref().unwrap().position[0] < first.as_ref().unwrap().position[0],
            "the edited wall must clip movement: committed={committed_edit}",
        );
    }
}

#[test]
fn motion_geometry_body_size_and_domain_fences_keep_live_shapes() {
    let (mut state, slot, mut v) = motion_geometry_fixture();
    state
        .world
        .ensure_blocking(&FxHashMap::from_iter([(ChunkPos { x: 1, z: 0 }, 0)]));
    state
        .world
        .set_block(16, 201, 8, state.registry.states.stone);
    state.world.fold_overlay();
    state
        .world
        .set_block(9, 202, 8, state.registry.states.stone);
    state.world.fold_overlay();
    capture_route(&state, &mut v, state.entities.pos(slot), true);
    let mut geometry = std::mem::take(&mut v.motion_geometry);
    let cell = *v.navigation_path.last().unwrap();
    let unchanged = geometry.begin(&state, &v, 0.15, 0.975);
    assert!(geometry.standing(&state, &v, cell, unchanged).is_some());
    let unchanged = geometry.begin(&state, &v, 0.3, 1.95);
    assert!(
        geometry.standing.is_none(),
        "growing a body clears its standing proof"
    );
    assert_eq!(geometry.standing(&state, &v, cell, unchanged), None);
    assert_eq!(
        standing(&state.world, &state.registry, cell, 0.3, 1.95),
        None
    );
    let next = Some([9.5, 201.0, 8.5]);
    let cached = resolve_body_motion(&state, slot, next, 5.0, &v, &mut geometry, unchanged);
    let mut live = MotionGeometryCache::default();
    assert_eq!(
        cached,
        resolve_body_motion(&state, slot, next, 5.0, &v, &mut live, false),
    );
    assert!(!geometry.blocks.is_empty());

    // Even a still-valid route proof must not make unrecorded cells air.
    let sampler = MotionSampler {
        live: TickSampler {
            world: &state.world,
            registry: &state.registry,
        },
        geometry: Some(&geometry),
    };
    assert!(geometry.block_at(12, 200, 8).is_none());
    let mut sampled = Vec::new();
    let mut ordinary = Vec::new();
    sampler.collision_boxes_at(12, 200, 8, &mut |shape| sampled.push(shape));
    sampler
        .live
        .collision_boxes_at(12, 200, 8, &mut |shape| ordinary.push(shape));
    assert!(!ordinary.is_empty());
    assert_eq!(sampled, ordinary);

    // Chunk 1 is resident, but is not in this route's captured halo. A shove
    // across that scope fence must sample its wall live, not trust chunk 0.
    assert!(!route_changed(&state, &v));
    assert!(!v.navigation_chunks.iter().any(|&(chunk, _)| chunk.x == 1));
    state.entities.set_pos(slot, [15.6, 201.0, 8.5]);
    let unchanged = geometry.begin(&state, &v, 0.3, 1.95);
    let hits = geometry.geometry_hits;
    let next = Some([17.5, 201.0, 8.5]);
    let fenced = resolve_body_motion(&state, slot, next, 5.0, &v, &mut geometry, unchanged);
    assert_eq!(geometry.geometry_hits, hits);
    assert_eq!(
        fenced,
        resolve_body_motion(&state, slot, next, 5.0, &v, &mut live, false),
    );
    assert!(
        fenced.unwrap().position[0] < 15.85,
        "the uncaptured wall still clips"
    );

    // Known chunks can still contain an unknown state. Caching must decline
    // it and preserve TickSampler's conservative full-block interpretation.
    state.entities.set_pos(slot, [8.5, 201.0, 8.5]);
    state.world.set_block(9, 201, 8, u32::MAX);
    state.world.fold_overlay();
    v.motion_geometry = geometry;
    capture_route(&state, &mut v, state.entities.pos(slot), true);
    let mut geometry = std::mem::take(&mut v.motion_geometry);
    let unchanged = geometry.begin(&state, &v, 0.3, 1.95);
    let unknown = resolve_body_motion(
        &state,
        slot,
        Some([9.5, 201.0, 8.5]),
        5.0,
        &v,
        &mut geometry,
        unchanged,
    );
    assert!(geometry.blocks.is_empty());
    assert_eq!(
        unknown,
        resolve_body_motion(
            &state,
            slot,
            Some([9.5, 201.0, 8.5]),
            5.0,
            &v,
            &mut live,
            false,
        ),
    );
}

#[test]
fn skipped_motion_geometry_and_journal_clones_stay_transient() {
    let (mut state, slot, v) = motion_geometry_fixture();
    state.entities.row_mut(slot).villager = Some(Box::new(v));
    let mut changed = FxHashSet::default();
    state.entities.swap_brain_changes(&mut changed);
    for _ in 0..8 {
        move_skipped(&mut state, slot, false, false, Activity::Idle);
        state.tick += 1;
    }
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    assert!(state.entities.pos(slot)[0] > 8.5);
    assert!(
        v.navigation_path.len() < 3,
        "native path progress is still consumed"
    );
    assert!(v.motion_geometry.geometry_hits > 0);
    assert!(v.motion_geometry.standing_hits > 0);
    assert!(!v.motion_geometry.blocks.is_empty());
    assert!(
        state.entities.brain_changes().is_empty(),
        "cache hits never dirty the cold row"
    );
    let mut cloned = v.clone();
    assert_eq!(
        cloned, *v,
        "operational cache state is not durable equality"
    );
    assert!(cloned.motion_geometry.blocks.is_empty());
    assert_eq!(
        cloned.motion_geometry.blocks.capacity(),
        0,
        "journal clones allocate no geometry"
    );
    assert!(cloned.motion_geometry.standing.is_none());

    // A fresh capture clears both caches while retaining reusable capacity on
    // the live actor; it cannot carry a standing point into the new route.
    cloned.motion_geometry =
        std::mem::take(state.entities.villager_motion_geometry_mut(slot).unwrap());
    let capacity = cloned.motion_geometry.blocks.capacity();
    capture_route(&state, &mut cloned, state.entities.pos(slot), true);
    assert!(cloned.motion_geometry.blocks.is_empty());
    assert!(cloned.motion_geometry.standing.is_none());
    assert_eq!(cloned.motion_geometry.blocks.capacity(), capacity);
}

#[test]
fn motion_geometry_on_and_off_match_every_physical_tick() {
    for skipped in [false, true] {
        let (mut state, cached_slot, cached) = motion_geometry_fixture();
        let start = state.entities.pos(cached_slot);
        let live_slot = villager(&mut state, start);
        state.entities.set_ground(live_slot, true);
        let mut live = cached.clone();
        live.motion_geometry.disable = true;
        state.entities.row_mut(cached_slot).villager = Some(Box::new(cached));
        state.entities.row_mut(live_slot).villager = Some(Box::new(live));
        for tick in 0..16 {
            if tick == 4 {
                for slot in [cached_slot, live_slot] {
                    state.entities.set_brain_impulse(slot, [1.4, -0.35]);
                }
            }
            if tick == 7 {
                for slot in [cached_slot, live_slot] {
                    state.entities.set_vel(slot, [0.0, 4.0, 0.0]);
                    state.entities.set_ground(slot, false);
                }
            }
            if tick == 10 {
                state.world.set_block(10, 200, 8, state.registry.states.air);
            }
            if tick == 12 {
                state.world.fold_overlay();
            }
            for slot in [cached_slot, live_slot] {
                if skipped {
                    move_skipped(&mut state, slot, tick >= 10, false, Activity::Idle);
                } else {
                    let mut v = state.entities.row_mut(slot).villager.take().unwrap();
                    move_committed(&mut state, slot, &mut v, Activity::Idle);
                    state.entities.row_mut(slot).villager = Some(v);
                }
            }
            assert_eq!(
                state.entities.pos(cached_slot),
                state.entities.pos(live_slot),
                "position tick={tick}, skipped={skipped}",
            );
            assert_eq!(
                state.entities.vel(cached_slot),
                state.entities.vel(live_slot),
                "velocity tick={tick}, skipped={skipped}",
            );
            assert_eq!(
                state.entities.ground(cached_slot),
                state.entities.ground(live_slot),
                "ground tick={tick}, skipped={skipped}",
            );
            let cached = state.entities.row(cached_slot);
            let live = state.entities.row(live_slot);
            assert_eq!(
                state.entities.horizontal_impulse(cached_slot),
                state.entities.horizontal_impulse(live_slot)
            );
            assert_eq!(
                cached.villager.as_ref().unwrap().navigation_path,
                live.villager.as_ref().unwrap().navigation_path,
            );
            state.tick += 1;
        }
        let cached = state.entities.row(cached_slot).villager.as_ref().unwrap();
        let live = state.entities.row(live_slot).villager.as_ref().unwrap();
        assert!(cached.motion_geometry.geometry_hits > 0);
        assert!(cached.motion_geometry.standing_hits > 0);
        assert_eq!(live.motion_geometry.geometry_hits, 0);
        assert_eq!(live.motion_geometry.geometry_misses, 0);
        assert_eq!(live.motion_geometry.standing_hits, 0);
        assert!(live.motion_geometry.blocks.is_empty());
        let mut cloned = live.clone();
        assert!(
            cloned.motion_geometry.disable,
            "cloning preserves test reference mode"
        );
        capture_route(&state, &mut cloned, state.entities.pos(live_slot), true);
        assert!(
            cloned.motion_geometry.disable,
            "new captures preserve test reference mode"
        );
    }
}

fn activate_fixture_chunks(state: &mut TickState, chunks: impl IntoIterator<Item = ChunkPos>) {
    for pos in chunks {
        state
            .tickets
            .insert(format!("brain-fixture:{}:{}", pos.x, pos.z), (pos, 0));
    }
    state.refresh_active_chunks(state.tick);
}

#[test]
fn birth_sequence_exhaustion_refuses_before_food_or_spawn() {
    let mut state = test_state();
    state.tick = u64::MAX / 65_536 + 1;
    let first = villager(&mut state, [8.0, 201.0, 8.0]);
    let second = villager(&mut state, [8.5, 201.0, 8.0]);
    let first_id = state.entities.body(first).id;
    let second_id = state.entities.body(second).id;
    let first_home = bed(&mut state, 4, 4);
    let second_home = bed(&mut state, 8, 4);
    bed(&mut state, 12, 4);
    state.world.fold_overlay();
    let bread = item(&state, "minecraft:bread");
    for (slot, partner, home) in [
        (first, second_id, first_home),
        (second, first_id, second_home),
    ] {
        let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
        v.home = Some(home);
        v.inventory[0] = Some(ItemStack::new(bread, 3));
        v.partner = Some(partner);
        v.courtship_until = Some(state.tick);
        v.courtship_started_tick = Some(0);
    }
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    begin_brain_tick(&mut state);
    let mut changes = ChangeSet::default();
    courtship(&mut state, &mut cache, &mut changes);
    assert!(
        changes.entities_spawned.is_empty(),
        "unrepresentable birth identity refuses atomically"
    );
    assert_eq!(state.entities.len(), 2);
    for parent in [first, second] {
        assert_eq!(food_count(&state, parent, bread), 3);
        assert_eq!(state.entities.row(parent).villager.as_ref().unwrap().age, 0);
    }
}

#[test]
fn city_with_ten_thousand_residents_forms_large_households_and_births_use_real_beds() {
    let mut state = test_state();
    state.tick = 300;
    state.day_time = 1000;
    let settlement = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "largecity".into(),
            "owner".into(),
            [8, 201, 8],
            48,
        )
        .unwrap();
    let mut people = Vec::with_capacity(10_000);
    for _ in 0..10_000 {
        let slot = villager(&mut state, [14.5, 201.0, 14.5]);
        let id = state.entities.body(slot).id;
        crate::settlements::bind(&mut state, settlement, id).unwrap();
        people.push(id);
    }
    let first = state.entities.slot_of(people[0]).unwrap();
    let second = state.entities.slot_of(people[1]).unwrap();
    state.entities.set_pos(first, [8.0, 201.0, 8.0]);
    state.entities.set_pos(second, [8.5, 201.0, 8.0]);
    let mut homes = vec![bed(&mut state, 4, 4), bed(&mut state, 8, 4)];
    for z in [10, 12, 14] {
        for x in [1, 3, 5, 7, 9, 11] {
            homes.push(bed(&mut state, x, z));
        }
    }
    for (&entity, home) in people.iter().zip(homes) {
        let slot = state.entities.slot_of(entity).unwrap();
        state.entities.row_mut(slot).villager.as_mut().unwrap().home = Some(home);
    }
    let child_home = bed(&mut state, 12, 4);
    state.world.fold_overlay();
    for family in people.chunks(20) {
        crate::settlements::households::form(
            &mut state,
            settlement,
            family,
            crate::settlements::households::HouseholdOrigin::Adoption,
        )
        .expect("physical housing, not a plugin member/count cap, bounds a native city");
    }
    assert_eq!(state.settlements.member_count(settlement), 10_000);
    assert_eq!(
        state.settlements.get(settlement).unwrap().households.len(),
        500
    );
    assert!(crate::settlements::validate_memberships(&state).is_ok());
    let family = state
        .entities
        .row(first)
        .villager
        .as_ref()
        .unwrap()
        .household
        .clone()
        .unwrap();
    let bread = item(&state, "minecraft:bread");
    for (slot, partner) in [(first, people[1]), (second, people[0])] {
        let villager = state.entities.row_mut(slot).villager.as_mut().unwrap();
        villager.inventory[0] = Some(ItemStack::new(bread, 3));
        villager.partner = Some(partner);
        villager.courtship_until = Some(300);
        villager.courtship_started_tick = Some(0);
    }
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    begin_brain_tick(&mut state);
    let mut changes = ChangeSet::default();
    courtship(&mut state, &mut cache, &mut changes);
    assert_eq!(
        changes.entities_spawned.len(),
        1,
        "the twenty-first actual bed permits birth"
    );
    let child = state.entities.slot_of(changes.entities_spawned[0]).unwrap();
    let child = state.entities.row(child).villager.as_ref().unwrap();
    assert_eq!(child.home, Some(child_home));
    assert_eq!(child.household.as_deref(), Some(family.as_str()));
    assert_eq!(state.settlements.household_size(&family), 21);
    assert_eq!(state.settlements.member_count(settlement), 10_001);
}

#[test]
fn sensor_refresh_ignores_remote_population_and_follows_resident_chunk_moves() {
    let mut state = test_state();
    let resident = villager(&mut state, [-0.1, 201.0, 0.1]);
    let zombie = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
        .unwrap();
    let near = [7.0, 201.0, 0.1];
    let far = [1000.0, 201.0, 1000.0];
    state.spawn_mob(zombie, near, 20.0);
    state.spawn_mob(zombie, far, 20.0);
    let bread = item(&state, "minecraft:bread");
    crate::tick::spawn_item_entity(&mut state, far, ItemStack::new(bread, 1));
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    assert_eq!(nearest_threat(&cache, [-0.1, 201.0, 0.1]), Some(near));
    assert_eq!(cache.threats.len(), 1, "remote mobs must not be projected");
    assert!(cache.items.is_empty(), "remote drops must not be projected");
    state.entities.set_pos(resident, far);
    cache.refresh(&state);
    assert_eq!(nearest_threat(&cache, far), Some(far));
    assert_eq!(cache.threats.len(), 1);
    assert_eq!(cache.items.len(), 1);
}

#[test]
fn golem_detection_uses_resident_positions_after_brain_motion() {
    let mut state = test_state();
    state.tick = 200;
    let resident = villager(&mut state, [8.5, 201.0, 8.5]);
    let golem = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:iron_golem").unwrap())
        .unwrap();
    state.spawn_mob(golem, [31.5, 201.0, 8.5], 100.0);
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    begin_brain_tick(&mut state);
    cache.prepare_brains(&mut state);
    assert!(cache.admitted(resident));
    state.entities.set_pos(resident, [15.9, 201.0, 8.5]);
    golems(&mut state, &mut cache, &mut ChangeSet::default());
    assert_eq!(
        state
            .entities
            .row(resident)
            .villager
            .as_ref()
            .unwrap()
            .golem_detected_until,
        Some(799)
    );
}

#[test]
fn panicking_villager_whose_flee_route_is_walled_off_retreats_home() {
    let mut state = test_state();
    state.tick = 1;
    state.day_time = 3_000;
    let stone = state.registry.states.stone;
    // A sealed one-wide tunnel along x at z=8; the flee direction away from the
    // zombie behind the south wall points into rock.
    for x in 1..=15 {
        for z in 7..=9 {
            for y in 201..=203 {
                let wall = z != 8 || y == 203 || x == 1 || x == 15;
                if wall {
                    state.world.set_block(x, y, z, stone);
                }
            }
        }
    }
    let home = bed(&mut state, 13, 8);
    state.world.fold_overlay();
    let start = [4.5, 201.0, 8.5];
    let slot = villager(&mut state, start);
    let zombie = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
        .unwrap();
    state.spawn_mob(zombie, [4.5, 201.0, 11.5], 20.0);
    {
        let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
        v.home = Some(home);
        v.home_dimension = state.dimension.name().into();
    }
    for _ in 0..400 {
        step(&mut state);
    }
    let pos = state.entities.pos(slot);
    assert!(
        pos[0] > 10.0,
        "a blocked flee must fall back to a reachable refuge, stuck at {pos:?}"
    );
}
