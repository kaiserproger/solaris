//! Bounded precipitation samples over active resident chunks, never world scans.
use crate::boundary::BlockPos;
use crate::tick::TickState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Precipitation {
    None,
    Rain,
    Snow,
}

// Vanilla biome families; altitude uses the documented temperature lapse rather
// than reproducing Mojang's per-column temperature noise.
pub(super) fn climate(name: &str, y: i32) -> Precipitation {
    let temperature = match name.strip_prefix("minecraft:") {
        Some(
            "desert" | "badlands" | "wooded_badlands" | "eroded_badlands" | "savanna"
            | "savanna_plateau" | "windswept_savanna" | "nether_wastes" | "soul_sand_valley"
            | "crimson_forest" | "warped_forest" | "basalt_deltas" | "the_end" | "end_highlands"
            | "end_midlands" | "small_end_islands" | "end_barrens" | "the_void",
        ) => return Precipitation::None,
        Some(
            "snowy_plains" | "ice_spikes" | "snowy_beach" | "frozen_river" | "frozen_ocean"
            | "deep_frozen_ocean",
        ) => 0.0,
        Some("snowy_taiga") => -0.5,
        Some("grove" | "snowy_slopes" | "jagged_peaks" | "frozen_peaks") => -0.7,
        Some(
            "windswept_hills" | "windswept_forest" | "windswept_gravelly_hills" | "stony_shore",
        ) => 0.2,
        Some("taiga" | "old_growth_spruce_taiga") => 0.25,
        Some("old_growth_pine_taiga") => 0.3,
        Some(_) => 0.8,
        None => return Precipitation::None,
    };
    if temperature - f64::from((y - 80).max(0)) / 600.0 < 0.15 {
        Precipitation::Snow
    } else {
        Precipitation::Rain
    }
}

pub(super) fn at(state: &TickState, pos: BlockPos) -> Precipitation {
    state
        .world
        .biome_at(pos.x, pos.y, pos.z)
        .map_or(Precipitation::None, |biome| climate(biome.as_str(), pos.y))
}

/// Top collision/fluid cell, including this tick's overlay. The scan is bounded
/// by dimension height and runs only for sampled active columns.
pub(super) fn surface(state: &TickState, x: i32, z: i32) -> Option<BlockPos> {
    for y in (state.dimension.geometry().min_y()..state.dimension.geometry().max_y()).rev() {
        let id = state.world.block_at(x, y, z)?;
        if state.registry.materials.classify(id) != mc_physics::BlockMaterial::Air {
            return Some(BlockPos { x, y, z });
        }
    }
    None
}

pub(super) fn precipitate(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    pos: BlockPos,
    roll: u64,
) {
    let Some(current) = state.world.block_at(pos.x, pos.y, pos.z) else {
        return;
    };
    let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(current)) else {
        return;
    };
    let weather = at(state, pos);
    let name = block.block.id.path();
    if state.weather.0 == crate::commands::Weather::Clear && name != "water" {
        return;
    }
    let level = block
        .properties
        .iter()
        .find(|(key, _)| key == "level")
        .and_then(|(_, value)| value.parse::<u8>().ok())
        .unwrap_or(0);
    let next = match (name, weather) {
        ("cauldron" | "water_cauldron", Precipitation::Rain)
            if level < 3 && roll.is_multiple_of(20) =>
        {
            Some(crate::portals::state_id(
                state,
                "minecraft:water_cauldron",
                &[("level", &(level + 1).to_string())],
            ))
        }
        ("cauldron" | "powder_snow_cauldron", Precipitation::Snow)
            if level < 3 && roll.is_multiple_of(10) =>
        {
            Some(crate::portals::state_id(
                state,
                "minecraft:powder_snow_cauldron",
                &[("level", &(level + 1).to_string())],
            ))
        }
        ("water", Precipitation::Snow)
            if level == 0
                && state
                    .world
                    .light_at(pos.x, pos.y, pos.z)
                    .is_some_and(|(_, light)| light < 10)
                && [[1, 0], [-1, 0], [0, 1], [0, -1]]
                    .into_iter()
                    .any(|[dx, dz]| {
                        state
                            .world
                            .block_at(pos.x + dx, pos.y, pos.z + dz)
                            .is_some_and(|id| {
                                state.registry.materials.classify(id)
                                    != mc_physics::BlockMaterial::Water
                            })
                    }) =>
        {
            Some(crate::portals::state_id(state, "minecraft:ice", &[]))
        }
        _ => None,
    };
    if let Some(next) = next {
        write(state, writes, pos, next);
        return;
    }
    if state.weather.0 == crate::commands::Weather::Clear || weather != Precipitation::Snow {
        return;
    }
    let maximum = crate::command_rules::integer(
        state,
        crate::command_rules::IntRule::MAX_SNOW_ACCUMULATION_HEIGHT,
    );
    if maximum == 0 {
        return;
    }
    let block = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(current))
        .unwrap();
    let layers = if block.block.id.path() == "snow" {
        block
            .properties
            .iter()
            .find(|(key, _)| key == "layers")
            .and_then(|(_, value)| value.parse::<i32>().ok())
            .unwrap_or(1)
    } else {
        0
    };
    let target = if layers > 0 {
        pos
    } else {
        BlockPos {
            y: pos.y + 1,
            ..pos
        }
    };
    if layers >= maximum
        || !state
            .world
            .light_at(target.x, target.y, target.z)
            .is_some_and(|(_, light)| light < 10)
    {
        return;
    }
    if layers == 0
        && (!mc_data::block_facts::has_full_sturdy_face(
            current,
            &block.block.id,
            &block.properties,
            mc_data::block_facts::SturdyFace::Up,
        ) || state.world.block_at(target.x, target.y, target.z)
            != Some(state.registry.states.air))
    {
        return;
    }
    let snow = crate::portals::state_id(
        state,
        "minecraft:snow",
        &[("layers", &(layers + 1).to_string())],
    );
    write(state, writes, target, snow);
}

fn write(state: &mut TickState, writes: &mut Vec<(BlockPos, u32)>, pos: BlockPos, next: u32) {
    if !crate::plugin_player::protects_cell(state, pos) {
        crate::portals::write(state, writes, pos, next);
    }
}
