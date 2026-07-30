use mc_data::block_facts::{BlockFactsTable, RandomTickFamily};
use mc_world::plant_rules_26_1_2::{
    PlantBlockEdit, PlantBlockRead, bamboo_sapling_growth_edits, next_crop_growth_state,
    sapling_tree_edits, stem_fruit_edits, vertical_plant_growth_edits,
};
use mc_world::{
    BlockPos, BlockRegistry, BlockStateId, ChunkSection, MAX_Y, MIN_Y, SECTION_COUNT, SECTION_DIM,
};

use super::{
    BlockEdit, BlockPlanningRead, Identifier, ItemRegistry, ItemStack, RandomTickPolicy,
    RandomTickSample, air_state_id, block_state_property, fluid_neighbour_positions,
    sibling_state_with_property, splitmix64,
};

struct PlantReadAdapter<'a, T: ?Sized>(&'a T);

impl<T: BlockPlanningRead + ?Sized> PlantBlockRead for PlantReadAdapter<'_, T> {
    fn get_cached_block(&self, pos: BlockPos) -> Option<BlockStateId> {
        self.0.get_cached_block(pos)
    }
}

fn into_block_edits(edits: Vec<PlantBlockEdit>) -> Vec<BlockEdit> {
    edits.into_iter().map(BlockEdit::from).collect()
}

pub(super) fn section_may_random_tick(section: &ChunkSection, facts: &BlockFactsTable) -> bool {
    if let Some(palette) = section.palette() {
        return palette
            .iter()
            .any(|state| facts.random_tick_family(state.0).is_some());
    }
    facts.random_tick_family(section.get(0, 0, 0).0).is_some()
}

#[cfg(test)]
pub(super) fn random_tick_edit(
    blocks: &BlockRegistry,
    facts: &BlockFactsTable,
    world: &impl BlockPlanningRead,
    pos: BlockPos,
    state: BlockStateId,
    family: RandomTickFamily,
) -> Option<Vec<BlockEdit>> {
    random_tick_edit_seeded(blocks, facts, world, pos, state, family, 0)
}

pub(super) fn random_tick_edit_seeded(
    blocks: &BlockRegistry,
    facts: &BlockFactsTable,
    world: &impl BlockPlanningRead,
    pos: BlockPos,
    state: BlockStateId,
    family: RandomTickFamily,
    random_seed: u64,
) -> Option<Vec<BlockEdit>> {
    let plant_world = PlantReadAdapter(world);
    match family {
        RandomTickFamily::Crop => next_crop_growth_state(blocks, state)
            .map(|new_state| vec![BlockEdit { pos, new_state }])
            .or_else(|| stem_fruit_edits(blocks, &plant_world, pos, state).map(into_block_edits))
            .or_else(|| {
                vertical_plant_growth_edits(blocks, &plant_world, pos, state, random_seed)
                    .map(into_block_edits)
            }),
        RandomTickFamily::Farmland => next_farmland_state(blocks, facts, world, pos, state)
            .map(|new_state| vec![BlockEdit { pos, new_state }]),
        RandomTickFamily::Fire => fire_tick_edits(blocks, world, pos, state, random_seed),
        RandomTickFamily::Grass => {
            next_grass_edit(blocks, world, pos, state).map(|edit| vec![edit])
        }
        RandomTickFamily::Leaves => {
            next_leaf_decay_state(blocks, state).map(|new_state| vec![BlockEdit { pos, new_state }])
        }
        RandomTickFamily::Sapling => {
            if blocks
                .by_id(state)
                .is_some_and(|state| state.block.id.path() == "bamboo_sapling")
            {
                return bamboo_sapling_growth_edits(blocks, &plant_world, pos, random_seed)
                    .map(into_block_edits);
            }
            if !random_seed.is_multiple_of(7) {
                return None;
            }
            sapling_tree_edits(
                blocks,
                &plant_world,
                pos,
                state,
                splitmix64(random_seed ^ 0x5452_4545_4752_4f57),
            )
            .map(into_block_edits)
        }
    }
}

pub(super) fn random_tick_candidate_seed(
    seed: u64,
    world_tick: u64,
    pos: BlockPos,
    candidate_index: usize,
) -> u64 {
    let seed = splitmix64(seed ^ world_tick);
    let seed = splitmix64(seed ^ pos.x as i64 as u64);
    let seed = splitmix64(seed ^ pos.y as i64 as u64);
    let seed = splitmix64(seed ^ pos.z as i64 as u64);
    splitmix64(seed ^ candidate_index as u64)
}

pub(super) fn next_leaf_decay_state(
    blocks: &BlockRegistry,
    state: BlockStateId,
) -> Option<BlockStateId> {
    let current = blocks.by_id(state)?;
    if !current.block.id.path().ends_with("_leaves") {
        return None;
    }
    if block_state_property(current, "persistent") == Some("true") {
        return None;
    }
    let distance = block_state_property(current, "distance")?
        .parse::<u8>()
        .ok()?;
    (distance >= 7).then(|| air_state_id(blocks))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LeafDecayDropRolls {
    pub(super) sapling: u16,
    pub(super) stick: u16,
    pub(super) apple: u16,
    pub(super) stick_count: i32,
}

pub(super) fn leaf_decay_drop_rolls(
    seed: u64,
    world_tick: u64,
    pos: BlockPos,
) -> LeafDecayDropRolls {
    let seed = splitmix64(seed ^ world_tick);
    let seed = splitmix64(seed ^ pos.x as i64 as u64);
    let seed = splitmix64(seed ^ pos.y as i64 as u64);
    let seed = splitmix64(seed ^ pos.z as i64 as u64);
    LeafDecayDropRolls {
        sapling: (splitmix64(seed ^ 1) % 1_000) as u16,
        stick: (splitmix64(seed ^ 2) % 1_000) as u16,
        apple: (splitmix64(seed ^ 3) % 1_000) as u16,
        stick_count: 1 + (splitmix64(seed ^ 4) & 1) as i32,
    }
}

pub(super) fn natural_leaf_decay_drops(
    blocks: &BlockRegistry,
    items: &ItemRegistry,
    state: BlockStateId,
    rolls: LeafDecayDropRolls,
) -> Vec<ItemStack> {
    let Some(state) = blocks.by_id(state) else {
        return Vec::new();
    };
    let path = state.block.id.path();
    let sapling = match path {
        "oak_leaves" => Some(("minecraft:oak_sapling", 50)),
        "spruce_leaves" => Some(("minecraft:spruce_sapling", 50)),
        "birch_leaves" => Some(("minecraft:birch_sapling", 50)),
        "jungle_leaves" => Some(("minecraft:jungle_sapling", 25)),
        "acacia_leaves" => Some(("minecraft:acacia_sapling", 50)),
        "dark_oak_leaves" => Some(("minecraft:dark_oak_sapling", 50)),
        "pale_oak_leaves" => Some(("minecraft:pale_oak_sapling", 50)),
        "cherry_leaves" => Some(("minecraft:cherry_sapling", 50)),
        "azalea_leaves" => Some(("minecraft:azalea", 50)),
        "flowering_azalea_leaves" => Some(("minecraft:flowering_azalea", 50)),
        _ => None,
    };

    let mut drops = Vec::new();
    if let Some((item, chance)) = sapling
        && rolls.sapling < chance
        && let Some(stack) = named_item_stack(items, item, 1)
    {
        drops.push(stack);
    }
    if path.ends_with("_leaves")
        && rolls.stick < 20
        && let Some(stack) =
            named_item_stack(items, "minecraft:stick", rolls.stick_count.clamp(1, 2))
    {
        drops.push(stack);
    }
    if matches!(path, "oak_leaves" | "dark_oak_leaves")
        && rolls.apple < 5
        && let Some(stack) = named_item_stack(items, "minecraft:apple", 1)
    {
        drops.push(stack);
    }
    drops
}

pub(super) fn named_item_stack(items: &ItemRegistry, name: &str, count: i32) -> Option<ItemStack> {
    let id = Identifier::parse(name).expect("static item identifier");
    items
        .id_of(&id)
        .map(|item_id| ItemStack::new(item_id, count))
}

pub(super) fn next_leaf_distance_state(
    blocks: &BlockRegistry,
    storage: &impl BlockPlanningRead,
    pos: BlockPos,
    state: BlockStateId,
) -> Option<BlockStateId> {
    let current = blocks.by_id(state)?;
    if !current.block.id.path().ends_with("_leaves") {
        return None;
    }
    let current_distance = block_state_property(current, "distance")?
        .parse::<u8>()
        .ok()?;
    let mut distance = 7;
    for neighbour in fluid_neighbour_positions(pos) {
        if !(MIN_Y..MAX_Y).contains(&neighbour.y) {
            continue;
        }
        let neighbour_state = storage.get_cached_block(neighbour)?;
        distance =
            distance.min(leaf_distance_from_state(blocks, neighbour_state).saturating_add(1));
        if distance == 1 {
            break;
        }
    }
    let distance = distance.min(7);
    if distance == current_distance {
        return None;
    }
    sibling_state_with_property(blocks, current, "distance", &distance.to_string())
}

pub(super) fn leaf_distance_from_state(blocks: &BlockRegistry, state: BlockStateId) -> u8 {
    let Some(state) = blocks.by_id(state) else {
        return 7;
    };
    let path = state.block.id.path();
    if path.ends_with("_log")
        || path.ends_with("_wood")
        || matches!(
            path,
            "crimson_stem"
                | "stripped_crimson_stem"
                | "warped_stem"
                | "stripped_warped_stem"
                | "crimson_hyphae"
                | "stripped_crimson_hyphae"
                | "warped_hyphae"
                | "stripped_warped_hyphae"
        )
    {
        return 0;
    }
    if !path.ends_with("_leaves") {
        return 7;
    }
    block_state_property(state, "distance")
        .and_then(|distance| distance.parse::<u8>().ok())
        .unwrap_or(7)
        .min(7)
}

pub(super) fn next_fire_state(blocks: &BlockRegistry, state: BlockStateId) -> Option<BlockStateId> {
    let current = blocks.by_id(state)?;
    if current.block.id.as_str() == "minecraft:soul_fire" {
        return Some(air_state_id(blocks));
    }
    if current.block.id.as_str() != "minecraft:fire" {
        return None;
    }
    let age = block_state_property(current, "age")?.parse::<u8>().ok()?;
    if age >= 15 {
        return Some(air_state_id(blocks));
    }
    sibling_state_with_property(blocks, current, "age", &(age + 1).to_string())
}

fn fire_tick_edits(
    blocks: &BlockRegistry,
    world: &impl BlockPlanningRead,
    pos: BlockPos,
    state: BlockStateId,
    random_seed: u64,
) -> Option<Vec<BlockEdit>> {
    let next_state = next_fire_state(blocks, state)?;
    let mut edits = vec![BlockEdit {
        pos,
        new_state: next_state,
    }];
    let current = blocks.by_id(state)?;
    if current.block.id.as_str() != "minecraft:fire"
        || next_state == air_state_id(blocks)
        || !random_seed.is_multiple_of(3)
    {
        return Some(edits);
    }

    let neighbours = fluid_neighbour_positions(pos);
    let start = random_seed as usize % neighbours.len();
    for offset in 0..neighbours.len() {
        let target = neighbours[(start + offset) % neighbours.len()];
        let Some(target_state) = world.get_cached_block(target) else {
            continue;
        };
        let Some(target_block) = blocks.by_id(target_state) else {
            continue;
        };
        if is_common_flammable_block(target_block.block.id.path()) {
            edits.push(BlockEdit {
                pos: target,
                new_state: current.block.default,
            });
            break;
        }
    }
    Some(edits)
}

fn is_common_flammable_block(path: &str) -> bool {
    path.ends_with("_log")
        || path.ends_with("_wood")
        || path.ends_with("_planks")
        || path.ends_with("_leaves")
        || path.ends_with("_wool")
        || matches!(path, "bookshelf" | "hay_block" | "tnt")
}

pub(super) fn next_grass_edit(
    blocks: &BlockRegistry,
    world: &impl BlockPlanningRead,
    pos: BlockPos,
    state: BlockStateId,
) -> Option<BlockEdit> {
    let current = blocks.by_id(state)?;
    if current.block.id.as_str() != "minecraft:grass_block" {
        return None;
    }
    if !block_above_allows_grass(blocks, world, pos) {
        let dirt = Identifier::parse("minecraft:dirt").expect("static identifier");
        return blocks.block(&dirt).map(|block| BlockEdit {
            pos,
            new_state: block.default,
        });
    }
    let grass_state = blocks
        .block(&Identifier::parse("minecraft:grass_block").expect("static identifier"))
        .map(|block| block.default)?;
    for dy in -1..=1 {
        for dz in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 && dz == 0 {
                    continue;
                }
                let target = BlockPos {
                    x: pos.x + dx,
                    y: pos.y + dy,
                    z: pos.z + dz,
                };
                if block_is(blocks, world, target, "minecraft:dirt")
                    && block_above_allows_grass(blocks, world, target)
                {
                    return Some(BlockEdit {
                        pos: target,
                        new_state: grass_state,
                    });
                }
            }
        }
    }
    None
}

pub(super) fn block_above_allows_grass(
    blocks: &BlockRegistry,
    world: &impl BlockPlanningRead,
    pos: BlockPos,
) -> bool {
    world
        .get_cached_block(BlockPos {
            x: pos.x,
            y: pos.y + 1,
            z: pos.z,
        })
        .and_then(|state| blocks.by_id(state))
        .is_none_or(|state| {
            matches!(
                state.block.id.as_str(),
                "minecraft:air"
                    | "minecraft:cave_air"
                    | "minecraft:short_grass"
                    | "minecraft:tall_grass"
            )
        })
}

pub(super) fn block_is(
    blocks: &BlockRegistry,
    world: &impl BlockPlanningRead,
    pos: BlockPos,
    name: &str,
) -> bool {
    world
        .get_cached_block(pos)
        .and_then(|state| blocks.by_id(state))
        .is_some_and(|state| state.block.id.as_str() == name)
}

pub(super) fn next_farmland_state(
    blocks: &BlockRegistry,
    facts: &BlockFactsTable,
    world: &impl BlockPlanningRead,
    pos: BlockPos,
    state: BlockStateId,
) -> Option<BlockStateId> {
    let current = blocks.by_id(state)?;
    if current.block.id.as_str() != "minecraft:farmland" {
        return None;
    }
    let moisture = block_state_property(current, "moisture")?
        .parse::<u8>()
        .ok()?;
    if farmland_has_nearby_water(blocks, world, pos) {
        return (moisture < 7)
            .then(|| farmland_state_with_moisture(blocks, 7))
            .flatten();
    }
    if moisture > 0 {
        return farmland_state_with_moisture(blocks, moisture - 1);
    }
    if farmland_has_crop_above(facts, world, pos) {
        return None;
    }
    let dirt = Identifier::parse("minecraft:dirt").expect("static identifier");
    blocks.block(&dirt).map(|block| block.default)
}

pub(super) fn farmland_state_with_moisture(
    blocks: &BlockRegistry,
    moisture: u8,
) -> Option<BlockStateId> {
    let farmland = Identifier::parse("minecraft:farmland").expect("static identifier");
    blocks.by_name_and_props(&farmland, &[("moisture".to_string(), moisture.to_string())])
}

pub(super) fn farmland_has_crop_above(
    facts: &BlockFactsTable,
    world: &impl BlockPlanningRead,
    pos: BlockPos,
) -> bool {
    world
        .get_cached_block(BlockPos {
            x: pos.x,
            y: pos.y + 1,
            z: pos.z,
        })
        .and_then(|state| facts.random_tick_family(state.0))
        == Some(RandomTickFamily::Crop)
}

pub(super) fn farmland_has_nearby_water(
    blocks: &BlockRegistry,
    world: &impl BlockPlanningRead,
    pos: BlockPos,
) -> bool {
    for x in (pos.x - 4)..=(pos.x + 4) {
        for z in (pos.z - 4)..=(pos.z + 4) {
            for y in pos.y..=(pos.y + 1) {
                let Some(state) = world.get_cached_block(BlockPos { x, y, z }) else {
                    continue;
                };
                if blocks
                    .by_id(state)
                    .is_some_and(|state| state.block.id.as_str() == "minecraft:water")
                {
                    return true;
                }
            }
        }
    }
    false
}

pub(super) fn sample_random_tick_positions(
    policy: RandomTickPolicy,
    world_tick: u64,
    chunks: &[(i32, i32)],
) -> Vec<RandomTickSample> {
    let policy = policy.normalized();
    if !policy.is_enabled() || chunks.is_empty() {
        return Vec::new();
    }
    let chunk_count = chunks.len();
    let budget = policy.chunk_budget.min(chunk_count);
    let start = (world_tick as usize) % chunk_count;
    let mut samples =
        Vec::with_capacity(budget * policy.random_tick_speed as usize * SECTION_COUNT);
    for offset in 0..budget {
        let chunk = chunks[(start + offset) % chunk_count];
        let chunk_seed = policy.seed
            ^ world_tick
            ^ ((chunk.0 as i64 as u64) << 32)
            ^ (chunk.1 as i64 as u64)
            ^ ((offset as u64) << 48);
        for section_idx in 0..SECTION_COUNT {
            for sample_idx in 0..policy.random_tick_speed {
                let section_sample = ((section_idx as u64) << 32) | u64::from(sample_idx);
                let hash = splitmix64(chunk_seed ^ splitmix64(section_sample));
                let local_x = (hash & 0xF) as i32;
                let local_z = ((hash >> 4) & 0xF) as i32;
                let local_y = ((hash >> 8) & 0xF) as i32;
                samples.push(RandomTickSample {
                    chunk,
                    pos: BlockPos {
                        x: chunk.0 * SECTION_DIM as i32 + local_x,
                        y: MIN_Y + section_idx as i32 * SECTION_DIM as i32 + local_y,
                        z: chunk.1 * SECTION_DIM as i32 + local_z,
                    },
                });
            }
        }
    }
    samples
}
