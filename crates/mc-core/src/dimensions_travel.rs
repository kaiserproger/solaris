//! Cross-world custody moves occur at the shared commit boundary and WAL batch.
use super::Dimension;
use crate::boundary::{ChangeSet, ConnectionId, EntityId, PlayerChange, TickOutput};
use crate::commands::{Command, Destination, Target};
use crate::entities::EntitySlot;
use crate::tick::TickState;
use std::collections::BTreeMap;
use std::sync::Arc;

pub(crate) struct Request {
    pub connection: ConnectionId,
    pub executor: EntityId,
    pub dimension: Dimension,
    pub command: Command,
}

pub(crate) fn world(state: &TickState, dimension: Dimension) -> Option<&TickState> {
    if state.dimension == dimension {
        Some(state)
    } else {
        state.other_dimensions.get(&dimension).map(|world| &**world)
    }
}

pub(crate) fn locate(state: &TickState, id: EntityId) -> Option<(Dimension, EntitySlot)> {
    if let Some(slot) = state.entities.slot_of_any(id) {
        return Some((state.dimension, slot));
    }
    state
        .other_dimensions
        .iter()
        .find_map(|(&dimension, world)| {
            world.entities.slot_of_any(id).map(|slot| (dimension, slot))
        })
}

pub fn locate_uuid(state: &TickState, uuid: uuid::Uuid) -> Option<(Dimension, EntityId)> {
    std::iter::once(state)
        .chain(state.other_dimensions.values().map(|world| &**world))
        .find_map(|world| {
            world
                .entities
                .slot_of_uuid(uuid)
                .map(|slot| (world.dimension, world.entities.body(slot).id))
        })
}

fn targets(
    state: &TickState,
    request: &Request,
    target: &Target,
    origin: [f64; 3],
) -> Vec<EntityId> {
    if matches!(target, Target::Executor) {
        return locate(state, request.executor)
            .map(|_| vec![request.executor])
            .unwrap_or_default();
    }
    if super::commands::global_target(target) {
        return super::commands::resolve(state, request, target, origin);
    }
    let Some(world) = world(state, request.dimension) else {
        return Vec::new();
    };
    let target = match target {
        Target::Nearest => Target::Selector(Box::new(crate::commands::Selector {
            kind: crate::commands::SelectorKind::Nearest,
            entity_types: Vec::new(),
            names: Vec::new(),
            scores: Vec::new(),
            tags: Vec::new(),
            team: None,
            level: None,
            gamemode: None,
            pitch: None,
            yaw: None,
            nbt: Vec::new(),
            advancements: Vec::new(),
            predicates: Vec::new(),
            distance: None,
            position: origin.map(Some),
            extent: [None; 3],
            limit: None,
            sort: None,
        })),
        Target::Selector(selector) => {
            let mut selector = selector.clone();
            for (axis, coordinate) in selector.position.iter_mut().enumerate() {
                if coordinate.is_none() {
                    *coordinate = Some(origin[axis]);
                }
            }
            Target::Selector(selector)
        }
        other => other.clone(),
    };
    crate::command_execution::resolve_target(world, request.executor, &target)
}

pub(crate) fn context_targets(
    state: &TickState,
    context: crate::command_context::Context,
    target: &Target,
) -> Result<Vec<EntityId>, String> {
    let request = Request {
        connection: context.connection,
        executor: context.executor,
        dimension: context.dimension,
        command: Command::Kill {
            target: Target::Executor,
        },
    };
    Ok(targets(state, &request, target, context.position))
}

/// Temporarily select another native world under the shared authority. No entity
/// custody moves occur: /execute in changes a source, unlike /teleport.
pub(crate) fn execute_context(
    state: &mut TickState,
    text: &str,
    context: crate::command_context::Context,
    require_result: bool,
) -> Result<crate::command_execution::CommandResult, String> {
    if !state.command_context.shared {
        return Err("Cross-world execution requires the shared tick boundary".into());
    }
    sync_wire_ids(state);
    let source = state.dimension;
    let mut destination = state
        .other_dimensions
        .remove(&context.dimension)
        .ok_or("Execute dimension unavailable")?;
    std::mem::swap(&mut state.command_context, &mut destination.command_context);
    std::mem::swap(&mut state.datapacks, &mut destination.datapacks);
    super::runtime::swap_globals(state, &mut destination);
    std::mem::swap(
        &mut state.other_dimensions,
        &mut destination.other_dimensions,
    );
    std::mem::swap(state, &mut destination);
    state.other_dimensions.insert(source, destination);
    let mut changes = state
        .command_context
        .changes
        .remove(&context.dimension)
        .unwrap_or_default();
    let result = crate::command_context::native(state, text, context, &mut changes, require_result);
    sync_wire_ids(state);
    append_changes(
        state
            .command_context
            .changes
            .entry(context.dimension)
            .or_default(),
        changes,
    );
    let mut original = state
        .other_dimensions
        .remove(&source)
        .expect("source retained by execution authority");
    std::mem::swap(state, &mut original);
    std::mem::swap(&mut state.other_dimensions, &mut original.other_dimensions);
    std::mem::swap(&mut state.command_context, &mut original.command_context);
    std::mem::swap(&mut state.datapacks, &mut original.datapacks);
    super::runtime::swap_globals(state, &mut original);
    state.other_dimensions.insert(context.dimension, original);
    result
}
fn execute_block_chain(
    state: &mut TickState,
    dimension: Dimension,
    pos: crate::boundary::BlockPos,
    changes: &mut ChangeSet,
) {
    if dimension == state.dimension {
        state.command_blocks_due.push(pos);
        crate::command_blocks::run(state, changes);
        return;
    }
    sync_wire_ids(state);
    let source = state.dimension;
    let Some(mut destination) = state.other_dimensions.remove(&dimension) else {
        return;
    };
    std::mem::swap(&mut state.command_context, &mut destination.command_context);
    std::mem::swap(&mut state.datapacks, &mut destination.datapacks);
    super::runtime::swap_globals(state, &mut destination);
    std::mem::swap(
        &mut state.other_dimensions,
        &mut destination.other_dimensions,
    );
    std::mem::swap(state, &mut destination);
    state.other_dimensions.insert(source, destination);
    let mut local = state
        .command_context
        .changes
        .remove(&dimension)
        .unwrap_or_default();
    state.command_blocks_due.push(pos);
    crate::command_blocks::run(state, &mut local);
    sync_wire_ids(state);
    append_changes(
        state.command_context.changes.entry(dimension).or_default(),
        local,
    );
    let mut original = state
        .other_dimensions
        .remove(&source)
        .expect("source retained by execution authority");
    std::mem::swap(state, &mut original);
    std::mem::swap(&mut state.other_dimensions, &mut original.other_dimensions);
    std::mem::swap(&mut state.command_context, &mut original.command_context);
    std::mem::swap(&mut state.datapacks, &mut original.datapacks);
    super::runtime::swap_globals(state, &mut original);
    state.other_dimensions.insert(dimension, original);
}

fn append_changes(target: &mut ChangeSet, mut source: ChangeSet) {
    target.blocks.append(&mut source.blocks);
    target.chunks_loaded.append(&mut source.chunks_loaded);
    target.chunks_unloaded.append(&mut source.chunks_unloaded);
    target.entities_spawned.append(&mut source.entities_spawned);
    target
        .entities_appearance_changed
        .append(&mut source.entities_appearance_changed);
    target.entities_removed.append(&mut source.entities_removed);
    target.entity_events.append(&mut source.entity_events);
    target.explosions.append(&mut source.explosions);
}

fn move_between(
    source: &mut TickState,
    destination: &mut TickState,
    id: EntityId,
    position: [f64; 3],
    rotation: [f32; 2],
    changes: &mut ChangeSet,
) {
    // Authentication presentations have no world-travel custody. Keep the
    // pending claimant beside its saved record until native unlock restores it.
    if source.entity_session_locked(id) {
        return;
    }
    let slot = source
        .entities
        .slot_of_any(id)
        .expect("transfer source exists");
    let connection = source.connections.entity_connection(id);
    let alive = source.entities.body(slot).alive;
    if let Some(villager) = &mut source.entities.row_mut(slot).villager {
        villager.navigation_target = None;
        villager.navigation_path.clear();
        villager.navigation_chunks.clear();
        villager.navigation_repath_at = 0;
        villager.sleeping = false;
    }
    crate::sleep::wake(source, id);
    crate::mounts::dismount(source, id);
    crate::mounts::death(source, slot);
    crate::leash::detach(source, slot);
    if let Some(connection) = connection {
        let rules = source.registry.item_rules();
        let effects = source
            .players
            .get_mut(&id)
            .map(|player| player.close(&rules))
            .unwrap_or_default();
        crate::tick::apply_player_effects(source, connection, id, effects, changes);
        source.open_containers.remove(&id);
        source.sign_editors.remove(&connection);
    }
    source
        .entities
        .transfer_to(slot, &mut destination.entities, position, rotation);
    if let Some(mut player) = source.players.remove(&id) {
        player.fall_distance = 0.0;
        destination.players.insert(id, player);
    }
    source.journaled_players.remove(&id);
    source.journaled_villagers.remove(&id);
    source.journaled_entity_state.remove(&id);
    if let Some(connection) = connection {
        if source.locked_sessions.remove(&connection) {
            destination.locked_sessions.insert(connection);
        }
        source
            .connections
            .transfer_to(connection, &mut destination.connections);
        if let Some(mut queue) = source.outbox.remove(&connection) {
            destination
                .outbox
                .entry(connection)
                .or_default()
                .append(&mut queue);
        }
        let game_mode = destination.players[&id].game_mode as u8;
        let portal_cooldown = destination
            .entities
            .row(
                destination
                    .entities
                    .slot_of_any(id)
                    .expect("transferred entity"),
            )
            .entity_life
            .portal
            .cooldown;
        if alive {
            let teleport_id = destination
                .players
                .get_mut(&id)
                .expect("transferred player")
                .begin_teleport();
            destination.push_player(
                connection,
                PlayerChange::DimensionChanged {
                    teleport_id,
                    dimension: destination.dimension,
                    position,
                    yaw: rotation[0],
                    pitch: rotation[1],
                    game_mode,
                    portal_cooldown,
                },
            );
            let air = destination.players[&id].air_change();
            destination.push_player(connection, air);
            crate::bossbar::sync_dimension(destination, connection);
        }
    }
    if alive && destination.players.contains_key(&id) {
        crate::advancements::trigger(
            destination,
            id,
            "minecraft:changed_dimension",
            serde_json::json!({
                "from": source.dimension.name(),
                "to": destination.dimension.name(),
            }),
        );
    }
    changes.entities_removed.push(id);
    // Leads held by a departing player also release in this same committed tick.
    crate::leash::tick(source, changes);
    crate::entity_life::commit_drops(source, &mut changes.entities_spawned);
    changes
        .entities_appearance_changed
        .append(&mut source.appearance_changed);
}

/// Portal admission can use this same commit-only primitive once its intents win.
pub(crate) fn change_dimension(
    state: &mut TickState,
    id: EntityId,
    dimension: Dimension,
    position: [f64; 3],
    rotation: [f32; 2],
    changes: &mut BTreeMap<Dimension, ChangeSet>,
) -> Result<(), String> {
    let (source, slot) = locate(state, id).ok_or_else(|| "Entity not found".to_owned())?;
    if world(state, dimension).is_none() {
        return Err(format!("Dimension {} unavailable", dimension.name()));
    }
    if !position.iter().all(|value| value.is_finite())
        || !rotation.iter().all(|value| value.is_finite())
        || position[0].abs() >= 30_000_000.0
        || position[2].abs() >= 30_000_000.0
        || position[1].abs() > 20_000_000.0
    {
        return Err("Invalid teleport position".to_owned());
    }
    if source == dimension {
        let world = if source == state.dimension {
            state
        } else {
            state
                .other_dimensions
                .get_mut(&source)
                .expect("source world")
        };
        crate::mounts::dismount(world, id);
        crate::sleep::wake(world, id);
        world.entities.set_pos(slot, position);
        world.entities.set_rot(slot, rotation[0], rotation[1]);
        world.entities.set_ground(slot, false);
        if let Some(player) = world.players.get_mut(&id) {
            player.fall_distance = 0.0;
        }
        if let Some(connection) = world.connections.entity_connection(id) {
            world.correct_position(connection, position, rotation[0], rotation[1]);
        }
        changes
            .entry(source)
            .or_default()
            .entities_appearance_changed
            .push(id);
        return Ok(());
    }
    if source == state.dimension {
        let mut target = state
            .other_dimensions
            .remove(&dimension)
            .expect("target world");
        move_between(
            state,
            &mut target,
            id,
            position,
            rotation,
            changes.entry(source).or_default(),
        );
        state.other_dimensions.insert(dimension, target);
    } else {
        let mut origin = state
            .other_dimensions
            .remove(&source)
            .expect("source world");
        if dimension == state.dimension {
            move_between(
                &mut origin,
                state,
                id,
                position,
                rotation,
                changes.entry(source).or_default(),
            );
        } else {
            let target = state
                .other_dimensions
                .get_mut(&dimension)
                .expect("target world");
            move_between(
                &mut origin,
                target,
                id,
                position,
                rotation,
                changes.entry(source).or_default(),
            );
        }
        state.other_dimensions.insert(source, origin);
    }
    changes
        .entry(dimension)
        .or_default()
        .entities_spawned
        .push(id);
    sync_wire_ids(state);
    Ok(())
}

pub(super) fn sync_wire_ids(state: &mut TickState) {
    let high_water = std::iter::once(&*state)
        .chain(state.other_dimensions.values().map(|world| &**world))
        .map(|world| world.entities.wire_id_high_water())
        .max()
        .unwrap_or(0);
    state.entities.set_next_wire_id(high_water);
    for world in state.other_dimensions.values_mut() {
        world.entities.set_next_wire_id(high_water);
    }
}

pub(crate) fn execute(
    state: &mut TickState,
    request: Request,
    changes: &mut BTreeMap<Dimension, ChangeSet>,
) -> Result<crate::command_execution::CommandResult, String> {
    if !matches!(request.command, Command::Teleport { .. }) {
        return super::commands::execute(state, request, changes);
    }
    let source_dimension = locate(state, request.executor)
        .map(|(dimension, _)| dimension)
        .unwrap_or(state.dimension);
    let source = world(state, source_dimension).expect("executor world");
    let (origin, rotation) = if let Some(context) = state.command_context.current {
        (context.position, context.rotation)
    } else {
        let (mut origin, rotation) = crate::command_console::pose(source, request.executor)
            .ok_or_else(|| "argument.entity.notfound.entity".to_owned())?;
        let scale = source_dimension.coordinate_scale() / request.dimension.coordinate_scale();
        origin[0] *= scale;
        origin[2] *= scale;
        (origin, rotation)
    };
    let Command::Teleport {
        target,
        destination,
        rotation: facing,
    } = &request.command
    else {
        return Err("Unsupported dimension command".to_owned());
    };
    let selected = targets(state, &request, target, origin);
    if selected.is_empty() {
        return Err("argument.entity.notfound.entity".to_owned());
    }
    let text =
        |value: String| mc_nbt::Tag::Compound(vec![("text".into(), mc_nbt::Tag::String(value))]);
    let label = if selected.len() == 1 {
        let (dimension, _) = locate(state, selected[0]).expect("selected entity");
        crate::command_execution::entity_name(
            world(state, dimension).expect("selected world"),
            selected[0],
        )
    } else {
        text(selected.len().to_string())
    };
    let (dimension, position, destination_rotation, destination_name) = match destination {
        Destination::Position(coords) => (
            request.dimension,
            crate::command_context::resolve_coords(state, *coords, origin, rotation),
            None,
            None,
        ),
        Destination::Entity(target) => {
            let ids = targets(state, &request, target, origin);
            if ids.len() != 1 {
                return Err(if ids.is_empty() {
                    "argument.entity.notfound.entity"
                } else {
                    "argument.entity.toomany"
                }
                .to_owned());
            }
            let (dimension, slot) = locate(state, ids[0]).expect("destination entity");
            let world = world(state, dimension).expect("destination world");
            let [yaw, pitch, _] = world.entities.rot(slot);
            (
                dimension,
                world.entities.pos(slot),
                Some([yaw, pitch]),
                Some(crate::command_execution::entity_name(world, ids[0])),
            )
        }
    };
    for &entity in &selected {
        let (dimension_before, slot) = locate(state, entity).expect("selected entity");
        let rot = world(state, dimension_before)
            .expect("selected world")
            .entities
            .rot(slot);
        let mut rot = destination_rotation.unwrap_or([rot[0], rot[1]]);
        if let Some(facing) = facing {
            rot = [
                facing[0].resolve(f64::from(rotation[0])) as f32,
                facing[1].resolve(f64::from(rotation[1])) as f32,
            ];
        }
        let wrap = |angle: f32| (angle + 180.0).rem_euclid(360.0) - 180.0;
        rot = [wrap(rot[0]), wrap(rot[1]).clamp(-90.0, 90.0)];
        change_dimension(state, entity, dimension, position, rot, changes)
            .map_err(|_| "commands.teleport.invalidPosition".to_owned())?;
    }
    let suffix = if selected.len() == 1 {
        "single"
    } else {
        "multiple"
    };
    let mut args = vec![label];
    let kind = if let Some(name) = destination_name {
        args.push(name);
        "entity"
    } else {
        args.extend(position.map(|coordinate| text(coordinate.to_string())));
        "location"
    };
    // The console executor is not an entity: its feedback stays in the current world.
    let dimension =
        locate(state, request.executor).map_or(state.dimension, |(dimension, _)| dimension);
    let world = if dimension == state.dimension {
        state
    } else {
        state
            .other_dimensions
            .get_mut(&dimension)
            .expect("executor world")
    };
    crate::command_execution::feedback(
        world,
        request.connection,
        &format!("commands.teleport.success.{kind}.{suffix}"),
        args,
    );
    Ok(crate::command_execution::CommandResult::Success(
        selected.len() as i32,
    ))
}

fn refresh(world: &mut TickState, output: &mut TickOutput, mut changes: ChangeSet) {
    let entity_changes = !changes.entities_spawned.is_empty()
        || !changes.entities_removed.is_empty()
        || !changes.entities_appearance_changed.is_empty()
        || !changes.entity_events.is_empty();
    if !entity_changes
        && changes.blocks.is_empty()
        && changes.chunks_loaded.is_empty()
        && changes.chunks_unloaded.is_empty()
        && changes.explosions.is_empty()
    {
        let snapshot =
            Arc::get_mut(&mut output.snapshot).expect("unpublished dimensional snapshot");
        snapshot.day_time = world.day_time;
        snapshot.day_time_rate =
            if crate::command_rules::boolean(world, crate::command_rules::BoolRule::ADVANCE_TIME) {
                1.0
            } else {
                0.0
            };
        snapshot.border = world.border;
        snapshot.raining = world.weather.0 != crate::commands::Weather::Clear;
        snapshot.rain_level = world.rain_level;
        snapshot.thunder_level = world.thunder_level;
        return;
    }
    let phase_start = std::env::var_os("MC_CORE_PHASE_LOG")
        .is_some()
        .then(std::time::Instant::now);
    world.refresh_active_chunks(output.tick);
    let want = world.desired_chunks();
    let desired_done = phase_start.map(|start| start.elapsed()).unwrap_or_default();
    changes.chunks_loaded.extend(world.world.ensure(&want));
    world.refresh_active_chunks(output.tick);
    let ensure_done = phase_start.map(|start| start.elapsed()).unwrap_or_default();
    let snapshot = Arc::get_mut(&mut output.snapshot).expect("unpublished dimensional snapshot");
    snapshot.day_time = world.day_time;
    snapshot.day_time_rate =
        if crate::command_rules::boolean(world, crate::command_rules::BoolRule::ADVANCE_TIME) {
            1.0
        } else {
            0.0
        };
    snapshot.border = world.border;
    snapshot.raining = world.weather.0 != crate::commands::Weather::Clear;
    snapshot.rain_level = world.rain_level;
    snapshot.thunder_level = world.thunder_level;
    snapshot.chunks = world.world.snapshot();
    let chunks_done = phase_start.map(|start| start.elapsed()).unwrap_or_default();
    if entity_changes {
        snapshot.entities = world.entities.views();
    }
    let views_done = phase_start.map(|start| start.elapsed()).unwrap_or_default();
    let output_changes =
        Arc::get_mut(&mut output.changes).expect("unpublished dimensional changes");
    output_changes.blocks.append(&mut changes.blocks);
    output_changes
        .entity_events
        .append(&mut changes.entity_events);
    output_changes.explosions.append(&mut changes.explosions);
    output_changes
        .chunks_unloaded
        .append(&mut changes.chunks_unloaded);
    output_changes
        .entities_spawned
        .append(&mut changes.entities_spawned);
    output_changes
        .entities_removed
        .append(&mut changes.entities_removed);
    output_changes
        .entities_appearance_changed
        .append(&mut changes.entities_appearance_changed);
    output_changes
        .chunks_loaded
        .append(&mut changes.chunks_loaded);
    if let Some(start) = phase_start {
        let total = start.elapsed();
        eprintln!(
            "dimension refresh ms: tick={} dimension={} desired={:.2} ensure={:.2} chunks={:.2} views={:.2} append={:.2} total={:.2} wanted={} loaded={}",
            world.tick,
            world.dimension.name(),
            desired_done.as_secs_f64() * 1000.0,
            ensure_done.saturating_sub(desired_done).as_secs_f64() * 1000.0,
            chunks_done.saturating_sub(ensure_done).as_secs_f64() * 1000.0,
            views_done.saturating_sub(chunks_done).as_secs_f64() * 1000.0,
            total.saturating_sub(views_done).as_secs_f64() * 1000.0,
            total.as_secs_f64() * 1000.0,
            want.len(),
            output_changes.chunks_loaded.len(),
        );
    }
}

pub(crate) fn finish(
    state: &mut TickState,
    output: &mut TickOutput,
    drains: &mut Vec<(ConnectionId, Vec<PlayerChange>)>,
) {
    let phase_start = std::env::var_os("MC_CORE_PHASE_LOG")
        .is_some()
        .then(std::time::Instant::now);
    let mut requests = std::mem::take(&mut state.dimension_requests);
    let mut respawns = std::mem::take(&mut state.dimension_respawns);
    for world in state.other_dimensions.values_mut() {
        requests.append(&mut world.dimension_requests);
        respawns.append(&mut world.dimension_respawns);
    }
    let mut jobs = std::mem::take(&mut state.command_context.jobs);
    for world in state.other_dimensions.values_mut() {
        jobs.append(&mut world.command_context.jobs);
    }
    if !requests.is_empty() || !respawns.is_empty() || !jobs.is_empty() {
        super::globals::roster(state);
    }
    let mut changes = BTreeMap::new();
    for request in requests.drain(..) {
        let connection = request.connection;
        let executor = request.executor;
        if let Err(message) = execute(state, request, &mut changes)
            && let Some((dimension, _)) = locate(state, executor)
        {
            let world = if dimension == state.dimension {
                &mut *state
            } else {
                state
                    .other_dimensions
                    .get_mut(&dimension)
                    .expect("executor world")
            };
            crate::command_execution::feedback(world, connection, &message, Vec::new());
        }
    }
    state.command_context.shared = true;
    for job in jobs.drain(..) {
        let dimension = state.dimension;
        let mut local = changes.remove(&dimension).unwrap_or_default();
        state.command_context.changes = changes;
        if let Some(pos) = job.block {
            execute_block_chain(state, job.context.dimension, pos, &mut local);
        } else {
            crate::command_context::run(state, &job.text, job.context, &mut local);
        }
        changes = std::mem::take(&mut state.command_context.changes);
        append_changes(changes.entry(dimension).or_default(), local);
    }
    if !changes.is_empty() {
        sync_wire_ids(state);
    }
    state.command_context.shared = false;
    state.command_context.jobs = jobs;
    state.dimension_requests = requests;
    let requests_done = phase_start.map(|start| start.elapsed()).unwrap_or_default();
    finish_respawns(state, &mut changes, &mut respawns);
    state.dimension_respawns = respawns;
    let respawns_done = phase_start.map(|start| start.elapsed()).unwrap_or_default();
    crate::portals::travel::finish(state, &mut changes);
    let portals_done = phase_start.map(|start| start.elapsed()).unwrap_or_default();
    if let Some(changes) = changes.remove(&state.dimension) {
        refresh(state, output, changes);
    }
    for (dimension, changes) in changes {
        let world = state
            .other_dimensions
            .get_mut(&dimension)
            .expect("changed world");
        let snapshot = Arc::get_mut(&mut output.snapshot).expect("unpublished root snapshot");
        refresh(
            world,
            snapshot
                .dimensions
                .get_mut(&dimension)
                .expect("published world"),
            changes,
        );
    }
    let refresh_done = phase_start.map(|start| start.elapsed()).unwrap_or_default();
    for (connection, queue) in std::mem::take(&mut state.outbox) {
        if !queue.is_empty() {
            drains.push((connection, queue));
        }
    }
    for world in state.other_dimensions.values_mut() {
        for (connection, queue) in std::mem::take(&mut world.outbox) {
            if !queue.is_empty() {
                drains.push((connection, queue));
            }
        }
    }
    if let Some(start) = phase_start {
        let total = start.elapsed();
        eprintln!(
            "dimension travel ms: tick={} requests={:.2} respawns={:.2} portals={:.2} refresh={:.2} drain={:.2} total={:.2}",
            state.tick,
            requests_done.as_secs_f64() * 1000.0,
            respawns_done.saturating_sub(requests_done).as_secs_f64() * 1000.0,
            portals_done.saturating_sub(respawns_done).as_secs_f64() * 1000.0,
            refresh_done.saturating_sub(portals_done).as_secs_f64() * 1000.0,
            total.saturating_sub(refresh_done).as_secs_f64() * 1000.0,
            total.as_secs_f64() * 1000.0,
        );
    }
}

fn finish_respawns(
    state: &mut TickState,
    changes: &mut BTreeMap<Dimension, ChangeSet>,
    respawns: &mut Vec<EntityId>,
) {
    respawns.sort_unstable_by(|a, b| b.cmp(a));
    while let Some(id) = respawns.pop() {
        let Some((source, slot)) = locate(state, id) else {
            continue;
        };
        let origin = world(state, source).expect("respawn source");
        if origin.entities.body(slot).alive {
            continue;
        }
        let Some(player) = origin.players.get(&id) else {
            continue;
        };
        let connection = player.connection;
        let destination = if player.respawn_point.is_some() {
            player.respawn_dimension
        } else {
            Dimension::Overworld
        };
        let target = world(state, destination).expect("all vanilla dimensions loaded");
        let cell = player.respawn_point.unwrap_or(target.world.world_spawn());
        let position = [
            f64::from(cell[0]) + 0.5,
            f64::from(cell[1]),
            f64::from(cell[2]) + 0.5,
        ];
        let rotation = [player.respawn_yaw, player.respawn_pitch];
        if source != destination {
            change_dimension(state, id, destination, position, rotation, changes)
                .expect("validated durable respawn dimension and position");
        }
        let target = if destination == state.dimension {
            &mut *state
        } else {
            state
                .other_dimensions
                .get_mut(&destination)
                .expect("respawn target")
        };
        if !crate::tick::respawn_player(target, connection, changes.entry(destination).or_default())
        {
            target
                .deferred_uses
                .push(crate::boundary::PlayerInput::Respawn { connection });
        }
        changes
            .entry(destination)
            .or_default()
            .entities_appearance_changed
            .append(&mut target.appearance_changed);
        respawns.append(&mut target.dimension_respawns);
    }
}
