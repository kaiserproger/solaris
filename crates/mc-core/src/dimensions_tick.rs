//! A single owner advances all worlds and publishes one immutable tick image.
use super::Dimension;
use crate::boundary::{ConnectionId, PlayerChange, PlayerInput, TickOutput, TickTimes};
use crate::tick::TickState;
use std::sync::Arc;

fn input_dimension(state: &TickState, input: &PlayerInput) -> Dimension {
    let connection = input.connection();
    if state.connections.entity_of(connection).is_some() {
        return state.dimension;
    }
    for (&dimension, world) in &state.other_dimensions {
        if world.connections.entity_of(connection).is_some() {
            return dimension;
        }
    }
    if let PlayerInput::Join { uuid, .. } = input {
        for world in
            std::iter::once(state).chain(state.other_dimensions.values().map(|world| &**world))
        {
            if world.entities.slot_of_uuid(*uuid).is_some()
                || world
                    .connections
                    .all_connections()
                    .any(|connection| world.connections.uuid_of(connection) == Some(*uuid))
                || world.offline_players.contains_key(uuid)
            {
                return world.dimension;
            }
        }
    }
    state.dimension
}

/// Move global authorities, never clone them or let a second owner mutate them.
pub(crate) fn swap_globals(root: &mut TickState, world: &mut TickState) {
    std::mem::swap(&mut root.plugins, &mut world.plugins);
    std::mem::swap(&mut root.plugin_runtime, &mut world.plugin_runtime);
    std::mem::swap(&mut root.plugin_players, &mut world.plugin_players);
    std::mem::swap(&mut root.gamerules, &mut world.gamerules);
    std::mem::swap(&mut root.gamerules_dirty, &mut world.gamerules_dirty);
    std::mem::swap(&mut root.scoreboard, &mut world.scoreboard);
    std::mem::swap(&mut root.command_storage, &mut world.command_storage);
    std::mem::swap(
        &mut root.command_storage_dirty,
        &mut world.command_storage_dirty,
    );
    std::mem::swap(&mut root.maps, &mut world.maps);
    std::mem::swap(&mut root.scoreboard_dirty, &mut world.scoreboard_dirty);
    std::mem::swap(&mut root.bossbars_dirty, &mut world.bossbars_dirty);
    std::mem::swap(&mut root.bossbars, &mut world.bossbars);
    std::mem::swap(&mut root.difficulty, &mut world.difficulty);
    std::mem::swap(
        &mut root.journaled_difficulty,
        &mut world.journaled_difficulty,
    );
    std::mem::swap(
        &mut root.dimension_globals.reserved_uuids,
        &mut world.dimension_globals.reserved_uuids,
    );
    // Settlement registries are per-dimension authorities and never swap;
    // only the allocator sequence (globally monotonic) and tick budget move.
    std::mem::swap(
        &mut root.settlement_sequence,
        &mut world.settlement_sequence,
    );
    std::mem::swap(&mut root.tick_budget, &mut world.tick_budget);
    std::mem::swap(&mut root.tick_control, &mut world.tick_control);
}

pub(crate) fn run_tick(
    state: &mut TickState,
    inputs: &[PlayerInput],
) -> (
    TickOutput,
    Vec<(ConnectionId, Vec<PlayerChange>)>,
    TickTimes,
) {
    state.tick_budget.reset(state.tick);
    // Absent dimensions have no pass to release their cold-start guarantees.
    for dimension in Dimension::ALL {
        if dimension != state.dimension && !state.other_dimensions.contains_key(&dimension) {
            state
                .tick_budget
                .finish(dimension, crate::tick_budget::Consumer::Allocator);
            state
                .tick_budget
                .finish(dimension, crate::tick_budget::Consumer::Brain);
        }
    }
    let phase_start = std::env::var_os("MC_CORE_PHASE_LOG")
        .is_some()
        .then(std::time::Instant::now);
    if state.other_dimensions.is_empty() {
        let (mut output, mut drains, times) = crate::tick::run_dimension_tick(state, inputs);
        let local_done = phase_start.map(|start| start.elapsed()).unwrap_or_default();
        super::travel::finish(state, &mut output, &mut drains);
        crate::settlements::finish(state);
        let travel_done = phase_start.map(|start| start.elapsed()).unwrap_or_default();
        super::globals::finish(state, &mut output, &mut drains);
        if let Some(start) = phase_start {
            let total = start.elapsed();
            eprintln!(
                "dimension coordinator ms: tick={} setup=0.00 locals={:.2} collect=0.00 travel={:.2} globals={:.2} total={:.2}",
                state.tick,
                local_done.as_secs_f64() * 1000.0,
                travel_done.saturating_sub(local_done).as_secs_f64() * 1000.0,
                total.saturating_sub(travel_done).as_secs_f64() * 1000.0,
                total.as_secs_f64() * 1000.0,
            );
        }
        return (output, drains, times);
    }
    state.dimension_globals.active = true;
    for world in state.other_dimensions.values_mut() {
        world.dimension_globals.active = true;
    }
    crate::chunk_tickets::publish_online(state);
    // Build only for command-bearing ticks; normal movement ticks do not scan
    // entity custody. The set follows the command owner while worlds are taken.
    let mut reserved = std::mem::take(&mut state.dimension_globals.reserved_uuids);
    reserved.clear();
    if inputs.iter().any(|input| {
        matches!(
            input,
            PlayerInput::Command { .. } | PlayerInput::ConsoleCommand(_)
        )
    }) {
        for world in
            std::iter::once(&*state).chain(state.other_dimensions.values().map(|world| &**world))
        {
            reserved.extend(
                world
                    .entities
                    .slots()
                    .map(|slot| world.entities.row(slot).uuid),
            );
            reserved.extend(world.offline_players.keys().copied());
            reserved.extend(
                world
                    .connections
                    .all_connections()
                    .filter_map(|connection| world.connections.uuid_of(connection)),
            );
        }
        reserved.extend(inputs.iter().filter_map(|input| {
            if let PlayerInput::Join { uuid, .. } = input {
                Some(*uuid)
            } else {
                None
            }
        }));
    }
    state.dimension_globals.reserved_uuids = reserved;
    let mut routes = std::mem::take(&mut state.dimension_routes);
    routes.clear();
    routes.extend(inputs.iter().map(|input| input_dimension(state, input)));
    let mut worlds = std::mem::take(&mut state.other_dimensions);
    let root_dimension = state.dimension;
    let setup_time = phase_start.map(|start| start.elapsed()).unwrap_or_default();
    let (mut output, mut drains, mut times) = crate::tick::run_dimension_tick(
        state,
        inputs
            .iter()
            .zip(&routes)
            .filter_map(|(input, dimension)| (*dimension == root_dimension).then_some(input)),
    );
    let mut local_time = phase_start
        .map(|start| start.elapsed().saturating_sub(setup_time))
        .unwrap_or_default();
    let mut high_water = state.entities.wire_id_high_water();
    for (&dimension, world) in &mut worlds {
        world.entities.set_next_wire_id(high_water);
        world.tick = state.tick - 1;
        // Each local phase advances its clock once; remove that extra step.
        world.day_time = state
            .day_time
            .wrapping_sub(i64::from(crate::tick_control::advances_day(state)));
        swap_globals(state, world);
        let child_start = phase_start.map(|_| std::time::Instant::now());
        // Unknown-connection metadata runs in the root before Join resolves
        // UUID locality to this dimension. Move only that pending value,
        // rather than copying it into every world's connection registry.
        for (input, target) in inputs.iter().zip(&routes) {
            if *target == dimension
                && let PlayerInput::Join { connection, .. } = input
                && state.connections.entity_of(*connection).is_none()
                && let Some(view) = state.connections.view_distance_of(*connection)
            {
                state.connections.remove_by_connection(*connection);
                world.connections.set_view_distance(*connection, view);
            }
        }
        let (child_output, mut child_drains, child_times) = crate::tick::run_dimension_tick(
            world,
            inputs
                .iter()
                .zip(&routes)
                .filter_map(|(input, target)| (*target == dimension).then_some(input)),
        );
        local_time += child_start.map(|start| start.elapsed()).unwrap_or_default();
        swap_globals(state, world);
        state.day_time = world.day_time;
        high_water = world.entities.wire_id_high_water();
        drains.append(&mut child_drains);
        state.disconnects.append(&mut world.disconnects);
        times.input_us += child_times.input_us;
        times.chunks_us += child_times.chunks_us;
        times.entity_us += child_times.entity_us;
        times.merge_us += child_times.merge_us;
        times.block_us += child_times.block_us;
        times.commit_us += child_times.commit_us;
        times.snapshot_us += child_times.snapshot_us;
        times.views_us += child_times.views_us;
        times.publish_us += child_times.publish_us;
        Arc::get_mut(&mut output.snapshot)
            .expect("unpublished root snapshot")
            .dimensions
            .insert(dimension, child_output);
    }
    state.entities.set_next_wire_id(high_water);
    for world in worlds.values_mut() {
        world.entities.set_next_wire_id(high_water);
    }
    let root_snapshot = Arc::get_mut(&mut output.snapshot).expect("unpublished root snapshot");
    root_snapshot.day_time = state.day_time;
    root_snapshot.border = state.border;
    for child in root_snapshot.dimensions.values_mut() {
        let snapshot = Arc::get_mut(&mut child.snapshot).expect("unpublished child snapshot");
        snapshot.day_time = state.day_time;
    }
    state.other_dimensions = worlds;
    state.dimension_routes = routes;
    let local_done = phase_start.map(|start| start.elapsed()).unwrap_or_default();
    super::travel::finish(state, &mut output, &mut drains);
    crate::settlements::finish(state);
    let travel_done = phase_start.map(|start| start.elapsed()).unwrap_or_default();
    super::globals::finish(state, &mut output, &mut drains);
    if let Some(start) = phase_start {
        let total = start.elapsed();
        eprintln!(
            "dimension coordinator ms: tick={} setup={:.2} locals={:.2} collect={:.2} travel={:.2} globals={:.2} total={:.2}",
            state.tick,
            setup_time.as_secs_f64() * 1000.0,
            local_time.as_secs_f64() * 1000.0,
            local_done
                .saturating_sub(setup_time)
                .saturating_sub(local_time)
                .as_secs_f64()
                * 1000.0,
            travel_done.saturating_sub(local_done).as_secs_f64() * 1000.0,
            total.saturating_sub(travel_done).as_secs_f64() * 1000.0,
            total.as_secs_f64() * 1000.0,
        );
    }
    (output, drains, times)
}
