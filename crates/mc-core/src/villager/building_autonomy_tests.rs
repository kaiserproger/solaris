//! Domain end-to-end closure: generated village provenance, empty founding
//! storage, real renewable/extracted stock, vanilla crafting and native builds.
//! Movement follows admitted navigation targets; graphical movement is a
//! separate integration gate, not claimed by this deterministic unit fixture.
use super::{VillagerProfession, generated};
use crate::boundary::{ChangeSet, ChunkPos};
use crate::registry::CoreRegistry;
use crate::settlements::buildings::BuildingKind;
use crate::tick::TickState;
use std::sync::Arc;

struct VillageGenerator {
    terrain: mc_worldgen::TerrainGenerator,
    registry: Arc<CoreRegistry>,
    sparse: bool,
}

impl VillageGenerator {
    fn block(&self, name: &str, props: &[(&str, &str)]) -> mc_world::BlockStateId {
        let name = mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap();
        let default = self.registry.blocks.block(&name).unwrap().default;
        let mut values = self
            .registry
            .blocks
            .by_id(default)
            .unwrap()
            .properties
            .clone();
        for (key, value) in props {
            if let Some((_, current)) = values.iter_mut().find(|(property, _)| property == key) {
                *current = (*value).into();
            }
        }
        self.registry
            .blocks
            .by_name_and_props(&name, &values)
            .unwrap()
    }
}

impl mc_world::ChunkGenerator for VillageGenerator {
    fn generate(&self, position: mc_world::ChunkPos) -> mc_world::Chunk {
        let mut chunk = mc_world::ChunkGenerator::generate(&self.terrain, position);
        let air = mc_world::BlockStateId(self.registry.states.air);
        let stone = mc_world::BlockStateId(self.registry.states.stone);
        let soil = self.block("grass_block", &[]);
        let coal = self.block("coal_ore", &[]);
        let log = self.block("oak_log", &[]);
        let leaves = self.block("oak_leaves", &[("persistent", "false"), ("distance", "1")]);
        for x in 0..16_u8 {
            for z in 0..16_u8 {
                let wx = position.x * 16 + i32::from(x);
                let wz = position.z * 16 + i32::from(z);
                for y in 188..200 {
                    let material = if !self.sparse && wx >= 16 && (wx + wz + y).rem_euclid(5) == 0 {
                        coal
                    } else {
                        stone
                    };
                    chunk.set_block(x, y, z, material);
                }
                chunk.set_block(
                    x,
                    200,
                    z,
                    if self.sparse && wx >= 16 { stone } else { soil },
                );
                for y in 201..225 {
                    chunk.set_block(x, y, z, air);
                }
                if self.sparse && [wx, wz] == [10, -1] {
                    chunk.set_block(x, 201, z, stone);
                    chunk.set_block(x, 202, z, stone);
                }
                if (-15..=-1).contains(&wx)
                    && (0..=14).contains(&wz)
                    && (wx + 14).rem_euclid(3) == 0
                    && (wz - 1).rem_euclid(3) == 0
                    && (!self.sparse || wz == 1)
                {
                    for y in 201..211 {
                        chunk.set_block(x, y, z, log);
                    }
                    chunk.set_block(x, 211, z, leaves);
                }
                if (2..=13).contains(&wx) && (-15..=-3).contains(&wz) {
                    chunk.set_block(x, 200, z, self.block("farmland", &[("moisture", "7")]));
                    chunk.set_block(x, 201, z, self.block("wheat", &[("age", "7")]));
                }
                if [wx, wz] == [5, -7] {
                    chunk.set_block(x, 201, z, self.block("composter", &[]));
                }
                if [wx, wz] == [0, 0] {
                    chunk.set_block(x, 201, z, self.block("bell", &[]));
                }
            }
        }
        if position.x == 0 && position.z == 0 {
            let professions = ["none", "none", "none", "farmer", "shepherd", "mason"];
            let markers: Vec<_> = professions.iter().enumerate().map(|(index, profession)| mc_world::GeneratedEntityMarker {
                claim: format!("minecraft:village/plains/town_centers/plains_meeting_point_1@0:200:0#{index}"),
                entity_type: "minecraft:villager".into(), position: [index as f64 + 0.5, 201.0, 2.5],
                persistent: false,
                villager_kind: "minecraft:plains".into(), profession: format!("minecraft:{profession}"),
                level: 1, age: 0, yaw: 0.0, pitch: 0.0,
            }).collect();
            chunk.set_generated_entities(&markers);
        }
        chunk
    }

    fn surface_height(&self, _x: i32, _z: i32) -> Option<i32> {
        Some(200)
    }

    fn village_sites_in_region(
        &self,
        min: (i32, i32),
        max: (i32, i32),
    ) -> Vec<mc_world::GeneratedVillageSite> {
        if min.0 > 0 || min.1 > 0 || max.0 < 0 || max.1 < 0 {
            return Vec::new();
        }
        vec![mc_world::GeneratedVillageSite {
            start_chunk: (0, 0),
            structure: mc_data::Identifier::parse("minecraft:village_plains").expect("static id"),
            min: mc_world::BlockPos {
                x: -16,
                y: 188,
                z: -16,
            },
            max: mc_world::BlockPos {
                x: 31,
                y: 224,
                z: 31,
            },
            pieces: vec![
                mc_world::GeneratedVillagePiece {
                    template: Some(
                        "minecraft:village/plains/town_centers/plains_meeting_point_1".into(),
                    ),
                    position: mc_world::BlockPos { x: 0, y: 200, z: 0 },
                    rotation: 0,
                    entity_owner: Some(
                        "minecraft:village/plains/town_centers/plains_meeting_point_1".into(),
                    ),
                    bounds: Some((
                        mc_world::BlockPos {
                            x: -1,
                            y: 200,
                            z: -1,
                        },
                        mc_world::BlockPos { x: 1, y: 203, z: 1 },
                    )),
                },
                mc_world::GeneratedVillagePiece {
                    template: Some("minecraft:village/plains/houses/plains_large_farm_1".into()),
                    position: mc_world::BlockPos {
                        x: 0,
                        y: 200,
                        z: -16,
                    },
                    rotation: 0,
                    entity_owner: None,
                    bounds: Some((
                        mc_world::BlockPos {
                            x: 0,
                            y: 200,
                            z: -16,
                        },
                        mc_world::BlockPos {
                            x: 14,
                            y: 203,
                            z: -3,
                        },
                    )),
                },
            ],
        }]
    }
}

fn village_with(sparse: bool) -> TickState {
    let mut state = super::conversion_tests::state();
    let registry = Arc::clone(&state.registry);
    state.world = crate::world::ChunkWorld::new(
        42,
        Arc::new(VillageGenerator {
            terrain: mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks)),
            registry,
            sparse,
        }),
        &state.registry,
        mc_world::BlockStateId(state.registry.states.air),
        [0, 201, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let chunks: Vec<_> = (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos { x, z }))
        .collect();
    state
        .world
        .ensure_blocking(&chunks.iter().map(|chunk| (*chunk, 0)).collect());
    for (index, chunk) in chunks.iter().enumerate() {
        state
            .tickets
            .insert(format!("autonomous-fixture-{index}"), (*chunk, 0));
    }
    state.refresh_active_chunks(0);
    let mut changes = ChangeSet::default();
    generated::load(&mut state, &chunks, &mut changes);
    changes.chunks_loaded.extend(chunks);
    crate::settlements::autonomy::tick(&mut state, &mut changes);
    let sheep = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:sheep").unwrap())
        .unwrap();
    for x in 8..11 {
        let entity = state.spawn_mob(sheep, [f64::from(x) + 0.5, 201.0, 8.5], 8.0);
        let slot = state.entities.slot_of(entity).unwrap();
        // Three real white sheep guarantee the authored bed's wool color,
        // independent of UUID-based natural color rolls; no wool is supplied.
        crate::tick::set_wool(
            Arc::make_mut(&mut state.entities.row_mut(slot).appearance),
            0,
        );
    }
    state
}

fn village() -> TickState {
    village_with(false)
}
fn seed_100_village() -> TickState {
    let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let mut config = crate::config::CoreConfig::new(std::path::PathBuf::new(), 100);
    config.vanilla_data_dir = Some(data.clone());
    let registry = Arc::new(CoreRegistry::load(Some(&data)).unwrap());
    let terrain = crate::core::overworld_terrain(&config, &registry).unwrap();
    let world = crate::world::ChunkWorld::new(
        100,
        Arc::new(terrain),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [128, 70, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 4, 512);
    // The client has loaded the outskirts, but only the 4-chunk simulation
    // square around its final position can tick villagers and resource work.
    let chunks: Vec<_> = (2..=11)
        .flat_map(|x| (-4..=4).map(move |z| ChunkPos { x, z }))
        .collect();
    state
        .world
        .ensure_blocking(&chunks.iter().map(|p| (*p, 0)).collect());
    for (index, chunk) in chunks.iter().filter(|p| p.x >= 3).enumerate() {
        state
            .tickets
            .insert(format!("seed100-{index}"), (*chunk, 0));
    }
    state.refresh_active_chunks(0);
    let mut changes = ChangeSet::default();
    generated::load(&mut state, &chunks, &mut changes);
    changes.chunks_loaded.extend(chunks);
    crate::settlements::autonomy::tick(&mut state, &mut changes);
    state
}

#[test]
#[ignore = "requires the local vanilla 26.1.2 content cache"]
fn seed_100_west_residents_can_route_to_founding_storage() {
    use crate::boundary::BlockPos;
    use crate::settlements::allocator::{PoiCache, RouteCtx, find_path_ctx};
    use crate::tick_budget::{Budget, Consumer};

    let state = seed_100_village();
    for start in [[95.5, 63.0, 5.5], [86.5, 63.0, 10.5]] {
        let mut budget = Budget::default();
        let mut ctx = RouteCtx {
            world: &state.world,
            registry: &state.registry,
            dimension: state.dimension,
            consumer: Consumer::Allocator,
            budget: &mut budget,
        };
        let mut cache = PoiCache::default();
        let mut path = Vec::new();
        assert!(ctx.admit());
        assert!(
            find_path_ctx(
                &mut ctx,
                &mut cache,
                start,
                BlockPos {
                    x: 117,
                    y: 70,
                    z: 5
                },
                0.3,
                1.95,
                &mut path,
                false,
                48,
                16,
                true,
            ),
            "west resident at {start:?} cannot reach storage; partial path: {path:?}"
        );
    }
}

#[test]
#[ignore = "requires the local vanilla 26.1.2 content cache"]
fn seed_100_west_unemployed_resident_is_assigned_to_funded_storage() {
    let mut state = seed_100_village();
    let id = state.settlements.records().next().unwrap().id;
    let chest = state
        .settlements
        .get(id)
        .unwrap()
        .bootstrap_storage
        .unwrap();
    assert_eq!(chest, [117, 70, 5]);
    let west = crate::settlements::allocator::tests::villager(&mut state, [86.5, 63.0, 10.5]);
    let central = crate::settlements::allocator::tests::villager(&mut state, [118.5, 70.0, 5.5]);
    for slot in [west, central] {
        let entity = state.entities.body(slot).id;
        crate::settlements::bind(&mut state, id, entity).unwrap();
    }
    state
        .entities
        .row_mut(central)
        .villager
        .as_mut()
        .unwrap()
        .data
        .profession = VillagerProfession::Farmer;
    let stock = &mut state
        .block_entities
        .get_mut(&(chest[0], chest[1], chest[2]))
        .unwrap()
        .slots;
    for (index, (name, count)) in [
        ("cobblestone", 64),
        ("cobblestone", 50),
        ("acacia_log", 43),
        ("coal", 2),
    ]
    .into_iter()
    .enumerate()
    {
        let item = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
            .unwrap();
        stock[index] = mc_data::ItemStack::new(item, count);
    }
    state.day_time = 3000;
    for tick in (0..4000).step_by(20) {
        state.tick = tick;
        crate::settlements::construction::tick(&mut state);
        let assigned = state
            .entities
            .row(west)
            .villager
            .as_ref()
            .unwrap()
            .data
            .profession
            == VillagerProfession::Builder;
        let started = state
            .settlements
            .get(id)
            .unwrap()
            .project
            .as_ref()
            .is_some_and(|project| project.blueprint == "storage" && project.level == 1);
        if assigned && started {
            return;
        }
    }
    panic!(
        "west unemployed resident was not assigned to funded Storage L1; project={:?}",
        state.settlements.get(id).unwrap().project
    );
}

#[test]
#[ignore = "requires the local vanilla 26.1.2 content cache"]
fn seed_100_farmer_delivers_from_west_bridge() {
    let mut state = seed_100_village();
    let id = state.settlements.records().next().unwrap().id;
    let chest = state
        .settlements
        .get(id)
        .unwrap()
        .bootstrap_storage
        .unwrap();
    let slot = crate::settlements::allocator::tests::villager(&mut state, [95.5, 63.0, 5.5]);
    let entity = state.entities.body(slot).id;
    crate::settlements::bind(&mut state, id, entity).unwrap();
    let carrot = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:carrot").unwrap())
        .unwrap();
    let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
    v.data.profession = VillagerProfession::Farmer;
    v.job_site = Some(crate::boundary::BlockPos { x: 66, y: 69, z: 3 });
    v.inventory[0] = Some(mc_data::ItemStack::new(carrot, 2));
    state.day_time = 3000;
    for _ in 0..4000 {
        crate::tick::run_tick(&mut state, &[]);
        if state.block_entities[&(chest[0], chest[1], chest[2])]
            .slots
            .iter()
            .any(|stack| stack.item_id == carrot && stack.count >= 2)
        {
            return;
        }
    }
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    panic!(
        "farmer failed to deliver: pos={:?} target={:?} path={} inventory={:?}",
        state.entities.pos(slot),
        v.navigation_target,
        v.navigation_path.len(),
        v.inventory
    );
}

#[test]
#[ignore = "requires the local vanilla 26.1.2 content cache"]
fn seed_100_generated_village_builds_storage_with_player_simulation_distance() {
    let mut state = seed_100_village();
    for _ in 0..12_000 {
        crate::tick::run_tick(&mut state, &[]);
        for record in state.settlements.records() {
            for building in record.buildings.values().filter(|b| b.natural_zone) {
                let [x, _, z] = building.work_points[0];
                assert!(
                    state.world.is_active(x.div_euclid(16), z.div_euclid(16)),
                    "natural {:?} work point {:?} lies outside simulation",
                    building.kind,
                    building.work_points[0]
                );
            }
        }
        if state.settlements.records().any(|record| {
            record
                .buildings
                .values()
                .any(|building| building.kind == BuildingKind::Storage)
        }) {
            return;
        }
    }
    let record = state
        .settlements
        .records()
        .next()
        .expect("generated village");
    let residents: Vec<_> = state
        .entities
        .slots()
        .filter_map(|slot| {
            let v = state.entities.row(slot).villager.as_ref()?;
            (v.settlement == Some(record.id)).then(|| {
                (
                    state.entities.pos(slot),
                    v.data.profession,
                    v.job_site,
                    v.inventory.clone(),
                )
            })
        })
        .collect();
    panic!(
        "seed 100: buildings={:?} project={:?} residents={residents:?}",
        record.buildings,
        crate::settlements::construction::project_status(&state, record.id)
    );
}

#[test]
#[ignore = "requires the local vanilla 26.1.2 content cache"]
fn seed_100_builder_continues_after_first_storage_through_native_ticks() {
    let mut state = seed_100_village();
    let id = state.settlements.records().next().unwrap().id;
    let mut first_storage = None;
    let mut second_building = None;
    let mut upgrade = None;
    let mut previous: Option<(String, u8, [i32; 3])> = None;
    for _ in 0..60_000 {
        crate::tick::run_tick(&mut state, &[]);
        let record = state.settlements.get(id).unwrap();
        let current = record
            .project
            .as_ref()
            .map(|p| (p.blueprint.as_str(), p.level, p.origin));
        if current
            != previous
                .as_ref()
                .map(|(name, level, origin)| (name.as_str(), *level, *origin))
        {
            eprintln!(
                "PACE_PROJECT tick={} day={} project={current:?} status={:?}",
                state.tick,
                state.day_time,
                crate::settlements::construction::project_status(&state, id)
            );
            previous = current.map(|(name, level, origin)| (name.to_owned(), level, origin));
        } else if current.is_some() && state.tick.is_multiple_of(2000) {
            eprintln!(
                "PACE_PROGRESS tick={} day={} status={:?}",
                state.tick,
                state.day_time,
                crate::settlements::construction::project_status(&state, id)
            );
        }
        let authored = record
            .buildings
            .values()
            .filter(|b| {
                crate::settlements::construction::upgrade_available(&state, id, b.id)
                    || (b.kind == BuildingKind::Storage && !b.natural_zone)
            })
            .count();
        if first_storage.is_none()
            && record
                .buildings
                .values()
                .any(|b| b.kind == BuildingKind::Storage && !b.natural_zone)
        {
            first_storage = Some(state.tick);
            eprintln!("FIRST_STORAGE {first_storage:?}");
        }
        if second_building.is_none() && authored >= 2 {
            second_building = Some(state.tick);
            eprintln!("SECOND_BUILDING {second_building:?}");
        }
        if upgrade.is_none() && record.buildings.values().any(|b| b.level > 1) {
            upgrade = Some(state.tick);
        }
        if second_building.is_some() && upgrade.is_some() {
            break;
        }
    }
    eprintln!(
        "PACE_RESULT first={first_storage:?} second={second_building:?} upgrade={upgrade:?} tick={} status={:?}",
        state.tick,
        crate::settlements::construction::project_status(&state, id)
    );
    let first = first_storage.expect("village must complete its first Storage");
    let second = second_building.expect("village must complete a second authored building");
    let upgraded = upgrade.expect("village must eventually complete an autonomous upgrade");
    assert!(
        first < second && second <= first + 30_000,
        "second building must complete within 30,000 ticks after Storage"
    );
    assert!(upgraded > first, "upgrade must follow the first Storage");
}

#[test]
fn empty_village_gathers_and_starts_storage_through_native_ticks() {
    let mut state = village_with(true);
    let id = state.settlements.records().next().unwrap().id;
    let stock = state
        .settlements
        .get(id)
        .unwrap()
        .bootstrap_storage
        .unwrap();
    assert!(
        state.block_entities[&(stock[0], stock[1], stock[2])]
            .slots
            .iter()
            .all(mc_data::ItemStack::is_empty)
    );
    let log = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:oak_log").unwrap())
        .unwrap();
    let cobble = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:cobblestone").unwrap())
        .unwrap();
    let mut original_logs = std::collections::HashSet::new();
    let mut original_stone = std::collections::HashSet::new();
    for x in -16..32 {
        for z in -16..32 {
            for y in 188..212 {
                let pos = (x, y, z);
                let Some(block) = state
                    .world
                    .block_at(x, y, z)
                    .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
                else {
                    continue;
                };
                match block.block.id.path() {
                    "oak_log" => {
                        original_logs.insert(pos);
                    }
                    "stone" if (y == 200 && x >= 16) || ([x, z] == [10, -1] && y >= 201) => {
                        original_stone.insert(pos);
                    }
                    _ => {}
                }
            }
        }
    }
    let mut removed = [false; 2];
    let mut deposited = [false; 2];
    for _ in 0..30_000 {
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        for change in &output.changes.blocks {
            let p = change.position;
            if change.state == state.registry.states.air {
                removed[0] |= original_logs.contains(&(p.x, p.y, p.z));
                removed[1] |= original_stone.contains(&(p.x, p.y, p.z));
            }
        }
        for stack in &state.block_entities[&(stock[0], stock[1], stock[2])].slots {
            deposited[0] |= stack.item_id == log && stack.count > 0;
            deposited[1] |= stack.item_id == cobble && stack.count > 0;
        }
        if removed == [true; 2]
            && deposited == [true; 2]
            && state
                .settlements
                .get(id)
                .unwrap()
                .project
                .as_ref()
                .is_some_and(|project| project.blueprint == "storage")
        {
            return;
        }
    }
    panic!(
        "native settlement failed: removed={removed:?}, deposited={deposited:?}, project={:?}",
        crate::settlements::construction::project_status(&state, id)
    );
}

#[test]
fn adopted_village_produces_stock_and_builds_without_player_supplies() {
    let mut state = village();
    let id = state
        .settlements
        .records()
        .find(|record| record.anchor == [0, 201, 0])
        .expect("generated bell is adopted")
        .id;
    let record = state.settlements.get(id).unwrap();
    assert_eq!(record.owner, None);
    let stock = record
        .bootstrap_storage
        .expect("empty founding marker chest");
    assert!(
        state.block_entities[&(stock[0], stock[1], stock[2])]
            .slots
            .iter()
            .all(mc_data::ItemStack::is_empty)
    );
    for kind in [BuildingKind::Mine, BuildingKind::LumberCamp] {
        assert!(
            !record
                .buildings
                .values()
                .any(|building| building.kind == kind),
            "adoption queues natural {kind:?} until a later resident route proof"
        );
    }
    let items = ["oak_log", "coal", "wheat", "white_wool"].map(|name| {
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
            .unwrap()
    });
    let mut observed = [false; 4];
    for _ in 0..20_000 {
        state.tick += 20;
        state.day_time = (state.tick as i64 + 3000).rem_euclid(24_000);
        state.tick_budget.reset(state.tick);
        let mut changes = ChangeSet::default();
        crate::settlements::autonomy::tick(&mut state, &mut changes);
        crate::settlements::tick(&mut state, &mut changes);
        super::brain::tick(&mut state, &mut changes);
        let residents: Vec<_> = crate::settlements::construction::residents(&state, id).collect();
        for slot in residents {
            let next = state
                .entities
                .row(slot)
                .villager
                .as_ref()
                .and_then(|v| v.navigation_path.last().copied());
            if let Some(next) = next {
                let body = state.entities.body(slot);
                if let Some(spot) = crate::settlements::allocator::standing(
                    &state.world,
                    &state.registry,
                    next,
                    body.half_width,
                    body.height,
                ) {
                    state.entities.set_pos(slot, spot);
                }
            }
        }
        // Match the real root tick's block commit boundary before POI refresh.
        state.world.fold_overlay();
        for (index, item) in items.iter().enumerate() {
            observed[index] |= state.block_entities.values().any(|container| {
                container
                    .slots
                    .iter()
                    .any(|stack| stack.item_id == *item && stack.count > 0)
            });
        }
        let record = state.settlements.get(id).unwrap();
        if observed.into_iter().all(|seen| seen)
            && [BuildingKind::Storage, BuildingKind::House]
                .into_iter()
                .all(|kind| {
                    record
                        .buildings
                        .values()
                        .any(|building| building.kind == kind)
                })
        {
            for kind in [BuildingKind::Mine, BuildingKind::LumberCamp] {
                assert!(
                    record
                        .buildings
                        .values()
                        .any(|building| building.kind == kind),
                    "planner certifies reachable natural {kind:?}"
                );
            }
            assert!(record.owner.is_none());
            assert!(
                !state.plugin_block_writes.is_empty(),
                "canonical structure executor journals construction"
            );
            assert!(state.entities.slots().any(|slot| {
                state.entities.row(slot).villager.as_ref().is_some_and(|v| {
                    v.settlement == Some(id) && v.data.profession == VillagerProfession::Builder
                })
            }));
            return;
        }
    }
    let mut stock = std::collections::BTreeMap::<String, i32>::new();
    for stack in state.block_entities.values().flat_map(|image| &image.slots) {
        if !stack.is_empty() {
            let name = state
                .registry
                .items
                .name_of(stack.item_id)
                .unwrap()
                .to_string();
            *stock.entry(name).or_default() += stack.count;
        }
    }
    let miners: Vec<_> = crate::settlements::construction::residents(&state, id)
        .filter_map(|slot| {
            let v = state.entities.row(slot).villager.as_ref()?;
            (v.data.profession == VillagerProfession::Miner).then_some((
                state.entities.pos(slot),
                &v.mining_progress,
                &v.inventory,
            ))
        })
        .collect();
    panic!(
        "autonomous closure failed: sources={observed:?}; projects={:?}\nstock={stock:?}\nminers={miners:?}\nroles={:?}",
        crate::settlements::construction::project_status(&state, id),
        crate::settlements::construction::residents(&state, id)
            .map(|slot| {
                let v = state.entities.row(slot).villager.as_ref().unwrap();
                (
                    v.data.profession,
                    v.job_site,
                    state.entities.pos(slot),
                    v.navigation_target,
                )
            })
            .collect::<Vec<_>>()
    );
}
