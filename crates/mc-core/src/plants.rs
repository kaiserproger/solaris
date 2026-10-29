//! Plant rules shared with the old core, evaluated against native tick state.
//! Plans record every read so growth and inventory payment merge atomically.

use std::cell::RefCell;

use mc_world::BlockStateId;
use mc_world::plant_rules_26_1_2::{self as rules, PlantBlockEdit, PlantBlockRead};
use rustc_hash::FxHashMap;

use crate::boundary::{BlockPos, EntityId};
use crate::tick::TickState;

#[path = "plants_growth.rs"]
mod growth;

pub(crate) struct Plan {
    pub(crate) writes: Vec<(BlockPos, u32)>,
    pub(crate) expects: Vec<(BlockPos, u32)>,
}

/// One live birth staged by an egg hatch: mob type, position, health.
pub struct EggSpawn {
    pub type_id: u32,
    pub pos: [f64; 3],
    pub health: f32,
}

/// A hatched egg: block rewrites plus live births.
pub(crate) struct Hatch {
    pub(crate) cells: Vec<(BlockPos, u32)>,
    pub(crate) spawn: Vec<EggSpawn>,
}

struct Read<'a> {
    state: &'a TickState,
    reads: RefCell<FxHashMap<BlockPos, u32>>,
}

impl PlantBlockRead for Read<'_> {
    fn get_cached_block(&self, pos: mc_world::BlockPos) -> Option<BlockStateId> {
        let at = native(pos);
        let id = self.state.world.block_at(at.x, at.y, at.z)?;
        self.reads.borrow_mut().entry(at).or_insert(id);
        Some(BlockStateId(id))
    }

    fn combined_light_at(&self, pos: mc_world::BlockPos) -> Option<u8> {
        Some(self.state.raw_brightness(pos.x, pos.y, pos.z))
    }
}

pub(crate) fn native(pos: mc_world::BlockPos) -> BlockPos {
    BlockPos {
        x: pos.x,
        y: pos.y,
        z: pos.z,
    }
}

pub(crate) fn old(pos: BlockPos) -> mc_world::BlockPos {
    mc_world::BlockPos {
        x: pos.x,
        y: pos.y,
        z: pos.z,
    }
}

fn plan(
    state: &TickState,
    at: BlockPos,
    f: impl FnOnce(&Read<'_>) -> Option<Vec<PlantBlockEdit>>,
) -> Option<Plan> {
    let read = Read {
        state,
        reads: RefCell::new(FxHashMap::default()),
    };
    let id = read.get_cached_block(old(at))?.0;
    read.get_cached_block(old(BlockPos { y: at.y - 1, ..at }))?;
    if state
        .registry
        .blocks
        .by_id(BlockStateId(id))
        .is_some_and(|block| matches!(block.block.id.path(), "cocoa" | "cactus"))
    {
        for side in horizontal(at) {
            read.get_cached_block(old(side))?;
        }
    }
    let edits = f(&read)?;
    // Every output must be resident, even when the shared rule did not read it.
    for edit in &edits {
        read.get_cached_block(edit.pos)?;
    }
    Some(Plan {
        writes: edits
            .into_iter()
            .map(|edit| (native(edit.pos), edit.new_state.0))
            .collect(),
        expects: read.reads.into_inner().into_iter().collect(),
    })
}

pub(crate) fn random_growth(state: &TickState, at: BlockPos, id: u32, seed: u64) -> Option<Plan> {
    let blocks = &state.registry.blocks;
    let current = blocks.by_id(BlockStateId(id))?;
    let path = current.block.id.path();
    plan(state, at, |read| {
        let pos = old(at);
        match path {
            "kelp" | "cave_vines" | "weeping_vines" | "twisting_vines" | "vine"
            | "brown_mushroom" | "red_mushroom" => growth::random(read, at, id, seed),
            "pointed_dripstone" => growth::dripstone(read, at, id, seed),
            "budding_amethyst" => growth::amethyst(read, at, seed),
            "chorus_flower" => crate::chorus::growth(blocks, read, pos, BlockStateId(id), seed),
            "bamboo_sapling" => rules::bamboo_sapling_growth_edits(blocks, read, pos, seed),
            "cactus" | "sugar_cane" | "bamboo" => {
                rules::vertical_plant_growth_edits(blocks, read, pos, BlockStateId(id), seed)
            }
            _ if path.ends_with("_sapling") => {
                rules::sapling_tree_edits(blocks, read, pos, BlockStateId(id), seed)
            }
            _ => {
                if let Some(new_state) = rules::next_crop_growth_state(blocks, BlockStateId(id)) {
                    if !rules::crop_growth_gate_passes(blocks, read, pos, BlockStateId(id), seed) {
                        return None;
                    }
                    Some(vec![PlantBlockEdit { pos, new_state }])
                } else {
                    if !rules::crop_growth_gate_passes(blocks, read, pos, BlockStateId(id), seed) {
                        return None;
                    }
                    rules::stem_fruit_edits(blocks, read, pos, BlockStateId(id))
                }
            }
        }
    })
}

/// Vanilla turtle-egg random tick: crack staging at night on sand
/// (hatch 0→1→2, then break into babies). Sniffer eggs and frogspawn use
/// the scheduled clock (crate::eggs).
pub(crate) fn egg_hatch(state: &TickState, at: BlockPos, id: u32, seed: u64) -> Option<Hatch> {
    let read = Read {
        state,
        reads: RefCell::new(FxHashMap::default()),
    };
    read.get_cached_block(old(at))?;
    let (cells, spawn) = growth::hatch(&read, at, id, seed)?;
    let mut births = Vec::new();
    for (name, pos) in spawn {
        let type_id = state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).ok()?)?;
        let health = state
            .registry
            .blueprint(type_id)
            .map_or(10.0, |blueprint| blueprint.params.max_health);
        births.push(EggSpawn {
            type_id,
            pos,
            health,
        });
    }
    Some(Hatch {
        cells: cells
            .into_iter()
            .map(|edit| (native(edit.pos), edit.new_state.0))
            .collect(),
        spawn: births,
    })
}

pub(crate) fn bonemeal(state: &TickState, at: BlockPos, seed: u64) -> Option<Plan> {
    let id = state.world.block_at(at.x, at.y, at.z)?;
    // Vanilla's hanging propagule bone-meal target checks age, not survival.
    // An unsupported command-created propagule remains usable until a
    // neighbor update removes it.
    let hanging_propagule = state
        .registry
        .blocks
        .by_id(BlockStateId(id))
        .is_some_and(|block| block.block.id.path() == "mangrove_propagule")
        && crate::farming::prop(&state.registry.blocks, id, "hanging") == Some("true");
    if !hanging_propagule && !can_stand(state, id, at) {
        return None;
    }
    plan(state, at, |read| {
        if let Some(edits) = growth::bonemeal(read, at, id, seed) {
            return Some(edits);
        }
        rules::bonemeal_growth_edits(
            &state.registry.blocks,
            read,
            old(at),
            BlockStateId(id),
            seed,
        )
    })
}

fn soil(path: &str) -> bool {
    matches!(
        path,
        "dirt"
            | "coarse_dirt"
            | "rooted_dirt"
            | "mud"
            | "muddy_mangrove_roots"
            | "moss_block"
            | "pale_moss_block"
            | "grass_block"
            | "podzol"
            | "mycelium"
    )
}

fn path_at(state: &TickState, at: BlockPos) -> Option<&str> {
    let id = state.world.block_at(at.x, at.y, at.z)?;
    Some(
        state
            .registry
            .blocks
            .by_id(BlockStateId(id))?
            .block
            .id
            .path(),
    )
}

fn sturdy(state: &TickState, at: BlockPos, support: BlockPos) -> bool {
    use mc_data::block_facts::{SturdyFace, has_center_sturdy_face, has_full_sturdy_face};
    let Some(id) = state.world.block_at(support.x, support.y, support.z) else {
        return true;
    };
    let Some(block) = state.registry.blocks.by_id(BlockStateId(id)) else {
        return true;
    };
    let face = if support.y < at.y {
        SturdyFace::Up
    } else if support.y > at.y {
        SturdyFace::Down
    } else if support.x < at.x {
        SturdyFace::East
    } else if support.x > at.x {
        SturdyFace::West
    } else if support.z < at.z {
        SturdyFace::South
    } else {
        SturdyFace::North
    };
    if support.y > at.y {
        has_center_sturdy_face(id, &block.block.id, &block.properties, face)
    } else {
        has_full_sturdy_face(id, &block.block.id, &block.properties, face)
    }
}

pub(crate) fn can_stand(state: &TickState, id: u32, at: BlockPos) -> bool {
    let Some(block) = state.registry.blocks.by_id(BlockStateId(id)) else {
        return false;
    };
    let path = block.block.id.path();
    let below = BlockPos { y: at.y - 1, ..at };
    let Some(support) = path_at(state, below) else {
        // Missing boundary data must not destroy a resident plant.
        return true;
    };
    let sand = matches!(support, "sand" | "red_sand" | "suspicious_sand");
    match path {
        "chorus_flower" | "chorus_plant" => {
            crate::chorus::supported(state, at, path == "chorus_flower")
        }
        "wheat"
        | "carrots"
        | "potatoes"
        | "beetroots"
        | "melon_stem"
        | "pumpkin_stem"
        | "attached_melon_stem"
        | "attached_pumpkin_stem"
        | "torchflower_crop" => support == "farmland",
        "pitcher_crop" => {
            let half = crate::farming::prop(&state.registry.blocks, id, "half");
            if half == Some("upper") {
                support == "pitcher_crop"
                    && state
                        .world
                        .block_at(below.x, below.y, below.z)
                        .is_none_or(|other| {
                            crate::farming::prop(&state.registry.blocks, other, "half")
                                == Some("lower")
                        })
            } else {
                support == "farmland"
                    && (crate::farming::prop(&state.registry.blocks, id, "age")
                        .and_then(|age| age.parse::<u32>().ok())
                        .is_none_or(|age| age < 3)
                        || path_at(state, BlockPos { y: at.y + 1, ..at })
                            .is_none_or(|path| path == "pitcher_crop"))
            }
        }
        "mangrove_propagule" => {
            if crate::farming::prop(&state.registry.blocks, id, "hanging") == Some("true") {
                path_at(state, BlockPos { y: at.y + 1, ..at })
                    .is_none_or(|path| path == "mangrove_leaves")
            } else {
                soil(support) || support == "farmland"
            }
        }
        "bell" => {
            let support = match crate::farming::prop(&state.registry.blocks, id, "attachment") {
                Some("floor") => below,
                Some("ceiling") => BlockPos { y: at.y + 1, ..at },
                Some("single_wall" | "double_wall") => {
                    match crate::farming::prop(&state.registry.blocks, id, "facing") {
                        Some("north") => BlockPos { z: at.z - 1, ..at },
                        Some("south") => BlockPos { z: at.z + 1, ..at },
                        Some("west") => BlockPos { x: at.x - 1, ..at },
                        Some("east") => BlockPos { x: at.x + 1, ..at },
                        _ => return true,
                    }
                }
                _ => return true,
            };
            let other = BlockPos {
                x: at.x * 2 - support.x,
                y: at.y * 2 - support.y,
                z: at.z * 2 - support.z,
            };
            sturdy(state, at, support)
                || (crate::farming::prop(&state.registry.blocks, id, "attachment")
                    == Some("double_wall")
                    && sturdy(state, at, other))
        }
        "nether_wart" => support == "soul_sand",
        "cactus" => {
            (support == "cactus" || sand)
                && horizontal(at).iter().all(|pos| {
                    state.world.block_at(pos.x, pos.y, pos.z).is_none_or(|id| {
                        state
                            .registry
                            .blocks
                            .by_id(BlockStateId(id))
                            .is_none_or(|block| block.block.id.path() != "lava")
                            && !state.registry.materials.classify(id).is_solid()
                    })
                })
        }
        "sugar_cane" => {
            support == "sugar_cane"
                || ((soil(support) || sand)
                    && horizontal(below).iter().any(|pos| {
                        path_at(state, *pos).is_none_or(|path| {
                            matches!(path, "water" | "flowing_water" | "frosted_ice")
                        }) || state.world.block_at(pos.x, pos.y, pos.z).is_some_and(|id| {
                            crate::farming::prop(&state.registry.blocks, id, "waterlogged")
                                == Some("true")
                        })
                    }))
        }
        "bamboo" | "bamboo_sapling" => {
            soil(support)
                || sand
                || matches!(
                    support,
                    "bamboo" | "bamboo_sapling" | "gravel" | "suspicious_gravel"
                )
        }
        "cocoa" => {
            let (dx, dz) = match crate::farming::prop(&state.registry.blocks, id, "facing") {
                Some("north") => (0, -1),
                Some("south") => (0, 1),
                Some("west") => (-1, 0),
                Some("east") => (1, 0),
                _ => return false,
            };
            path_at(
                state,
                BlockPos {
                    x: at.x + dx,
                    z: at.z + dz,
                    ..at
                },
            )
            .is_none_or(|path| {
                matches!(
                    path,
                    "jungle_log" | "jungle_wood" | "stripped_jungle_log" | "stripped_jungle_wood"
                )
            })
        }
        _ if path.ends_with("_sapling") || path == "sweet_berry_bush" => {
            soil(support) || support == "farmland"
        }
        _ => true,
    }
}

pub(crate) fn horizontal(at: BlockPos) -> [BlockPos; 4] {
    [
        BlockPos { x: at.x - 1, ..at },
        BlockPos { x: at.x + 1, ..at },
        BlockPos { z: at.z - 1, ..at },
        BlockPos { z: at.z + 1, ..at },
    ]
}

pub(crate) fn neighbours(at: BlockPos) -> [BlockPos; 6] {
    let [a, b, c, d] = horizontal(at);
    [
        a,
        b,
        c,
        d,
        BlockPos { y: at.y - 1, ..at },
        BlockPos { y: at.y + 1, ..at },
    ]
}

/// Called for each settled block write; cascades through unsupported columns.
pub(crate) fn neighbor_updates(
    state: &mut TickState,
    changed: BlockPos,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &mut Vec<EntityId>,
) {
    let mut pending = vec![changed];
    while let Some(changed) = pending.pop() {
        for at in neighbours(changed).into_iter().chain(horizontal(BlockPos {
            y: changed.y + 1,
            ..changed
        })) {
            let Some(id) = state.world.block_at(at.x, at.y, at.z) else {
                continue;
            };
            crate::leaves::schedule(state, at, id);
            if crate::farming::covered_farmland(state, at, id) {
                let dirt = state.registry.states.dirt;
                state.world.set_block(at.x, at.y, at.z, dirt);
                writes.retain(|(pos, _)| *pos != at);
                writes.push((at, dirt));
                pending.push(at);
            } else if !can_stand(state, id, at) {
                if !state.pop_block_loot(at, id, spawned) {
                    continue;
                }
                let replacement = crate::placement::break_state(
                    &state.registry.blocks,
                    id,
                    state.registry.states.air,
                    state.registry.states.water_levels[0],
                );
                state.world.set_block(at.x, at.y, at.z, replacement);
                writes.retain(|(pos, _)| *pos != at);
                writes.push((at, replacement));
                pending.push(at);
            } else if let Some(new) = bell_wall_update(state, at, id) {
                state.world.set_block(at.x, at.y, at.z, new);
                writes.retain(|(pos, _)| *pos != at);
                writes.push((at, new));
            } else if let Some(new) = detached_stem(state, at, id) {
                state.world.set_block(at.x, at.y, at.z, new);
                writes.retain(|(pos, _)| *pos != at);
                writes.push((at, new));
            } else if let Some(new) = crate::chorus::connections(state, at, id) {
                state.world.set_block(at.x, at.y, at.z, new);
                writes.retain(|(pos, _)| *pos != at);
                writes.push((at, new));
            }
        }
        if let Some(id) = state.world.block_at(changed.x, changed.y, changed.z) {
            crate::leaves::schedule(state, changed, id);
        }
    }
}

fn bell_wall_update(state: &TickState, at: BlockPos, id: u32) -> Option<u32> {
    let blocks = &state.registry.blocks;
    if blocks.by_id(BlockStateId(id))?.block.id.path() != "bell"
        || crate::farming::prop(blocks, id, "attachment") != Some("double_wall")
    {
        return None;
    }
    let facing = crate::farming::prop(blocks, id, "facing")?;
    let (dx, dz, opposite) = match facing {
        "north" => (0, -1, "south"),
        "south" => (0, 1, "north"),
        "west" => (-1, 0, "east"),
        "east" => (1, 0, "west"),
        _ => return None,
    };
    let front = sturdy(
        state,
        at,
        BlockPos {
            x: at.x + dx,
            z: at.z + dz,
            ..at
        },
    );
    let back = sturdy(
        state,
        at,
        BlockPos {
            x: at.x - dx,
            z: at.z - dz,
            ..at
        },
    );
    if front == back {
        return None;
    }
    let single = crate::farming::with_prop(blocks, id, "attachment", "single_wall")?;
    crate::farming::with_prop(
        blocks,
        single,
        "facing",
        if front { facing } else { opposite },
    )
}

fn detached_stem(state: &TickState, at: BlockPos, id: u32) -> Option<u32> {
    let block = state.registry.blocks.by_id(BlockStateId(id))?;
    let fruit = match block.block.id.path() {
        "attached_melon_stem" => "melon",
        "attached_pumpkin_stem" => "pumpkin",
        _ => return None,
    };
    let (dx, dz) = match crate::farming::prop(&state.registry.blocks, id, "facing")? {
        "north" => (0, -1),
        "south" => (0, 1),
        "west" => (-1, 0),
        "east" => (1, 0),
        _ => return None,
    };
    let path = path_at(
        state,
        BlockPos {
            x: at.x + dx,
            z: at.z + dz,
            ..at
        },
    )?;
    if path == fruit {
        return None;
    }
    let name = mc_data::Identifier::parse(format!("minecraft:{fruit}_stem")).ok()?;
    state
        .registry
        .blocks
        .by_name_and_props(&name, &[("age".to_owned(), "7".to_owned())])
        .map(|id| id.0)
}
