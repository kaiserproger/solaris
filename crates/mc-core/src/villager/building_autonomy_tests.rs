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
