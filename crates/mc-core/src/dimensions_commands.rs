//! Resolve global player commands once, then mutate their native owning worlds.
use super::Dimension;
use super::travel::{Request, locate, world};
use crate::boundary::{ChangeSet, EntityId};
use crate::commands::{Command, SelectorKind, SelectorSort, Target};
use crate::tick::TickState;
use std::collections::BTreeMap;

pub(crate) fn target(command: &Command) -> Option<&Target> {
    match command {
        Command::EffectGive { target, .. }
        | Command::EffectClear { target, .. }
        | Command::GameMode { target, .. }
        | Command::Give { target, .. }
        | Command::Clear { target, .. }
        | Command::Kill { target, .. }
        | Command::Experience { target, .. }
        | Command::ExperienceSet { target, .. }
        | Command::ExperienceQuery { target, .. }
        | Command::Advancement { target, .. }
        | Command::Message { target, .. }
        | Command::TellRaw { target, .. }
        | Command::Title { target, .. } => Some(target),
        Command::Enchant(command) => Some(&command.target),
        Command::Damage(command) => Some(&command.target),
        Command::World(crate::command_world::WorldCommand::SpawnPoint { target, .. }) => {
            Some(target)
        }
        _ => None,
    }
}

fn target_mut(command: &mut Command) -> &mut Target {
    match command {
        Command::EffectGive { target, .. }
        | Command::EffectClear { target, .. }
        | Command::GameMode { target, .. }
        | Command::Give { target, .. }
        | Command::Clear { target, .. }
        | Command::Kill { target, .. }
        | Command::Experience { target, .. }
        | Command::ExperienceSet { target, .. }
        | Command::ExperienceQuery { target, .. }
        | Command::Advancement { target, .. }
        | Command::Message { target, .. }
        | Command::TellRaw { target, .. }
        | Command::Title { target, .. } => target,
        Command::Enchant(command) => &mut command.target,
        Command::Damage(command) => &mut command.target,
        _ => unreachable!("target-bearing native command"),
    }
}

pub(crate) fn global_target(target: &Target) -> bool {
    // Unqualified @e/@p join @a/@r in global routing: vanilla
    // EntitySelector.findEntities/findPlayers iterate all levels when the
    // selector carries no world-limiting option. Explicit position,
    // distance or volume constrains the dimension instead.
    match target {
        Target::Player(_) | Target::AllPlayers => true,
        Target::Selector(selector) => {
            matches!(
                selector.kind,
                SelectorKind::AllPlayers | SelectorKind::Random | SelectorKind::Entities
            ) && selector.distance.is_none()
                && selector.position.iter().all(Option::is_none)
                && selector.extent.iter().all(Option::is_none)
        }
        _ => false,
    }
}

pub(crate) fn resolve(
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
    if !global_target(target) {
        return world(state, request.dimension)
            .map(|world| crate::command_execution::resolve_target(world, request.executor, target))
            .unwrap_or_default();
    }
    let mut selected = Vec::new();
    let selector = if let Target::Selector(selector) = target {
        let mut selector = selector.clone();
        selector.position = origin.map(Some);
        selector.limit = Some(usize::MAX);
        selector.sort = Some(SelectorSort::Arbitrary);
        Some(Target::Selector(selector))
    } else {
        None
    };
    for world in std::iter::once(state).chain(state.other_dimensions.values().map(|world| &**world))
    {
        selected.extend(crate::command_execution::resolve_target(
            world,
            request.executor,
            selector.as_ref().unwrap_or(target),
        ));
    }
    let Target::Selector(selector) = target else {
        selected.sort_unstable();
        return selected;
    };
    let candidates = selected
        .into_iter()
        .map(|entity| {
            let (dimension, slot) = locate(state, entity).expect("selected entity");
            let position = world(state, dimension)
                .expect("selected world")
                .entities
                .pos(slot);
            let distance = position
                .iter()
                .zip(origin)
                .map(|(coordinate, origin)| (coordinate - origin).powi(2))
                .sum::<f64>();
            (entity, distance)
        })
        .collect();
    crate::command_execution::order_and_limit(state, request.executor, selector, candidates)
}

pub(crate) fn commit(world: &mut TickState, changes: &mut ChangeSet) {
    // Like any out-of-merge world edit (`portals::commit_removals`), a command
    // settles everything it staged (damage, drops, destroyed container custody
    // and viewer closure) at its own boundary: the next tick's inputs must never
    // reach storage whose removal was already snapshotted.
    let (_, mut events, mut spawned, mut removed) = world.commit_merge_effects();
    changes.entity_events.append(&mut events);
    changes.entities_spawned.append(&mut spawned);
    changes.entities_removed.append(&mut removed);
    crate::entity_life::commit_drops(world, &mut changes.entities_spawned);
    if !world.merge_block_writes.is_empty() {
        let mut writes = std::mem::take(&mut world.merge_block_writes);
        let mut spawned = Vec::new();
        crate::tick::pop_unsupported(world, &mut writes, &mut spawned);
        if crate::placement::shape_updates(&world.registry.blocks, &mut writes, &|pos| {
            world.world.block_at(pos.x, pos.y, pos.z)
        })
        .is_some()
        {
            for (pos, block) in &writes {
                world.world.set_block(pos.x, pos.y, pos.z, *block);
            }
        }
        let count = writes.len();
        for index in 0..count {
            let pos = writes[index].0;
            crate::plants::neighbor_updates(world, pos, &mut writes, &mut spawned);
        }
        crate::falling::start(world, &mut writes, &mut spawned);
        crate::signs::invalidate(world, &writes);
        crate::redstone::neighbours(world, &writes);
        crate::fluids::schedule(world, &writes);
        crate::portals::nether::after_blocks(world, &mut writes);
        crate::shrieker::after_blocks(world, &writes);
        crate::end_portals::after_blocks(world, &writes);
        crate::conduit::after_blocks(world, &writes);
        crate::block_entity_defaults::after_blocks(world, &writes);
        crate::trial_spawner::after_blocks(world, &writes);
        crate::vault::after_blocks(world, &writes);
        crate::block_log::after_blocks(world);
        world.world.fold_overlay();
        changes.entities_spawned.append(&mut spawned);
        changes.blocks.extend(
            writes
                .drain(..)
                .map(|(position, state)| crate::boundary::BlockChange { position, state }),
        );
        world.merge_block_writes = writes;
    }
    changes
        .entities_appearance_changed
        .append(&mut world.appearance_changed);
}

pub(crate) fn execute(
    state: &mut TickState,
    mut request: Request,
    changes: &mut BTreeMap<Dimension, ChangeSet>,
) -> Result<crate::command_execution::CommandResult, String> {
    use crate::command_execution::CommandResult::{Completed, Failed, Success, Unavailable};
    let source_dimension = locate(state, request.executor)
        .map(|(dimension, _)| dimension)
        .unwrap_or(state.dimension);
    let source = world(state, source_dimension).expect("executor world");
    let (origin, _) = state
        .command_context
        .current
        .map(|context| (context.position, context.rotation))
        .or_else(|| crate::command_console::pose(source, request.executor))
        .ok_or("argument.entity.notfound.entity")?;
    let selected = resolve(
        state,
        &request,
        target(&request.command).expect("global command target"),
        origin,
    );
    if selected.is_empty() {
        let key = match request.command {
            Command::EffectGive { .. }
            | Command::EffectClear { .. }
            | Command::Kill { .. }
            | Command::Enchant(_) => "argument.entity.notfound.entity",
            _ => "argument.entity.notfound.player",
        };
        return Err(key.into());
    }
    if matches!(request.command, Command::ExperienceQuery { .. }) && selected.len() != 1 {
        return Err("argument.player.toomany".into());
    }
    if let Command::World(crate::command_world::WorldCommand::SpawnPoint {
        position,
        rotation,
        ..
    }) = &request.command
    {
        let location = crate::command_world::spawnpoint_location(
            source,
            request.executor,
            *position,
            *rotation,
        )
        .map_err(|error| error.message())?;
        let mut count = 0;
        for entity in selected {
            let (dimension, _) = locate(state, entity).expect("selected entity");
            let world = if dimension == state.dimension {
                &mut *state
            } else {
                state
                    .other_dimensions
                    .get_mut(&dimension)
                    .expect("selected world")
            };
            if let Some(player) = world.players.get_mut(&entity) {
                player.respawn_point = Some(location.0);
                player.respawn_dimension = source_dimension;
                player.respawn_forced = true;
                player.respawn_yaw = location.1[0];
                player.respawn_pitch = location.1[1];
                count += 1;
            }
        }
        if count == 0 {
            return Err("argument.entity.notfound.player".into());
        }
        state.push_player(
            request.connection,
            crate::boundary::PlayerChange::Chat {
                message: format!(
                    "Set spawn point for {count} player(s) to {}, {}, {}",
                    location.0[0], location.0[1], location.0[2]
                ),
            },
        );
        return Ok(Completed);
    }
    let title_action = match &request.command {
        Command::Title { action, .. } => Some(action.clone()),
        _ => None,
    };
    if let Some(action) = title_action {
        return execute_title_global(state, request, changes, selected, action).map(|()| Completed);
    }
    let mut result = Failed;
    for entity in selected {
        let (dimension, _) = locate(state, entity).expect("selected entity");
        *target_mut(&mut request.command) = Target::Entity(entity);
        let changes = changes.entry(dimension).or_default();
        let outcome = if dimension == state.dimension {
            let outcome = crate::command_execution::execute(
                state,
                request.connection,
                request.executor,
                &request.command,
                changes,
            );
            commit(state, changes);
            outcome
        } else {
            let mut world = state
                .other_dimensions
                .remove(&dimension)
                .expect("selected world");
            super::runtime::swap_globals(state, &mut world);
            let outcome = crate::command_execution::execute(
                &mut world,
                request.connection,
                request.executor,
                &request.command,
                changes,
            );
            commit(&mut world, changes);
            super::runtime::swap_globals(state, &mut world);
            state.other_dimensions.insert(dimension, world);
            outcome
        };
        result = match (result, outcome) {
            (Unavailable, _) | (_, Unavailable) => Unavailable,
            (Completed, _) | (_, Completed) => Completed,
            (Success(total), Success(value)) => Success(total.wrapping_add(value)),
            (Success(total), Failed) => Success(total),
            (Failed, outcome) => outcome,
        };
        super::travel::sync_wire_ids(state);
    }
    Ok(result)
}

/// `/title` across dimensions: send each player's `TitleChange` in their own
/// world (components resolve per player, as in `command_text::execute_title`),
/// but emit the `multiple` feedback once with the TOTAL count instead of once
/// per player the way the generic loop below would.
#[allow(clippy::needless_pass_by_value)]
fn execute_title_global(
    state: &mut TickState,
    request: Request,
    changes: &mut BTreeMap<Dimension, ChangeSet>,
    selected: Vec<EntityId>,
    action: crate::command_text::TitleAction,
) -> Result<(), String> {
    use crate::boundary::{PlayerChange, TitleChange};
    let total = selected.len();
    for &entity in &selected {
        let (dimension, _) = locate(state, entity).expect("selected entity");
        let resolved = match &action {
            crate::command_text::TitleAction::ShowTitle(component)
            | crate::command_text::TitleAction::ShowSubtitle(component)
            | crate::command_text::TitleAction::ShowActionBar(component) => {
                let name = match world(state, dimension) {
                    Some(world) => crate::command_text::resolve_component(world, entity, component)
                        .map(Some)
                        .unwrap_or_else(|message| {
                            let executor_dimension =
                                locate(state, request.executor).map(|(dimension, _)| dimension);
                            let world = match executor_dimension {
                                Some(dimension) if dimension == state.dimension => &mut *state,
                                Some(dimension) => state
                                    .other_dimensions
                                    .get_mut(&dimension)
                                    .expect("executor world"),
                                None => &mut *state,
                            };
                            world.push_player(request.connection, PlayerChange::Chat { message });
                            None
                        }),
                    None => None,
                };
                name.map(|tag| match &action {
                    crate::command_text::TitleAction::ShowTitle(_) => TitleChange::Title(tag),
                    crate::command_text::TitleAction::ShowSubtitle(_) => TitleChange::Subtitle(tag),
                    _ => TitleChange::ActionBar(tag),
                })
            }
            crate::command_text::TitleAction::Clear { reset_times } => Some(TitleChange::Clear {
                reset_times: *reset_times,
            }),
            crate::command_text::TitleAction::Times {
                fade_in,
                stay,
                fade_out,
            } => Some(TitleChange::Times {
                fade_in: *fade_in,
                stay: *stay,
                fade_out: *fade_out,
            }),
        };
        if dimension == state.dimension {
            if let Some(connection) = state.connections.entity_connection(entity) {
                state.push_player(
                    connection,
                    PlayerChange::Title(resolved.expect("title change")),
                );
            }
            commit(state, changes.entry(dimension).or_default());
        } else {
            let mut world = state
                .other_dimensions
                .remove(&dimension)
                .expect("selected world");
            super::runtime::swap_globals(state, &mut world);
            if let Some(connection) = world.connections.entity_connection(entity) {
                world.push_player(
                    connection,
                    PlayerChange::Title(resolved.expect("title change")),
                );
            }
            let world_changes = changes.entry(dimension).or_default();
            commit(&mut world, world_changes);
            super::runtime::swap_globals(state, &mut world);
            state.other_dimensions.insert(dimension, world);
        }
        super::travel::sync_wire_ids(state);
    }
    let executor_dimension = locate(state, request.executor).map(|(dimension, _)| dimension);
    let label = if total == 1 {
        let world = world(state, executor_dimension.unwrap_or(state.dimension)).unwrap_or(state);
        crate::command_execution::entity_name(world, selected[0])
    } else {
        mc_nbt::Tag::Compound(vec![(
            "text".into(),
            mc_nbt::Tag::String(total.to_string()),
        )])
    };
    let key = format!(
        "commands.title.{}.{}",
        action.feedback_kind(),
        if total == 1 { "single" } else { "multiple" }
    );
    let world = match executor_dimension {
        Some(dimension) if dimension == state.dimension => &mut *state,
        Some(dimension) => state
            .other_dimensions
            .get_mut(&dimension)
            .expect("executor world"),
        None => &mut *state,
    };
    crate::command_execution::feedback(world, request.connection, &key, vec![label]);
    Ok(())
}
