//! Tick-owned vanilla command execution and target selection.
use crate::boundary::{ChangeSet, ConnectionId, EntityId, PlayerChange};
use crate::commands::{
    Command, Destination, Selector, SelectorKind, SelectorSort, Target, Weather,
};
use crate::entities::EntityKind;
use crate::tick::{DamagePiece, TickState};
use mc_nbt::{ListTag, Tag};

/// Native execution outcome, independent of translated player feedback.
/// `Success(0)` is distinct from failure; unavailable results are rejected
/// before executing a command with an execute-store callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommandResult {
    Success(i32),
    Failed,
    /// The command succeeded, but does not expose a numerical result.
    Completed,
    Unavailable,
}

use CommandResult::{Failed, Success, Unavailable};
mod selectors;
pub(crate) use selectors::*;
mod settlement;
use settlement::*;

pub(crate) fn supports_store_result(command: &Command) -> bool {
    match command {
        Command::Kill { .. }
        | Command::Summon { .. }
        | Command::Attribute(_)
        | Command::Item(_)
        | Command::Loot(_)
        | Command::Give { .. }
        | Command::Clear { .. }
        | Command::EffectGive { .. }
        | Command::EffectClear { .. }
        | Command::Teleport { .. }
        | Command::Experience { .. }
        | Command::ExperienceSet { .. }
        | Command::ExperienceQuery { .. }
        | Command::TimeQuery(_)
        | Command::DifficultyQuery
        | Command::DebugStart
        | Command::DebugStop
        | Command::DebugFunction { .. }
        | Command::GameRule { value: None, .. }
        | Command::WorldBorder(crate::command_border::BorderCommand::Get)
        | Command::World(
            crate::command_world::WorldCommand::SetBlock { .. }
            | crate::command_world::WorldCommand::Fill { .. }
            | crate::command_world::WorldCommand::Clone { .. },
        ) => true,
        Command::Scoreboard(command) => command.supports_store_result(),
        _ => false,
    }
}

fn text(value: impl ToString) -> Tag {
    Tag::Compound(vec![("text".into(), Tag::String(value.to_string()))])
}

fn translated(key: impl Into<String>, arguments: Vec<Tag>) -> Tag {
    let mut fields = vec![("translate".into(), Tag::String(key.into()))];
    if !arguments.is_empty() {
        fields.push((
            "with".into(),
            Tag::List(ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: arguments,
            }),
        ));
    }
    Tag::Compound(fields)
}

pub(crate) fn feedback(
    state: &mut TickState,
    connection: ConnectionId,
    key: &str,
    arguments: Vec<Tag>,
) {
    state.push_player(
        connection,
        PlayerChange::StyledChat {
            content: translated(key, arguments),
        },
    );
}

fn error(state: &mut TickState, connection: ConnectionId, message: impl Into<String>) {
    state.push_player(
        connection,
        PlayerChange::Chat {
            message: message.into(),
        },
    );
}

fn entity_type_name(state: &TickState, entity: EntityId) -> Option<&'static str> {
    let body = state.entities.body(state.entities.slot_of(entity)?);
    match body.kind {
        EntityKind::Player { .. } => Some("minecraft:player"),
        EntityKind::Mob { type_id }
        | EntityKind::Item { type_id }
        | EntityKind::ExperienceOrb { type_id } => {
            mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
                .map(|contract| contract.name)
        }
    }
}

/// Entity-type tag membership from the tag registry (e.g. `#minecraft:skeletons`).
fn entity_type_tags(state: &TickState, entity: EntityId) -> Vec<&str> {
    let Some(slot) = state.entities.slot_of_any(entity) else {
        return Vec::new();
    };
    let type_id = match state.entities.body(slot).kind {
        EntityKind::Player { .. } => return Vec::new(),
        EntityKind::Mob { type_id }
        | EntityKind::Item { type_id }
        | EntityKind::ExperienceOrb { type_id } => type_id,
    };
    state
        .registry
        .tags
        .registries
        .iter()
        .filter(|(registry, _)| registry.as_str() == "minecraft:entity_type")
        .flat_map(|(_, tags)| tags.iter())
        .filter(|(_, entries)| entries.contains(&(type_id as i32)))
        .map(|(tag, _)| tag.as_str())
        .collect()
}

/// Vanilla plain display name for a nameless entity: Java
/// `Entity.getName` falls back to the entity-type description while the
/// `name=` predicate reads `getPlainTextName`, so `@e[name=Pig]`
/// selects an unnamed pig rather than requiring `minecraft:pig`.
fn plain_entity_name(state: &TickState, entity: EntityId) -> String {
    entity_type_name(state, entity)
        .and_then(|name| name.strip_prefix("minecraft:"))
        .map(|name| {
            let mut plain = String::with_capacity(name.len());
            for (index, word) in name.split('_').enumerate() {
                if index > 0 {
                    plain.push(' ');
                }
                let mut chars = word.chars();
                plain.extend(chars.next().iter().flat_map(|first| first.to_uppercase()));
                plain.push_str(chars.as_str());
            }
            plain
        })
        .unwrap_or_default()
}

pub(crate) fn entity_name(state: &TickState, entity: EntityId) -> Tag {
    if entity == crate::command_console::EXECUTOR {
        return text("Server");
    }
    if let Some(name) = state.connections.name_of(entity) {
        return text(name);
    }
    if let Some(player) = state
        .dimension_globals
        .roster
        .iter()
        .find(|player| player.entity == entity)
    {
        return text(&player.name);
    }
    if let Some(slot) = state.entities.slot_of(entity)
        && let Some(name) = &state.entities.row(slot).appearance.custom_name
    {
        return text(name);
    }
    let name = entity_type_name(state, entity)
        .unwrap_or("minecraft:generic")
        .replace(':', ".");
    translated(format!("entity.{name}"), Vec::new())
}

fn targets_label(state: &TickState, targets: &[EntityId]) -> Tag {
    if targets.len() == 1 {
        entity_name(state, targets[0])
    } else {
        text(targets.len())
    }
}

fn item_name(state: &TickState, item: &mc_data::Identifier) -> Tag {
    let category = if state.registry.blocks.block(item).is_some() {
        "block"
    } else {
        "item"
    };
    translated(
        format!("{category}.{}", item.as_str().replace(':', ".")),
        Vec::new(),
    )
}

fn mode_name(mode: crate::commands::GameMode) -> Tag {
    translated(
        format!(
            "gameMode.{}",
            match mode {
                crate::commands::GameMode::Survival => "survival",
                crate::commands::GameMode::Creative => "creative",
                crate::commands::GameMode::Adventure => "adventure",
                crate::commands::GameMode::Spectator => "spectator",
            }
        ),
        Vec::new(),
    )
}

fn difficulty_name(difficulty: crate::survival::Difficulty) -> Tag {
    translated(
        format!(
            "options.difficulty.{}",
            match difficulty {
                crate::survival::Difficulty::Peaceful => "peaceful",
                crate::survival::Difficulty::Easy => "easy",
                crate::survival::Difficulty::Normal => "normal",
                crate::survival::Difficulty::Hard => "hard",
            }
        ),
        Vec::new(),
    )
}

fn advancement_ids(
    tree: &mc_data::advancements::AdvancementTree,
    scope: crate::commands::AdvancementScope,
    advancement: Option<&str>,
) -> Result<Vec<String>, String> {
    use crate::commands::AdvancementScope;
    if scope == AdvancementScope::Everything {
        return Ok(tree.definitions.keys().cloned().collect());
    }
    let id = advancement.ok_or("Missing advancement")?;
    let definition = tree
        .definitions
        .get(id)
        .ok_or_else(|| format!("Unknown advancement '{id}'"))?;
    let mut ids = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    seen.insert(id);
    if matches!(scope, AdvancementScope::Through | AdvancementScope::Until) {
        let mut parent = definition.parent.as_deref();
        while let Some(id) = parent {
            if !seen.insert(id) {
                break;
            }
            let Some(definition) = tree.definitions.get(id) else {
                break;
            };
            ids.push(id.to_owned());
            parent = definition.parent.as_deref();
        }
    }
    ids.push(id.to_owned());
    if matches!(scope, AdvancementScope::From | AdvancementScope::Through) {
        let mut children = std::collections::BTreeMap::<&str, Vec<&str>>::new();
        for (id, definition) in &tree.definitions {
            if let Some(parent) = definition.parent.as_deref() {
                children.entry(parent).or_default().push(id);
            }
        }
        let mut pending = vec![id];
        while let Some(parent) = pending.pop() {
            if let Some(children) = children.get(parent) {
                for &child in children.iter().rev() {
                    if seen.insert(child) {
                        ids.push(child.to_owned());
                        pending.push(child);
                    }
                }
            }
        }
    }
    Ok(ids)
}

pub(crate) fn execute(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    command: &Command,
    changes: &mut ChangeSet,
) -> CommandResult {
    match command {
        Command::Attribute(command) => {
            return match crate::command_attribute::execute(state, connection, executor, command) {
                Ok(value) => Success(value),
                Err(message) => {
                    error(state, connection, message);
                    Failed
                }
            };
        }
        Command::Item(command) => {
            return match crate::command_item_replace::execute(state, connection, executor, command)
            {
                Ok(value) => Success(value),
                Err(message) => {
                    error(state, connection, message);
                    Failed
                }
            };
        }
        Command::Loot(command) => {
            return match crate::command_loot::execute(state, connection, executor, command) {
                Ok(value) => Success(value),
                Err(message) => {
                    error(state, connection, message);
                    Failed
                }
            };
        }
        Command::Advancement {
            target,
            grant,
            scope,
            advancement,
            criterion,
        } => {
            let ids =
                match advancement_ids(&state.registry.advancements, *scope, advancement.as_deref())
                {
                    Ok(ids) => ids,
                    Err(message) => {
                        error(state, connection, message);
                        return Failed;
                    }
                };
            if let (Some(id), Some(criterion)) = (advancement, criterion)
                && !state.registry.advancements.definitions[id]
                    .criteria
                    .contains_key(criterion)
            {
                error(
                    state,
                    connection,
                    format!("Unknown criterion '{criterion}' for advancement '{id}'"),
                );
                return Failed;
            }
            let targets = player_targets(state, executor, target);
            if targets.is_empty() {
                feedback(
                    state,
                    connection,
                    "argument.entity.notfound.player",
                    Vec::new(),
                );
                return Failed;
            }
            let mut count = 0;
            for &entity in &targets {
                count += if *grant {
                    crate::advancements::grant(state, entity, &ids, criterion.as_deref())
                } else {
                    crate::advancements::revoke(state, entity, &ids, criterion.as_deref())
                };
            }
            let operation = if *grant { "grant" } else { "revoke" };
            let result = if count == 0 { "failure" } else { "success" };
            let destination = if targets.len() == 1 { "one" } else { "many" };
            let label = targets_label(state, &targets);
            let (subject, arguments) = if let Some(criterion) = criterion {
                (
                    "criterion",
                    vec![
                        text(criterion),
                        text(advancement.as_deref().unwrap_or("")),
                        label,
                    ],
                )
            } else if ids.len() == 1 {
                ("one", vec![text(&ids[0]), label])
            } else {
                ("many", vec![text(ids.len()), label])
            };
            feedback(
                state,
                connection,
                &format!("commands.advancement.{operation}.{subject}.to.{destination}.{result}"),
                arguments,
            );
        }
        Command::EffectGive { .. } | Command::EffectClear { .. } => {
            return crate::effects::execute_command(state, connection, executor, command);
        }
        Command::GameMode { target, mode } => {
            let Some(targets) = command_players(state, connection, executor, target) else {
                return Failed;
            };
            let changed = game_mode(
                state, connection, executor, &targets, *mode, command, changes,
            );
            return if changed == 0 {
                Failed
            } else {
                Success(changed)
            };
        }
        Command::Give {
            target,
            item,
            count,
        } => {
            let Some(targets) = command_players(state, connection, executor, target) else {
                return Failed;
            };
            return give(state, connection, &targets, item, *count, command, changes);
        }
        Command::Clear { target, item, max } => {
            let Some(targets) = command_players(state, connection, executor, target) else {
                return Failed;
            };
            return clear(
                state,
                connection,
                &targets,
                item.as_ref(),
                *max,
                command,
                changes,
            );
        }
        Command::Experience {
            target,
            amount,
            levels,
        } => {
            let Some(targets) = command_players(state, connection, executor, target) else {
                return Failed;
            };
            for &target in &targets {
                apply_player_command(state, target, command, changes);
            }
            experience_success(state, connection, "add", *amount, *levels, &targets);
            return Success(targets.len() as i32);
        }
        Command::ExperienceSet {
            target,
            amount,
            levels,
        } => {
            let Some(targets) = command_players(state, connection, executor, target) else {
                return Failed;
            };
            // Points beyond the current level's bar are invalid; vanilla
            // fails only when no target accepts them.
            let accepts = |state: &TickState, id: &EntityId| {
                *levels
                    || *amount
                        < crate::survival::Experience::points_for_level(
                            state.players[id].vitals.experience.level,
                        )
            };
            if !targets.iter().any(|id| accepts(state, id)) {
                feedback(
                    state,
                    connection,
                    "commands.experience.set.points.invalid",
                    Vec::new(),
                );
                return Failed;
            }
            for &target in &targets {
                if accepts(state, &target) {
                    apply_player_command(state, target, command, changes);
                }
            }
            experience_success(state, connection, "set", *amount, *levels, &targets);
            return Success(targets.len() as i32);
        }
        Command::ExperienceQuery { target, levels } => {
            let Some(targets) = command_players(state, connection, executor, target) else {
                return Failed;
            };
            let [target] = targets[..] else {
                feedback(state, connection, "argument.player.toomany", Vec::new());
                return Failed;
            };
            let experience = state.players[&target].vitals.experience;
            let (key, amount) = if *levels {
                ("commands.experience.query.levels", experience.level)
            } else {
                (
                    "commands.experience.query.points",
                    (experience.progress
                        * crate::survival::Experience::points_for_level(experience.level) as f32)
                        .floor() as i32,
                )
            };
            feedback(
                state,
                connection,
                key,
                vec![entity_name(state, target), text(amount)],
            );
            return Success(amount);
        }
        Command::Enchant(enchant_command) => {
            let targets = resolve_target(state, executor, &enchant_command.target);
            if targets.is_empty() {
                feedback(
                    state,
                    connection,
                    "argument.entity.notfound.entity",
                    Vec::new(),
                );
                return Failed;
            }
            let single = targets.len() == 1;
            let mut successful = Vec::new();
            for target in targets {
                let Some(player) = state.players.get(&target) else {
                    if single {
                        feedback(
                            state,
                            connection,
                            "commands.enchant.failed.itemless",
                            vec![entity_name(state, target)],
                        );
                        return Failed;
                    }
                    continue;
                };
                if let Err(message) = crate::command_enchant::validate(
                    &state.registry.item_rules(),
                    &player.inventory,
                    enchant_command,
                ) {
                    if single {
                        error(state, connection, message);
                    }
                    continue;
                }
                let effects = state
                    .players
                    .get_mut(&target)
                    .expect("checked player exists")
                    .apply_command(&state.registry.item_rules(), command);
                if let Some(target_connection) = state.connections.entity_connection(target) {
                    crate::tick::apply_player_effects(
                        state,
                        target_connection,
                        target,
                        effects,
                        changes,
                    );
                }
                successful.push(target);
            }
            if !successful.is_empty() {
                let enchantment = translated(
                    format!(
                        "enchantment.{}",
                        enchant_command.enchantment.as_str().replace(':', ".")
                    ),
                    Vec::new(),
                );
                feedback(
                    state,
                    connection,
                    if successful.len() == 1 {
                        "commands.enchant.success.single"
                    } else {
                        "commands.enchant.success.multiple"
                    },
                    vec![enchantment, targets_label(state, &successful)],
                );
                return Success(successful.len() as i32);
            } else if !single {
                feedback(state, connection, "commands.enchant.failed", Vec::new());
            }
            return Failed;
        }
        Command::DebugGive {
            item,
            count,
            hotbar_slot,
        } => {
            let mut replacement = match item.stack(&state.registry.items) {
                Ok(stack) => stack,
                Err(reason) => {
                    error(state, connection, reason);
                    return Failed;
                }
            };
            replacement = if *count <= 0 {
                mc_data::ItemStack::EMPTY
            } else {
                replacement.count = (*count).min(255);
                replacement
            };
            if crate::plugin_inventories::below_player_replacement_floors(
                state,
                executor,
                &[(usize::from((*hotbar_slot).min(8)), &replacement)],
            ) {
                error(
                    state,
                    connection,
                    "Inventory is reserved by an active native operation",
                );
                return Failed;
            }
            let effects = state
                .players
                .get_mut(&executor)
                .map(|player| player.apply_command(&state.registry.item_rules(), command))
                .unwrap_or_default();
            crate::tick::apply_player_effects(state, connection, executor, effects, changes);
        }
        Command::DebugStart => {
            return crate::command_debug::start(state, connection, executor, command);
        }
        Command::DebugStop => {
            return crate::command_debug::stop(state, connection, executor, command);
        }
        Command::DebugFunction { name } => {
            return crate::command_debug::trace_function(
                state, connection, executor, name, command,
            );
        }
        Command::Teleport {
            target,
            destination,
            rotation: facing,
        } => {
            let targets = resolve_target(state, executor, target);
            if targets.is_empty() {
                feedback(
                    state,
                    connection,
                    "argument.entity.notfound.entity",
                    Vec::new(),
                );
                return Failed;
            }
            let (origin, rotation) = crate::command_console::pose(state, executor)
                .unwrap_or((state.world.world_spawn_position(), [0.0; 2]));
            let (to, destination_name, destination_rotation) = match destination {
                Destination::Position(coords) => (
                    crate::command_context::resolve_coords(state, *coords, origin, rotation),
                    None,
                    None,
                ),
                Destination::Entity(target) => {
                    // Java rejects a destination selector whose declared
                    // maximum exceeds one even when only one entity happens
                    // to match; `/tp @s @a` errors rather than teleporting.
                    if !single_target(target) {
                        feedback(state, connection, "argument.entity.toomany", Vec::new());
                        return Failed;
                    }
                    let ids = resolve_target(state, executor, target);
                    if ids.len() != 1 {
                        feedback(
                            state,
                            connection,
                            if ids.is_empty() {
                                "argument.entity.notfound.entity"
                            } else {
                                "argument.entity.toomany"
                            },
                            Vec::new(),
                        );
                        return Failed;
                    }
                    (
                        state.entities.pos(
                            state
                                .entities
                                .slot_of(ids[0])
                                .expect("resolved entity exists"),
                        ),
                        Some(entity_name(state, ids[0])),
                        Some(
                            state.entities.rot(
                                state
                                    .entities
                                    .slot_of(ids[0])
                                    .expect("resolved entity exists"),
                            ),
                        ),
                    )
                }
            };
            if !to.iter().all(|value| value.is_finite())
                || to[0].abs() >= 30_000_000.0
                || to[2].abs() >= 30_000_000.0
                || to[1].abs() > 20_000_000.0
            {
                feedback(
                    state,
                    connection,
                    "commands.teleport.invalidPosition",
                    Vec::new(),
                );
                return Failed;
            }
            for &target in &targets {
                let slot = state
                    .entities
                    .slot_of(target)
                    .expect("resolved entity exists");
                let current_rotation =
                    destination_rotation.unwrap_or_else(|| state.entities.rot(slot));
                let [yaw, pitch] = facing
                    .map(|coords| {
                        [
                            coords[0].resolve(f64::from(rotation[0])) as f32,
                            coords[1].resolve(f64::from(rotation[1])) as f32,
                        ]
                    })
                    .unwrap_or([current_rotation[0], current_rotation[1]]);
                let wrap = |angle: f32| (angle + 180.0).rem_euclid(360.0) - 180.0;
                state
                    .entities
                    .set_rot(slot, wrap(yaw), wrap(pitch).clamp(-90.0, 90.0));
                crate::mounts::dismount(state, target);
                state.entities.set_pos(slot, to);
                state.entities.set_ground(slot, false);
                if let Some(player) = state.players.get_mut(&target) {
                    player.fall_distance = 0.0;
                }
                if let Some(target_connection) = state.connections.entity_connection(target) {
                    let [yaw, pitch, _] = state.entities.rot(slot);
                    state.correct_position(target_connection, to, yaw, pitch);
                }
            }
            let suffix = if targets.len() == 1 {
                "single"
            } else {
                "multiple"
            };
            let mut args = vec![targets_label(state, &targets)];
            let kind = if let Some(name) = destination_name {
                args.push(name);
                "entity"
            } else {
                args.extend(to.map(text));
                "location"
            };
            feedback(
                state,
                connection,
                &format!("commands.teleport.success.{kind}.{suffix}"),
                args,
            );
            return Success(targets.len() as i32);
        }
        Command::Time { ticks, add } => {
            state.day_time = if *add {
                state.day_time.saturating_add(*ticks).max(0)
            } else {
                *ticks
            };
            feedback(
                state,
                connection,
                "commands.time.set.absolute",
                vec![text("minecraft:overworld"), text(state.day_time)],
            );
            return Success(1);
        }
        Command::TimeMarker { name, ticks } => {
            let delta = *ticks - state.day_time.rem_euclid(24000);
            state.day_time =
                state
                    .day_time
                    .saturating_add(if delta > 0 { delta } else { delta + 24000 });
            feedback(
                state,
                connection,
                "commands.time.set.time_marker",
                vec![text("minecraft:overworld"), text(name)],
            );
            return Success(1);
        }
        Command::TimeQuery(query) => {
            let value = query.value(state.tick_control.game_ticks(state.tick), state.day_time);
            feedback(state, connection, "commands.time.query", vec![text(value)]);
            return Success((value % i64::from(i32::MAX)) as i32);
        }
        Command::Weather {
            weather,
            duration_ticks,
        } => {
            if state.dimension_globals.active && !state.dimension.has_weather() {
                state.dimension_globals.broadcasts.push(
                    crate::dimensions::globals::Broadcast::Weather(*weather, *duration_ticks),
                );
            } else {
                crate::weather::set_command(state, *weather, *duration_ticks);
            }
            feedback(
                state,
                connection,
                match weather {
                    Weather::Clear => "commands.weather.set.clear",
                    Weather::Rain => "commands.weather.set.rain",
                    Weather::Thunder => "commands.weather.set.thunder",
                },
                Vec::new(),
            );
            return Success(1);
        }
        Command::Difficulty(difficulty) => {
            if state.difficulty == *difficulty {
                feedback(
                    state,
                    connection,
                    "commands.difficulty.failure",
                    vec![difficulty_name(*difficulty)],
                );
                return Failed;
            }
            state.difficulty = *difficulty;
            if state.dimension_globals.active {
                state.dimension_globals.broadcasts.push(
                    crate::dimensions::globals::Broadcast::Difficulty(difficulty.id()),
                );
            }
            let connections: Vec<_> = state.connections.all_connections().collect();
            for connection in connections {
                state.push_player(
                    connection,
                    PlayerChange::DifficultyChanged {
                        difficulty: difficulty.id(),
                    },
                );
            }
            feedback(
                state,
                connection,
                "commands.difficulty.success",
                vec![difficulty_name(*difficulty)],
            );
            return Success(1);
        }
        Command::DifficultyQuery => feedback(
            state,
            connection,
            "commands.difficulty.query",
            vec![difficulty_name(state.difficulty)],
        ),
        Command::GameRule { name, value } => {
            if let Some(value) = value
                && state.gamerules.get(name) != Some(value)
            {
                state.gamerules.insert(name.clone(), value.clone());
                state.gamerules_dirty = true;
            }
            let value = state
                .gamerules
                .get(name)
                .map(String::as_str)
                .or_else(|| crate::command_rules::default_value(name))
                .unwrap_or("")
                .to_owned();
            let result = match value.as_str() {
                "true" => Success(1),
                "false" => Success(0),
                _ => value.parse::<i32>().map_or(Unavailable, Success),
            };
            feedback(
                state,
                connection,
                if matches!(command, Command::GameRule { value: Some(_), .. }) {
                    "commands.gamerule.set"
                } else {
                    "commands.gamerule.query"
                },
                vec![text(name), text(value)],
            );
            return result;
        }
        Command::WorldBorder(command) => {
            let origin = crate::command_console::pose(state, executor).map_or_else(
                || state.world.world_spawn_position(),
                |(position, _)| position,
            );
            let message = crate::command_border::execute(&mut state.border, *command, origin);
            state.push_player(connection, PlayerChange::Chat { message });
            if matches!(command, crate::command_border::BorderCommand::Get) {
                return Success((state.border.size + 0.5).floor() as i32);
            }
            return CommandResult::Completed;
        }
        Command::Summon {
            entity,
            position,
            nbt,
        } => {
            let Some(facts) = state.registry.entity_types.facts_of(entity) else {
                error(
                    state,
                    connection,
                    format!("Unknown entity type '{}'", entity.as_str()),
                );
                return Failed;
            };
            if facts.flags.is_some_and(|flags| !flags.is_summonable()) {
                feedback(state, connection, "commands.summon.failed", Vec::new());
                return Failed;
            }
            if state.difficulty == crate::survival::Difficulty::Peaceful
                && facts.category.is_hostile()
            {
                feedback(
                    state,
                    connection,
                    "commands.summon.failed.peaceful",
                    Vec::new(),
                );
                return Failed;
            }
            let type_id = facts.protocol_id;
            let (origin, rotation) = crate::command_console::pose(state, executor)
                .unwrap_or((state.world.world_spawn_position(), [0.0; 2]));
            let at = position
                .as_ref()
                .map(|coords| {
                    crate::command_context::resolve_coords(state, *coords, origin, rotation)
                })
                .unwrap_or(origin);
            if at[0].abs() >= 30_000_000.0
                || at[2].abs() >= 30_000_000.0
                || at[1].abs() > 20_000_000.0
            {
                feedback(
                    state,
                    connection,
                    "commands.summon.invalidPosition",
                    Vec::new(),
                );
                return Failed;
            }
            if entity.as_str() == "minecraft:item" {
                let item = match crate::command_summon::prepare_item(state, nbt) {
                    Ok(item) => item,
                    Err(failure) => {
                        error(state, connection, failure.message());
                        return Failed;
                    }
                };
                let id = crate::tick::spawn_item_entity(state, at, item.stack);
                if let Some(slot) = state.entities.slot_of(id) {
                    if let Some(delay) = item.pickup_delay {
                        state.entities.body_mut(slot).pickup_delay = delay;
                    }
                    if let Some(age) = item.age {
                        state.entities.row_mut(slot).age = age;
                    }
                    if let Some(motion) = item.motion {
                        state.entities.set_vel(slot, motion.map(|v| v * 20.0));
                    }
                }
                changes.entities_spawned.push(id);
                feedback(
                    state,
                    connection,
                    "commands.summon.success",
                    vec![entity_name(state, id)],
                );
                return Success(1);
            }
            let image =
                match crate::command_summon::prepare(state, type_id, entity.as_str(), at, nbt) {
                    Ok(image) => image,
                    Err(failure) => {
                        error(state, connection, failure.message());
                        return Failed;
                    }
                };
            let health = state
                .registry
                .blueprint(type_id)
                .map_or(20.0, |blueprint| blueprint.params.max_health);
            let id = state.spawn_mob(type_id, at, health);
            crate::command_summon::apply(state, id, image);
            changes.entities_spawned.push(id);
            feedback(
                state,
                connection,
                "commands.summon.success",
                vec![entity_name(state, id)],
            );
            return Success(1);
        }
        Command::Kill { target } => {
            let targets = resolve_target(state, executor, target);
            if targets.is_empty() {
                feedback(
                    state,
                    connection,
                    "argument.entity.notfound.entity",
                    Vec::new(),
                );
                return Failed;
            }
            for &target in &targets {
                if let Some(slot) = state.entities.slot_of(target) {
                    state.merge_damage.push((
                        slot,
                        DamagePiece {
                            physical: None,
                            custom_armor: false,
                            amount: f32::MAX,
                            kind: crate::survival::DamageKind::GenericKill,
                            source: None,
                        },
                    ));
                }
            }
            feedback(
                state,
                connection,
                if targets.len() == 1 {
                    "commands.kill.success.single"
                } else {
                    "commands.kill.success.multiple"
                },
                vec![targets_label(state, &targets)],
            );
            return Success(targets.len() as i32);
        }
        Command::Damage(command) => {
            if let Err(failure) =
                crate::command_damage::execute(state, connection, executor, command, changes)
            {
                error(state, connection, failure.message());
                return Failed;
            }
            return Success(1);
        }
        Command::World(command) => {
            return crate::command_world::execute(state, connection, executor, command, changes);
        }
        Command::BlockLog(command) => {
            crate::block_log::execute(state, connection, executor, command);
        }
        Command::BossBar(command) => {
            crate::bossbar::execute(state, connection, executor, command);
        }
        Command::ForceLoad(command) => {
            let reply = crate::forceload::execute(state, executor, command);
            let args = reply.args.into_iter().map(text).collect();
            if reply.failure {
                let Tag::Compound(mut fields) = translated(reply.key, args) else {
                    unreachable!("translated builds a compound");
                };
                fields.push(("color".into(), Tag::String("red".into())));
                state.push_player(
                    connection,
                    PlayerChange::StyledChat {
                        content: Tag::Compound(fields),
                    },
                );
                return Failed;
            } else {
                feedback(state, connection, reply.key, args);
                return Success(1);
            }
        }
        Command::Scoreboard(command) => {
            let (message, result) = crate::scoreboard::execute(state, executor, command);
            error(state, connection, message);
            return result;
        }
        Command::Say(message) | Command::Me { message, .. } => {
            let name = entity_name(state, executor);
            let key = if matches!(command, Command::Say(_)) {
                "chat.type.announcement"
            } else {
                "chat.type.emote"
            };
            let message = if matches!(
                command,
                Command::Me {
                    selectors: false,
                    ..
                }
            ) {
                message.clone()
            } else {
                expand_message(state, executor, message)
            };
            let content = translated(key, vec![name, text(message)]);
            if connection == crate::command_console::CONNECTION {
                state.push_player(
                    connection,
                    PlayerChange::StyledChat {
                        content: content.clone(),
                    },
                );
            }
            let connections: Vec<_> = state.connections.all_connections().collect();
            for connection in connections {
                state.push_player(
                    connection,
                    PlayerChange::StyledChat {
                        content: content.clone(),
                    },
                );
            }
            if state.dimension_globals.active {
                state
                    .dimension_globals
                    .broadcasts
                    .push(crate::dimensions::globals::Broadcast::Chat(content));
            }
            return Success(1);
        }
        Command::TeamMessage { message, selectors } => {
            let sender = state.connections.name_of(executor).unwrap_or("").to_owned();
            let Some(team) = state
                .scoreboard
                .teams
                .values()
                .find(|team| team.members.contains(&sender))
            else {
                feedback(
                    state,
                    connection,
                    "commands.teammsg.failed.noteam",
                    Vec::new(),
                );
                return Failed;
            };
            let members = state.dimension_globals.active.then(|| team.members.clone());
            let display_name = team.display_name.clone();
            let recipients: Vec<_> = state
                .connections
                .all_connections()
                .filter(|recipient| !state.session_locked(*recipient))
                .filter(|recipient| {
                    state
                        .connections
                        .entity_of(*recipient)
                        .and_then(|id| state.connections.name_of(id))
                        .is_some_and(|name| team.members.contains(name))
                })
                .collect();
            let message = if *selectors {
                expand_message(state, executor, message)
            } else {
                message.clone()
            };
            for recipient in recipients {
                feedback(
                    state,
                    recipient,
                    if recipient == connection {
                        "chat.type.team.sent"
                    } else {
                        "chat.type.team.text"
                    },
                    vec![
                        text(&display_name),
                        entity_name(state, executor),
                        text(&message),
                    ],
                );
            }
            if let Some(members) = members {
                state.dimension_globals.broadcasts.push(
                    crate::dimensions::globals::Broadcast::TeamChat {
                        members,
                        content: translated(
                            "chat.type.team.text",
                            vec![
                                text(display_name),
                                entity_name(state, executor),
                                text(message),
                            ],
                        ),
                    },
                );
            }
        }
        Command::Message {
            target,
            message,
            selectors,
        } => {
            let targets: Vec<_> = player_targets(state, executor, target)
                .into_iter()
                .filter(|target| !state.entity_session_locked(*target))
                .collect();
            if targets.is_empty() {
                feedback(
                    state,
                    connection,
                    "argument.entity.notfound.player",
                    Vec::new(),
                );
                return Failed;
            }
            let message = if *selectors {
                expand_message(state, executor, message)
            } else {
                message.clone()
            };
            for target in targets {
                if let Some(recipient) = state.connections.entity_connection(target) {
                    feedback(
                        state,
                        recipient,
                        "commands.message.display.incoming",
                        vec![entity_name(state, executor), text(&message)],
                    );
                }
                feedback(
                    state,
                    connection,
                    "commands.message.display.outgoing",
                    vec![entity_name(state, target), text(&message)],
                );
            }
        }
        Command::TellRaw { target, message } => {
            return crate::command_text::execute_tellraw(
                state, connection, executor, target, message,
            );
        }
        Command::Title { target, action } => {
            return crate::command_text::execute_title(state, connection, executor, target, action);
        }
        Command::Settlement(command) => {
            settlement(state, connection, executor, command);
        }
        Command::Seed => feedback(
            state,
            connection,
            "commands.seed.success",
            vec![text(state.world.seed())],
        ),
        Command::Tick(tick) => crate::command_tick::execute(state, connection, tick),
        Command::Locate(locate) => {
            crate::command_locate::execute(state, connection, executor, locate)
        }
    }
    Unavailable
}

/// Command actor identity: the connection UUID string, never the display
/// name; the console acts as `"Server"`. Absent means no authenticated actor:
/// mutations and creates refuse rather than inventing a shared owner.
fn actor_identity(state: &TickState, connection: ConnectionId) -> Option<String> {
    if connection == crate::command_console::CONNECTION {
        return Some("Server".to_owned());
    }
    state
        .connections
        .uuid_of(connection)
        .map(|uuid| uuid.to_string())
}

/// Online players a player command addresses; reports "no player found"
/// and yields `None` when there are none. A non-player selector shape is a
/// parse-time admission error, reported before state changes and before the
/// no-match feedback.
fn command_players(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    target: &Target,
) -> Option<Vec<EntityId>> {
    if let Target::Selector(selector) = target
        && !matches!(
            selector.kind,
            SelectorKind::Nearest
                | SelectorKind::AllPlayers
                | SelectorKind::Random
                | SelectorKind::Executor
        )
    {
        feedback(
            state,
            connection,
            "argument.entity.selector.not_player",
            Vec::new(),
        );
        return None;
    }
    let targets = player_targets(state, executor, target);
    if targets.is_empty() {
        feedback(
            state,
            connection,
            "argument.entity.notfound.player",
            Vec::new(),
        );
        return None;
    }
    Some(targets)
}

/// Run a player command's state change on one target and sync its client.
fn apply_player_command(
    state: &mut TickState,
    target: EntityId,
    command: &Command,
    changes: &mut ChangeSet,
) {
    let effects = state
        .players
        .get_mut(&target)
        .expect("resolved player exists")
        .apply_command(&state.registry.item_rules(), command);
    if let Some(target_connection) = state.connections.entity_connection(target) {
        crate::tick::apply_player_effects(state, target_connection, target, effects, changes);
    }
}

fn success_suffix(targets: &[EntityId]) -> &'static str {
    if targets.len() == 1 {
        "single"
    } else {
        "multiple"
    }
}

fn experience_success(
    state: &mut TickState,
    connection: ConnectionId,
    verb: &str,
    amount: i32,
    levels: bool,
    targets: &[EntityId],
) {
    let unit = if levels { "levels" } else { "points" };
    let key = format!(
        "commands.experience.{verb}.{unit}.success.{}",
        success_suffix(targets)
    );
    let label = targets_label(state, targets);
    feedback(state, connection, &key, vec![text(amount), label]);
}

fn game_mode(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    targets: &[EntityId],
    mode: crate::commands::GameMode,
    command: &Command,
    changes: &mut ChangeSet,
) -> i32 {
    let mut changed = 0;
    for &target in targets {
        if state.players[&target].game_mode == mode {
            continue;
        }
        changed += 1;
        apply_player_command(state, target, command, changes);
        if target == executor {
            feedback(
                state,
                connection,
                "commands.gamemode.success.self",
                vec![mode_name(mode)],
            );
            continue;
        }
        let name = entity_name(state, target);
        feedback(
            state,
            connection,
            "commands.gamemode.success.other",
            vec![name, mode_name(mode)],
        );
        if crate::command_rules::boolean(
            state,
            crate::command_rules::BoolRule::SEND_COMMAND_FEEDBACK,
        ) && let Some(target_connection) = state.connections.entity_connection(target)
        {
            feedback(
                state,
                target_connection,
                "gameMode.changed",
                vec![mode_name(mode)],
            );
        }
    }
    changed
}

fn give(
    state: &mut TickState,
    connection: ConnectionId,
    targets: &[EntityId],
    item: &crate::command_items::ItemArgument,
    count: i32,
    command: &Command,
    changes: &mut ChangeSet,
) -> CommandResult {
    let stack = match item.stack_with(&state.registry.item_rules()) {
        Ok(stack) => stack,
        Err(reason) => {
            error(state, connection, reason);
            return Failed;
        }
    };
    let maximum = state
        .registry
        .item_rules()
        .max_stack(&stack)
        .saturating_mul(100);
    if count > maximum {
        let name = item_name(state, &item.item);
        feedback(
            state,
            connection,
            "commands.give.failed.toomanyitems",
            vec![text(maximum), name],
        );
        return Failed;
    }
    for &target in targets {
        apply_player_command(state, target, command, changes);
    }
    let key = format!("commands.give.success.{}", success_suffix(targets));
    let args = vec![
        text(count),
        item_name(state, &item.item),
        targets_label(state, targets),
    ];
    feedback(state, connection, &key, args);
    Success(targets.len() as i32)
}

fn clear(
    state: &mut TickState,
    connection: ConnectionId,
    targets: &[EntityId],
    item: Option<&crate::command_items::ItemPredicate>,
    max: Option<i32>,
    command: &Command,
    changes: &mut ChangeSet,
) -> CommandResult {
    let rules = state.registry.item_rules();
    if let Some(item) = item
        && let Err(reason) = item.validate(&rules)
    {
        error(state, connection, reason);
        return Failed;
    }
    // `max == Some(0)` only counts (`/clear ... 0`); a real clear must not
    // debit an inventory an active native operation still reserves.
    if max != Some(0) {
        for &target in targets {
            if !crate::plugin_inventories::has_reservations_for_player(state, target) {
                continue;
            }
            // The native clear kernel walks slots in order. Only its
            // ordinary 0..35 source debits affect addressable holds.
            let mut debits = [(0_usize, 0_u32); 36];
            let mut len = 0;
            let mut budget = max.unwrap_or(i32::MAX);
            for (slot, stack) in state.players[&target]
                .inventory
                .slots
                .iter()
                .take(36)
                .enumerate()
            {
                if stack.is_empty() || item.is_some_and(|item| !item.matches(&rules, stack)) {
                    continue;
                }
                let taken = stack.count.min(budget);
                debits[len] = (slot, taken as u32);
                len += 1;
                budget -= taken;
                if budget == 0 {
                    break;
                }
            }
            if crate::plugin_inventories::below_player_debit_floors(
                state,
                target,
                &debits[..len],
                None,
            ) {
                error(
                    state,
                    connection,
                    "Inventory is reserved by an active native operation",
                );
                return Failed;
            }
        }
    }
    let mut cleared = 0_i32;
    for &target in targets {
        let rules = state.registry.item_rules();
        let matching: i32 = state.players[&target]
            .inventory
            .slots
            .iter()
            .filter(|stack| {
                !stack.is_empty() && item.is_none_or(|item| item.matches(&rules, stack))
            })
            .map(|stack| stack.count)
            .sum();
        cleared =
            cleared.saturating_add(matching.min(max.filter(|max| *max > 0).unwrap_or(i32::MAX)));
        apply_player_command(state, target, command, changes);
    }
    let suffix = success_suffix(targets);
    let label = targets_label(state, targets);
    if cleared == 0 {
        feedback(
            state,
            connection,
            &format!("clear.failed.{suffix}"),
            vec![label],
        );
        Failed
    } else {
        let mode = if max == Some(0) { "test" } else { "success" };
        feedback(
            state,
            connection,
            &format!("commands.clear.{mode}.{suffix}"),
            vec![text(cleared), label],
        );
        Success(cleared)
    }
}

fn expand_message(state: &TickState, executor: EntityId, message: &str) -> String {
    let mut expanded = String::with_capacity(message.len());
    let mut offset = 0;
    while let Some(relative) = message[offset..].find('@') {
        let start = offset + relative;
        expanded.push_str(&message[offset..start]);
        let rest = &message[start..];
        if !matches!(
            rest.as_bytes().get(1),
            Some(b'p' | b'a' | b'r' | b's' | b'e')
        ) {
            expanded.push('@');
            offset = start + 1;
            continue;
        }
        let end = if rest.as_bytes().get(2) == Some(&b'[') {
            rest.find(']').map(|end| end + 1).unwrap_or(2)
        } else {
            2.min(rest.len())
        };
        let candidate = &rest[..end];
        if let Ok(target) = crate::commands::target_of(candidate) {
            let names = resolve_target(state, executor, &target)
                .into_iter()
                .filter_map(|id| state.connections.name_of(id))
                .collect::<Vec<_>>()
                .join(", ");
            expanded.push_str(&names);
        } else {
            expanded.push_str(candidate);
        }
        offset = start + end;
    }
    expanded.push_str(&message[offset..]);
    expanded
}

#[cfg(test)]
#[path = "command_execution_tests.rs"]
mod tests;
