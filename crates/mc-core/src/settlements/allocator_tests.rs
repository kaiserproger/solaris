//! Native allocator regression gates: the S1 housing/POI behavior that the
//! allocator now owns as sole authority (replacing the brain-local gates).
use super::*;
use crate::boundary::{BlockPos, ChangeSet};
use crate::entities::EntitySlot;
use crate::registry::CoreRegistry;
use crate::tick::TickState;
use crate::villager::{VillagerProfession as Profession, VillagerState};
use std::sync::Arc;

pub(crate) fn test_state() -> TickState {
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
        "allocator-fixture".into(),
        (crate::boundary::ChunkPos { x: 0, z: 0 }, 0),
    );
    state.refresh_active_chunks(0);
    state
}
pub(crate) fn block(state: &TickState, name: &str, props: &[(&str, &str)]) -> u32 {
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
pub(crate) fn villager(state: &mut TickState, pos: [f64; 3]) -> EntitySlot {
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
pub(crate) fn bed(state: &mut TickState, x: i32, z: i32) -> BlockPos {
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
fn step(state: &mut TickState) -> ChangeSet {
    state.tick_budget.reset(state.tick);
    let mut changes = ChangeSet::default();
    tick(state, &mut changes);
    crate::villager::brain::tick(state, &mut changes);
    state.tick += 1;
    changes
}

#[test]
fn unemployed_villager_does_not_reserve_work_before_shift() {
    let mut state = test_state();
    state.day_time = 1000;
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(4, 201, 4, composter);
    state.world.fold_overlay();
    let slot = villager(&mut state, [2.5, 201.0, 2.5]);
    for _ in 0..60 {
        step(&mut state);
    }
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
    state.day_time = 2000;
    for _ in 0..240 {
        step(&mut state);
    }
    assert_eq!(
        state.entities.row(slot).villager.as_ref().unwrap().job_site,
        Some(BlockPos { x: 4, y: 201, z: 4 })
    );
}

#[test]
fn allocator_assigns_home_job_and_meeting_point_as_sole_authority() {
    let mut state = test_state();
    state.day_time = 2000;
    let home = bed(&mut state, 8, 8);
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(4, 201, 4, composter);
    let bell = block(&state, "minecraft:bell", &[]);
    state.world.set_block(8, 201, 10, bell);
    state.world.fold_overlay();
    let slot = villager(&mut state, [2.5, 201.0, 2.5]);
    for _ in 0..60 {
        step(&mut state);
    }
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(v.home, Some(home));
    assert_eq!(v.job_site, Some(BlockPos { x: 4, y: 201, z: 4 }));
    assert_eq!(
        v.meeting_point,
        Some(BlockPos {
            x: 8,
            y: 201,
            z: 10
        })
    );
    assert_eq!(v.data.profession, Profession::Farmer);
    assert_eq!(v.home_dimension, "minecraft:overworld");
    assert_eq!(v.job_site_dimension, "minecraft:overworld");
    assert_eq!(v.meeting_point_dimension, "minecraft:overworld");
}

#[test]
fn allocator_never_loads_chunks_for_unloaded_claims() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let before = state.world.chunks().count();
    for _ in 0..40 {
        step(&mut state);
    }
    assert_eq!(state.world.chunks().count(), before);
    // An unloaded local cell is not evidence that a retained claim vanished.
    let unloaded = BlockPos {
        x: 1600,
        y: 201,
        z: 1600,
    };
    state.entities.row_mut(slot).villager.as_mut().unwrap().home = Some(unloaded);
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    let id = state.entities.body(slot).id;
    let mut v = state.entities.row_mut(slot).villager.take().unwrap();
    validate_pois(&state, &cache, id, &mut v);
    state.entities.row_mut(slot).villager = Some(v);
    assert_eq!(
        state.entities.row(slot).villager.as_ref().unwrap().home,
        Some(unloaded)
    );
    assert_eq!(state.world.chunks().count(), before);
}

#[test]
fn allocator_revalidates_destroyed_claims_and_releases_traded_profession() {
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
fn allocator_keeps_every_body_executing_under_budget_pressure() {
    let mut state = test_state();
    state.day_time = 3000;
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(8, 201, 8, composter);
    state.world.fold_overlay();
    let first = villager(&mut state, [7.5, 201.0, 8.5]);
    let second = villager(&mut state, [9.5, 201.0, 8.5]);
    // Adult bodies with nonzero age tick their life clock every brain pass.
    for slot in [first, second] {
        state.entities.row_mut(slot).villager.as_mut().unwrap().age = 100;
    }
    // Exhaust the whole shared allowance before the pass: no admission, but
    // both bodies still run their brain phase every tick.
    state
        .tick_budget
        .exhaust_for_test(crate::tick_budget::Kind::Decision);
    state
        .tick_budget
        .exhaust_for_test(crate::tick_budget::Kind::PathNode);
    let mut changes = ChangeSet::default();
    tick(&mut state, &mut changes);
    crate::villager::brain::tick(&mut state, &mut changes);
    for slot in [first, second] {
        let v = state.entities.row(slot).villager.as_ref().unwrap();
        assert!(v.job_site.is_none());
        assert_eq!(
            v.age, 99,
            "every active body executes its life clock each tick, even with no allowance"
        );
    }
}

#[test]
fn allocator_shares_one_pool_across_dimensions_without_local_reset() {
    let mut state = test_state();
    let mut sibling = test_state();
    sibling.dimension = crate::dimensions::Dimension::Nether;
    state.tick_budget.reset(11);
    state
        .tick_budget
        .exhaust_for_test(crate::tick_budget::Kind::Decision);
    // Swap the globals into a sibling world: the sibling pass in the same
    // global tick observes the same exhausted pool (no refill on entry).
    crate::dimensions::swap_globals(&mut state, &mut sibling);
    assert!(
        !sibling
            .tick_budget
            .try_charge(sibling.dimension, Consumer::Allocator, Kind::Decision, 1),
        "a second dimension in the same global tick must not refill the pool"
    );
    crate::dimensions::swap_globals(&mut state, &mut sibling);
    assert!(
        !state
            .tick_budget
            .try_charge(state.dimension, Consumer::Allocator, Kind::Decision, 1)
    );
    state.tick_budget.reset(11);
    assert!(
        !state
            .tick_budget
            .try_charge(state.dimension, Consumer::Allocator, Kind::Decision, 1)
    );
    state.tick_budget.reset(12);
    assert!(
        state
            .tick_budget
            .remaining(crate::tick_budget::Kind::Decision)
            > 0
    );
}

#[test]
fn allocator_route_refusal_retries_on_next_pass() {
    let mut state = test_state();
    state.day_time = 2000;
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(10, 201, 8, composter);
    state.world.fold_overlay();
    let slot = villager(&mut state, [2.5, 201.0, 8.5]);
    let id = state.entities.body(slot).id;
    state.villager_pois.search_at.insert(id, state.tick);
    state.tick_budget.reset(state.tick);
    state
        .tick_budget
        .exhaust_for_test(crate::tick_budget::Kind::PathNode);
    let mut changes = ChangeSet::default();
    tick(&mut state, &mut changes);
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
    assert!(state.villager_pois.search_deferred);
    let deadline = state.villager_pois.search_at[&id];
    // Next global tick refills the pool; the refused route is retried.
    state.tick += 1;
    state.tick_budget.reset(state.tick);
    tick(&mut state, &mut changes);
    assert_eq!(
        state.entities.row(slot).villager.as_ref().unwrap().job_site,
        Some(BlockPos {
            x: 10,
            y: 201,
            z: 8
        })
    );
    assert!(state.villager_pois.search_at[&id] > deadline);
}

#[test]
fn allocator_rebuilds_member_counts_from_live_columns() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let mut sequence = state.settlement_sequence;
    let id = state
        .settlements
        .create(
            &mut sequence,
            "town".into(),
            "owner".into(),
            [8, 201, 8],
            48,
        )
        .unwrap();
    state.settlement_sequence = sequence;
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .settlement = Some(id);
    let mut changes = ChangeSet::default();
    tick(&mut state, &mut changes);
    assert_eq!(state.settlements.member_count(id), 1);
    state.entities.body_mut(slot).alive = false;
    tick(&mut state, &mut changes);
    assert_eq!(state.settlements.member_count(id), 0);
}

#[test]
fn allocator_scopes_acquisition_to_bound_settlement_radius() {
    let mut state = test_state();
    state.day_time = 2000;
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    // Certified in-settlement workstation vs a closer uncertified one:
    // the bound villager must skip the closer POI and claim the building slot.
    let far = block(&state, "minecraft:composter", &[]);
    state.world.set_block(9, 201, 8, far);
    let near = block(&state, "minecraft:composter", &[]);
    state.world.set_block(14, 201, 8, near);
    state.world.fold_overlay();
    let mut sequence = state.settlement_sequence;
    let id = state
        .settlements
        .create(
            &mut sequence,
            "town".into(),
            "owner".into(),
            [14, 201, 8],
            4,
        )
        .unwrap();
    state.settlement_sequence = sequence;
    crate::settlements::buildings::certify(
        &mut state,
        id,
        crate::settlements::buildings::BuildingKind::Workshop(Profession::Farmer),
        mc_script::ScriptWorldBounds::new([13, 200, 7], [15, 203, 9]).unwrap(),
    )
    .unwrap();
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .settlement = Some(id);
    for _ in 0..60 {
        step(&mut state);
    }
    assert_eq!(
        state.entities.row(slot).villager.as_ref().unwrap().job_site,
        Some(BlockPos {
            x: 14,
            y: 201,
            z: 8
        }),
        "bound villagers claim inside their anchor radius, not the closer foreign POI"
    );
    // Membership survives the body standing outside its radius.
    state.entities.set_pos(slot, [1000.5, 201.0, 1000.5]);
    for _ in 0..5 {
        step(&mut state);
    }
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .settlement,
        Some(id),
        "moving out of radius never clears membership"
    );
    // Unbound villagers keep vanilla reach: the closer POI wins.
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .settlement = None;
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .job_site = None;
    state.entities.set_pos(slot, [8.5, 201.0, 8.5]);
    state
        .villager_pois
        .search_at
        .insert(state.entities.body(slot).id, state.tick);
    for _ in 0..60 {
        step(&mut state);
    }
    assert_eq!(
        state.entities.row(slot).villager.as_ref().unwrap().job_site,
        Some(BlockPos { x: 9, y: 201, z: 8 }),
        "unbound villagers keep vanilla nearby acquisition"
    );
}

#[test]
fn allocator_denies_acquisition_for_foreign_dimension_membership() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(9, 201, 8, composter);
    state.world.fold_overlay();
    // Record lives in another dimension's registry, not the local one.
    let mut other = test_state();
    other.dimension = crate::dimensions::Dimension::Nether;
    let mut sequence = other.settlement_sequence;
    let id = other
        .settlements
        .create(
            &mut sequence,
            "nether-town".into(),
            "owner".into(),
            [8, 201, 8],
            48,
        )
        .unwrap();
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .settlement = Some(id);
    for _ in 0..60 {
        step(&mut state);
    }
    assert!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .job_site
            .is_none(),
        "foreign-dimension membership must not acquire local POIs"
    );
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .settlement,
        Some(id),
        "foreign membership column is retained, not cleared"
    );
}

#[test]
fn admitted_brain_path_does_not_pay_a_second_decision() {
    let mut state = test_state();
    state.tick_budget.exhaust_for_test(Kind::Decision);
    let nodes = state.tick_budget.remaining(Kind::PathNode);
    let mut cache = PoiCache::default();
    let mut path = Vec::new();
    let mut ctx = RouteCtx::tick(&mut state, Consumer::Brain);
    assert!(
        find_path_exact_ctx(
            &mut ctx,
            &mut cache,
            [2.5, 201.0, 8.5],
            BlockPos {
                x: 10,
                y: 201,
                z: 8
            },
            0.3,
            1.95,
            &mut path,
        ),
        "full brain admission already paid its decision; only A* nodes charge here"
    );
    assert!(!path.is_empty());
    assert_eq!(state.tick_budget.remaining(Kind::Decision), 0);
    assert!(state.tick_budget.remaining(Kind::PathNode) < nodes);
}

/// A real village streets/bridge flooded by spring water left every resident
/// without a single legal step: the route layer treated any water in the body
/// as a wall. Vanilla ground navigation wades; only head-deep water and lava
/// remain barriers.
#[test]
fn routes_wade_through_foot_deep_water_but_never_deeper_or_lava() {
    let route = |fluid: &str, depth: i32| -> bool {
        let mut state = test_state();
        let id = block(&state, fluid, &[]);
        for z in 0..16 {
            for x in 6..8 {
                for y in 201..201 + depth {
                    state.world.set_block(x, y, z, id);
                }
            }
        }
        state.world.fold_overlay();
        let mut cache = PoiCache::default();
        let mut path = Vec::new();
        let mut ctx = RouteCtx::tick(&mut state, Consumer::Brain);
        find_path_exact_ctx(
            &mut ctx,
            &mut cache,
            [2.5, 201.0, 8.5],
            BlockPos {
                x: 10,
                y: 201,
                z: 8,
            },
            0.3,
            1.95,
            &mut path,
        )
    };
    assert!(route("minecraft:water", 1), "one-block ford is walkable");
    assert!(!route("minecraft:water", 2), "head-deep water is a barrier");
    assert!(!route("minecraft:lava", 1), "lava is never walkable");
}

/// Seed 87122 replay: a farmer standing on the rim of an irrigation channel has
/// water under its start cell. Routes began from the cell centre, whose support
/// row is that water, so every first step was refused and the farmer never moved.
#[test]
fn route_starts_from_the_real_body_when_the_start_cell_is_supported_by_water() {
    let mut state = test_state();
    let water = block(&state, "minecraft:water", &[]);
    let farmland = block(&state, "minecraft:farmland", &[("moisture", "7")]);
    state.world.set_block(8, 200, 8, water);
    state.world.set_block(9, 200, 8, farmland);
    state.world.set_block(7, 200, 8, farmland);
    state.world.fold_overlay();
    let mut cache = PoiCache::default();
    let mut path = Vec::new();
    let mut ctx = RouteCtx::tick(&mut state, Consumer::Brain);
    // Body straddles the channel and rests on the farmland rim (top 15/16).
    let found = find_path_ctx(
        &mut ctx,
        &mut cache,
        [8.8, 200.9375, 8.5],
        BlockPos {
            x: 13,
            y: 201,
            z: 8,
        },
        0.3,
        1.95,
        &mut path,
        false,
        48,
        16,
        true,
    );
    assert!(found, "a body that physically stands there can leave");
}

/// Seed 87122 replay: the cleric walked onto a platform and its route dropped
/// three blocks "off the edge" into a column that the platform itself covers,
/// so it pushed against the stone forever. A drop must have a clear shaft.
#[test]
fn route_never_drops_through_an_overhang_into_a_sealed_pocket() {
    let mut state = test_state();
    let stone = state.registry.states.stone;
    // Pillar (5,201..203) and overhang (6,203): standing surface at y=204.
    for y in 201..=203 {
        state.world.set_block(5, y, 8, stone);
    }
    state.world.set_block(6, 203, 8, stone);
    // Sealed pocket under the overhang: walls on every side but the ceiling.
    for y in 201..=202 {
        for (x, z) in [(7, 8), (6, 7), (6, 9)] {
            state.world.set_block(x, y, z, stone);
        }
    }
    state.world.fold_overlay();
    let mut cache = PoiCache::default();
    let mut path = Vec::new();
    let mut ctx = RouteCtx::tick(&mut state, Consumer::Brain);
    let found = find_path_exact_ctx(
        &mut ctx,
        &mut cache,
        [6.5, 204.0, 8.5],
        BlockPos { x: 6, y: 201, z: 8 },
        0.3,
        1.95,
        &mut path,
    );
    assert!(!found, "the pocket is only reachable through solid stone");
}

/// A one-block jump rises 1.25 blocks. When only the ledge's column is roofed
/// two blocks over the ledge, the head stops the body above the ledge edge
/// from entering it and the resident jumped in place forever. A roof over the
/// departure column too caps the rise to the narrow window just above the
/// edge (a one-block mine staircase), which does cross.
#[test]
fn route_never_jumps_onto_a_ledge_roofed_below_the_jump_apex() {
    let route = |roof: Option<i32>, over_departure: bool| {
        let mut state = test_state();
        let stone = state.registry.states.stone;
        // Ledge (8,201,9)-(8,201,10): standing surface y=202.
        for z in 9..=10 {
            state.world.set_block(8, 201, z, stone);
        }
        if let Some(y) = roof {
            for z in 9..=10 {
                state.world.set_block(8, y, z, stone);
            }
            if over_departure {
                state.world.set_block(8, y, 8, stone);
            }
        }
        state.world.fold_overlay();
        let mut cache = PoiCache::default();
        let mut path = Vec::new();
        let mut ctx = RouteCtx::tick(&mut state, Consumer::Brain);
        find_path_exact_ctx(
            &mut ctx,
            &mut cache,
            [8.5, 201.0, 8.5],
            BlockPos {
                x: 8,
                y: 202,
                z: 10,
            },
            0.3,
            1.95,
            &mut path,
        )
    };
    assert!(route(None, false), "an open ledge is a jump up");
    assert!(route(Some(205), false), "a roof with headroom for the jump");
    assert!(
        !route(Some(204), false),
        "a roof two blocks over the ledge stops the jump short of the edge"
    );
    assert!(
        route(Some(204), true),
        "a staircase roofed over both columns caps the rise inside the crossing window"
    );
}

/// Seed 87122 claim QA: a builder in a crater jumped at a two-block wall for
/// ten minutes. Every replan started from the cell above its hovering body, so
/// the wall top looked one block up and the route the floor cannot make was
/// accepted again. A body mid-jump routes from the floor it left.
#[test]
fn route_from_a_jumping_body_starts_at_its_floor() {
    let route = |wall: i32, feet: f64| {
        let mut state = test_state();
        let stone = state.registry.states.stone;
        // Wall column (8,201..) on a floor with standing surface y=201.
        for y in 201..201 + wall {
            state.world.set_block(8, y, 9, stone);
        }
        state.world.fold_overlay();
        let mut cache = PoiCache::default();
        let mut path = Vec::new();
        let mut ctx = RouteCtx::tick(&mut state, Consumer::Brain);
        find_path_exact_ctx(
            &mut ctx,
            &mut cache,
            [8.5, feet, 8.5],
            BlockPos {
                x: 8,
                y: 201 + wall,
                z: 9,
            },
            0.3,
            1.95,
            &mut path,
        )
    };
    for feet in [201.0, 201.42, 202.04, 202.25] {
        assert!(route(1, feet), "a one-block step is a jump from {feet}");
        assert!(
            !route(2, feet),
            "a two-block wall cannot be climbed from a body at {feet}"
        );
    }
}

/// A centre resident in the seed-100 village repeatedly exhausted the 4096
/// node allowance while approaching a workstation only 20 blocks away.  A
/// long fence-lined street and a one-door house make an unweighted A* flood
/// the west side of the barrier before it follows the available detour.
#[test]
fn route_to_nearby_workstation_survives_cluttered_village_node_budget() {
    let mut state = test_state();
    let wanted = (-3..=3)
        .flat_map(|x| (-3..=3).map(move |z| crate::boundary::ChunkPos { x, z }))
        .map(|chunk| (chunk, 0))
        .collect();
    state.world.ensure_blocking(&wanted);
    for x in -48..64 {
        for z in -48..64 {
            state
                .world
                .set_block(x, 200, z, state.registry.states.stone);
            for y in 201..206 {
                state.world.set_block(x, y, z, state.registry.states.air);
            }
        }
    }

    let stone = state.registry.states.stone;
    let fence = block(&state, "minecraft:oak_fence", &[]);
    let water = block(&state, "minecraft:water", &[]);
    let wheat = block(&state, "minecraft:wheat", &[("age", "7")]);
    // The only crossing is far south.  Fences, crop rows and flooded plots
    // leave the route recognisably village-like without closing that crossing.
    for z in -40..51 {
        if z != 36 {
            for y in 201..204 {
                state.world.set_block(12, y, z, stone);
            }
        }
    }
    for x in -30..-18 {
        for z in -30..-18 {
            for y in 201..203 {
                state.world.set_block(x, y, z, water);
            }
        }
    }
    for x in -16..8 {
        state.world.set_block(x, 201, 28, fence);
        state.world.set_block(x, 201, 30, wheat);
    }
    // The target is inside a house entered from its east-side door after the
    // street detour.
    for x in 18..27 {
        for z in 4..13 {
            if (x == 18 || x == 26 || z == 4 || z == 12) && (x, z) != (26, 10) {
                for y in 201..204 {
                    state.world.set_block(x, y, z, stone);
                }
            }
        }
    }
    let cauldron = block(&state, "minecraft:cauldron", &[]);
    let target = BlockPos {
        x: 22,
        y: 201,
        z: 8,
    };
    state
        .world
        .set_block(target.x, target.y, target.z, cauldron);
    state.world.fold_overlay();

    let mut cache = PoiCache::default();
    let mut path = Vec::new();
    assert!(
        find_path(
            &mut state,
            &mut cache,
            [2.5, 201.0, 8.5],
            target,
            0.3,
            1.95,
            &mut path,
        ),
        "a nearby POI behind a cluttered village street must fit one tick's path allowance"
    );
    assert!(!path.is_empty());
    assert!(
        state.tick_budget.remaining(Kind::PathNode) > 0,
        "the route must leave shared path-node capacity for other residents"
    );
}

#[test]
fn allocator_noop_validation_and_empty_search_leave_journal_rows_quiet() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let id = state.entities.body(slot).id;
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    state.entities.take_touched();
    validate_changed_claims(&mut state, &mut cache);
    assert!(
        state.entities.take_touched().is_empty(),
        "unchanged validation must not take row_mut"
    );
    cache.search_at.insert(id, state.tick);
    acquire_pois(&mut state, &mut cache);
    assert!(
        state.entities.take_touched().is_empty(),
        "empty search must not take row_mut"
    );
}

#[test]
fn allocator_in_place_validation_covers_collision_and_destruction() {
    let mut state = test_state();
    let home = bed(&mut state, 8, 8);
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(4, 201, 4, composter);
    state.world.fold_overlay();
    let me = villager(&mut state, [8.5, 201.0, 8.5]);
    let rival = villager(&mut state, [9.5, 201.0, 8.5]);
    let rival_id = state.entities.body(rival).id;
    // Seed identical claims, then index one owner as the claim winner.
    let mut cache = PoiCache::default();
    cache.update_claims(
        state.entities.body(me).id,
        [None; 3],
        [Some(home), Some(BlockPos { x: 4, y: 201, z: 4 }), None],
    );
    cache.refresh(&state);
    for slot in [me, rival] {
        let v = state
            .entities
            .row_mut_unmarked(slot)
            .villager
            .as_mut()
            .unwrap();
        v.home = Some(home);
        v.job_site = Some(BlockPos { x: 4, y: 201, z: 4 });
        v.data.profession = Profession::Farmer;
    }
    state.entities.take_touched();
    // The indexed owner validates cache-only (no mark); the colliding rival
    // releases same-tick and marks its row alone.
    validate_changed_claims(&mut state, &mut cache);
    assert_eq!(state.entities.take_touched(), vec![rival_id]);
    let mine = state.entities.row(me).villager.as_ref().unwrap();
    assert_eq!(mine.home, Some(home));
    assert_eq!(mine.job_site, Some(BlockPos { x: 4, y: 201, z: 4 }));
    // Collision: the rival releases same-tick and the winner keeps custody.
    assert_eq!(
        state.entities.row(rival).villager.as_ref().unwrap().home,
        None
    );
    assert!(
        !cache.claimed(
            state.entities.body(me).id,
            Poi {
                pos: home,
                kind: PoiKind::Home
            }
        ),
        "winner keeps its claim"
    );
    // Destroyed bed/workstation invalidates; traded profession survives.
    state.entities.row_mut(me).villager.as_mut().unwrap().xp = 1;
    state
        .world
        .set_block(home.x, home.y, home.z, state.registry.states.stone);
    state
        .world
        .set_block(4, 201, 4, state.registry.states.stone);
    state.world.fold_overlay();
    cache.refresh(&state);
    state.entities.take_touched();
    validate_changed_claims(&mut state, &mut cache);
    let v = state.entities.row(me).villager.as_ref().unwrap();
    assert_eq!(v.home, None);
    assert_eq!(v.job_site, None);
    assert_eq!(v.data.profession, Profession::Farmer);
    assert!(!state.entities.take_touched().is_empty());
}

#[test]
fn unrelated_activity_change_does_not_refresh_every_resident() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let id = state.entities.body(slot).id;
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    let mut changed = FxHashSet::default();
    state.entities.swap_brain_changes(&mut changed);
    let unrelated = ChunkPos { x: 100, z: 100 };
    state.world.set_active_chunks(
        [ChunkPos { x: 0, z: 0 }, unrelated].into_iter().collect(),
        state.tick + 1,
    );
    cache.refresh(&state);
    assert!(
        !cache.updates.contains(&id),
        "a global activity fence must not dirty residents of unchanged chunks"
    );
}

#[test]
fn bound_resource_roles_keep_their_profession_while_other_roles_can_retarget() {
    for profession in [
        Profession::Farmer,
        Profession::Shepherd,
        Profession::Miner,
        Profession::Lumberjack,
        Profession::Builder,
        Profession::None,
        Profession::Mason,
    ] {
        let (mut state, settlement, _) = crate::settlements::construction::tests::fixture();
        let station = block(&state, "minecraft:blast_furnace", &[]);
        state.world.set_block(8, 201, 8, station);
        crate::settlements::buildings::certify(
            &mut state,
            settlement,
            crate::settlements::buildings::BuildingKind::Workshop(Profession::Armorer),
            mc_script::ScriptWorldBounds::new([7, 200, 7], [9, 203, 9]).unwrap(),
        )
        .unwrap();
        state.world.fold_overlay();
        let slot = villager(&mut state, [6.5, 201.0, 8.5]);
        let entity = state.entities.body(slot).id;
        crate::settlements::bind(&mut state, settlement, entity).unwrap();
        state
            .entities
            .row_mut(slot)
            .villager
            .as_mut()
            .unwrap()
            .data
            .profession = profession;
        for _ in 0..1500 {
            step(&mut state);
        }
        let resident = state.entities.row(slot).villager.as_ref().unwrap();
        if matches!(profession, Profession::None | Profession::Mason) {
            assert_eq!(resident.data.profession, Profession::Armorer);
            assert_eq!(resident.job_site, Some(BlockPos { x: 8, y: 201, z: 8 }));
        } else {
            assert_eq!(
                resident.data.profession, profession,
                "preserve {profession:?}"
            );
            assert!(resident.job_site.is_none(), "do not take a foreign slot");
        }
    }
}

fn far_workshop_fixture() -> (TickState, SettlementId, EntitySlot, BlockPos) {
    let mut state = test_state();
    let chunks = (1..=4).map(|x| (ChunkPos { x, z: 0 }, 0)).collect();
    state.world.ensure_blocking(&chunks);
    for x in 16..80 {
        for z in 0..16 {
            state
                .world
                .set_block(x, 200, z, state.registry.states.stone);
            for y in 201..206 {
                state.world.set_block(x, y, z, state.registry.states.air);
            }
        }
    }
    state
        .tickets
        .insert("far-workshop".into(), (ChunkPos { x: 2, z: 0 }, 2));
    state.refresh_active_chunks(0);
    state.day_time = 3000;
    let settlement = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "far-workshop".into(),
            "owner".into(),
            [8, 201, 8],
            80,
        )
        .unwrap();
    state.settlements.get_mut(settlement).unwrap().owner = None;
    let job = BlockPos {
        x: 60,
        y: 201,
        z: 8,
    };
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(job.x, job.y, job.z, composter);
    crate::settlements::buildings::certify(
        &mut state,
        settlement,
        crate::settlements::buildings::BuildingKind::Workshop(Profession::Farmer),
        mc_script::ScriptWorldBounds::new([59, 200, 7], [61, 203, 9]).unwrap(),
    )
    .unwrap();
    state.world.fold_overlay();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let entity = state.entities.body(slot).id;
    crate::settlements::bind(&mut state, settlement, entity).unwrap();
    (state, settlement, slot, job)
}

#[test]
fn bound_resident_claims_certified_work_beyond_vanilla_poi_reach() {
    let (mut state, _, slot, job) = far_workshop_fixture();
    for _ in 0..400 {
        state.tick_budget.reset(state.tick);
        tick(&mut state, &mut ChangeSet::default());
        state.tick += 1;
        if state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .job_site
            .is_some()
        {
            break;
        }
    }
    let resident = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(resident.job_site, Some(job));
    assert_eq!(resident.navigation_target, Some(job));
    assert!(!resident.navigation_path.is_empty());
    assert_eq!(state.entities.pos(slot), [8.5, 201.0, 8.5]);
}

#[test]
fn unbound_resident_does_not_claim_work_beyond_vanilla_poi_reach() {
    let (mut state, _, slot, _) = far_workshop_fixture();
    let entity = state.entities.body(slot).id;
    crate::settlements::unbind(&mut state, entity).unwrap();
    for _ in 0..400 {
        state.tick_budget.reset(state.tick);
        tick(&mut state, &mut ChangeSet::default());
        state.tick += 1;
    }
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
}

#[test]
fn far_farmer_walks_to_work_and_returns_surplus_to_founding_chest() {
    let (mut state, settlement, slot, job) = far_workshop_fixture();
    let chest = block(&state, "minecraft:chest", &[]);
    state.world.set_block(5, 201, 8, chest);
    state.block_entities.insert(
        (5, 201, 8),
        crate::tick::BlockEntityState {
            slots: vec![mc_data::ItemStack::EMPTY; 27],
            furnace: None,
        },
    );
    state
        .settlements
        .get_mut(settlement)
        .unwrap()
        .bootstrap_storage = Some([5, 201, 8]);
    let crop = block(&state, "minecraft:wheat", &[("age", "7")]);
    state.world.set_block(60, 201, 9, crop);
    state.world.fold_overlay();
    let [bread, wheat] = ["minecraft:bread", "minecraft:wheat"].map(|name| {
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .unwrap()
    });
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .inventory[0] = Some(mc_data::ItemStack::new(bread, 3));
    let mut reached_work = false;
    for _ in 0..4000 {
        step(&mut state);
        let resident = state.entities.row(slot).villager.as_ref().unwrap();
        reached_work |=
            resident.data.profession == Profession::Farmer && resident.job_site == Some(job);
        if state.block_entities[&(5, 201, 8)]
            .slots
            .iter()
            .any(|stack| stack.item_id == wheat && stack.count > 0)
        {
            break;
        }
        // Domain navigation assistance follows only the brain's admitted
        // route; never teleport directly to a job or delivery target.
        let next = state
            .entities
            .row_mut(slot)
            .villager
            .as_mut()
            .unwrap()
            .navigation_path
            .pop();
        if let Some(next) = next {
            state.entities.set_pos(
                slot,
                [
                    f64::from(next.x) + 0.5,
                    f64::from(next.y),
                    f64::from(next.z) + 0.5,
                ],
            );
        }
    }
    assert!(
        reached_work,
        "the untrained resident must reach its certified far job"
    );
    assert!(
        state.block_entities[&(5, 201, 8)]
            .slots
            .iter()
            .any(|stack| stack.item_id == wheat && stack.count > 0),
        "real harvested grain must reach the empty physical founding chest"
    );
    assert_eq!(
        state.world.block_at(60, 201, 9),
        Some(block(&state, "minecraft:wheat", &[("age", "0")]))
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
            .filter(|stack| stack.item_id == bread)
            .map(|stack| stack.count)
            .sum::<i32>(),
        3,
        "delivery preserves the breeding ration",
    );
}

fn scarce_path_nodes(state: &mut TickState, tick: u64) {
    state.tick = tick;
    state.tick_budget.reset(tick);
    let allowance =
        state
            .tick_budget
            .allowance(state.dimension, Consumer::Allocator, Kind::PathNode);
    assert!(state.tick_budget.try_charge(
        state.dimension,
        Consumer::Allocator,
        Kind::PathNode,
        allowance.saturating_sub(24),
    ));
}

fn blocked_home_with_resource_job() -> (TickState, EntitySlot, BlockPos) {
    let mut state = test_state();
    let home = bed(&mut state, 5, 5);
    for x in 3..=7 {
        for z in 3..=7 {
            for y in 201..=204 {
                state.world.set_block(x, y, z, state.registry.states.stone);
            }
        }
    }
    assert_eq!(bed(&mut state, 5, 5), home);
    let settlement = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "resource_jobs".into(),
            "owner".into(),
            [8, 201, 8],
            16,
        )
        .unwrap();
    let job = BlockPos {
        x: 13,
        y: 201,
        z: 10,
    };
    let building = super::super::buildings::BuildingId((settlement.0 << 32) | 1);
    let record = state.settlements.get_mut(settlement).unwrap();
    record.building_sequence = 1;
    record.buildings.insert(
        building,
        super::super::buildings::Building {
            id: building,
            settlement,
            kind: super::super::buildings::BuildingKind::LumberCamp,
            level: 1,
            bounds: mc_script::ScriptWorldBounds::new([12, 200, 9], [14, 204, 11]).unwrap(),
            work_points: vec![[job.x, job.y, job.z]],
            revision: 1,
            natural_zone: true,
        },
    );
    let slot = villager(&mut state, [1.5, 201.0, 9.5]);
    let entity = state.entities.body(slot).id;
    super::super::bind(&mut state, settlement, entity).unwrap();
    state.world.fold_overlay();
    (state, slot, job)
}

#[test]
fn settlement_without_storage_fills_a_reachable_resource_job_before_a_nearby_workshop() {
    let (mut state, slot, resource_job) = blocked_home_with_resource_job();
    state.day_time = 2000;
    let settlement = state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .unwrap()
        .settlement
        .unwrap();
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(2, 201, 8, composter);
    state.world.fold_overlay();
    let id = super::super::buildings::BuildingId((settlement.0 << 32) | 2);
    let record = state.settlements.get_mut(settlement).unwrap();
    record.building_sequence = 2;
    record.buildings.insert(
        id,
        super::super::buildings::Building {
            id,
            settlement,
            kind: super::super::buildings::BuildingKind::Workshop(Profession::Farmer),
            level: 1,
            bounds: mc_script::ScriptWorldBounds::new([1, 200, 7], [2, 203, 8]).unwrap(),
            work_points: vec![[2, 201, 8]],
            revision: 1,
            natural_zone: false,
        },
    );
    for _ in 0..60 {
        step(&mut state);
    }
    assert_eq!(
        state.entities.row(slot).villager.as_ref().unwrap().job_site,
        Some(resource_job)
    );
}

#[test]
fn exhausted_home_search_does_not_starve_reachable_resource_job() {
    let (mut state, slot, job) = blocked_home_with_resource_job();
    state.day_time = 2000;
    for time in 0..40 {
        scarce_path_nodes(&mut state, time);
        tick(&mut state, &mut ChangeSet::default());
        if state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .job_site
            .is_some()
        {
            break;
        }
    }
    let resident = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(resident.home, None, "the enclosed bed stays unreachable");
    assert_eq!(
        resident.job_site,
        Some(job),
        "a budget-denied home route must not monopolize every subsequent job search"
    );
}

#[test]
fn exhausted_nearest_job_search_retries_another_certified_candidate() {
    let (mut state, slot, job) = blocked_home_with_resource_job();
    let mut cache = PoiCache::default();
    cache.sync_search(&state, slot);
    let mut resident = state.entities.row_mut(slot).villager.take().unwrap();
    let settlement = resident.settlement.unwrap();
    let blocked = BlockPos { x: 5, y: 201, z: 5 };
    let stonecutter = block(&state, "minecraft:stonecutter", &[]);
    state
        .world
        .set_block(blocked.x, blocked.y, blocked.z, stonecutter);
    let id = super::super::buildings::BuildingId((settlement.0 << 32) | 2);
    state
        .settlements
        .get_mut(settlement)
        .unwrap()
        .buildings
        .insert(
            id,
            super::super::buildings::Building {
                id,
                settlement,
                kind: super::super::buildings::BuildingKind::Workshop(Profession::Mason),
                level: 1,
                bounds: mc_script::ScriptWorldBounds::new([3, 200, 3], [7, 204, 7]).unwrap(),
                work_points: vec![[blocked.x, blocked.y, blocked.z]],
                revision: 1,
                natural_zone: false,
            },
        );
    let mut acquired = None;
    for attempt in 0..4 {
        scarce_path_nodes(&mut state, attempt);
        acquired = acquire(
            &mut state,
            &mut cache,
            slot,
            &mut resident,
            PoiKind::Job(Profession::None),
        );
        if acquired.is_some() {
            break;
        }
    }
    assert_eq!(
        acquired,
        Some(job),
        "retry must not restart the same budget-exhausting nearest workstation forever"
    );
}
