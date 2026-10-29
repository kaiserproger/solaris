//! Native command sources and bounded /execute continuations. Cross-world dispatch
//! is admitted only by dimensions_travel at the shared tick boundary.
use crate::boundary::{ChangeSet, ConnectionId, EntityId, PlayerChange};
use crate::commands::Command;
use crate::dimensions::Dimension;
use crate::tick::TickState;
use std::collections::BTreeMap;

pub(crate) const LIMIT: usize = 65_536;
/// Local simulation increments the published tick before shared command commit.
pub(crate) fn execution_tick(state: &TickState) -> u64 {
    state
        .tick
        .saturating_sub(u64::from(state.command_context.shared))
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Context {
    pub connection: ConnectionId,
    pub executor: EntityId,
    pub permission: u8,
    pub position: [f64; 3],
    pub rotation: [f32; 2],
    pub dimension: Dimension,
    pub eyes: bool,
    pub eye_height: f64,
}
impl Context {
    pub(crate) fn source(
        state: &TickState,
        connection: ConnectionId,
        executor: EntityId,
        permission: u8,
    ) -> Self {
        if let Some(context) = state.command_context.current {
            return context;
        }
        let (position, rotation) = crate::command_console::pose(state, executor)
            .unwrap_or((state.world.world_spawn_position(), [0.0; 2]));
        Self {
            connection,
            executor,
            permission,
            position,
            rotation,
            dimension: state.dimension,
            eyes: false,
            eye_height: eye_height(state, executor),
        }
    }
    pub(crate) fn coordinate_origin(self, local: bool) -> [f64; 3] {
        let mut origin = self.position;
        if local && self.eyes {
            origin[1] += self.eye_height;
        }
        origin
    }
}
pub(crate) fn resolve_coords(
    state: &TickState,
    coords: [crate::commands::Coord; 3],
    origin: [f64; 3],
    rotation: [f32; 2],
) -> [f64; 3] {
    let origin = state
        .command_context
        .current
        .map_or(origin, |context| context.coordinate_origin(coords[0].local));
    crate::commands::resolve_coords(coords, origin, rotation)
}
pub(crate) struct Job {
    pub text: String,
    pub context: Context,
}
#[derive(Default)]
pub(crate) struct Runtime {
    pub current: Option<Context>,
    pub shared: bool,
    pub jobs: Vec<Job>,
    pub changes: BTreeMap<Dimension, ChangeSet>,
    used: usize,
    limit: usize,
    forks: usize,
    depth: usize,
    /// Per-selection random draws this tick; each `@r`/`sort=random`
    /// evaluation advances it so repeated same-tick evaluations draw again.
    pub(crate) random_draws: std::cell::Cell<u64>,
}
fn eye_height(state: &TickState, id: EntityId) -> f64 {
    state.entities.slot_of_any(id).map_or(0.0, |slot| {
        if state.players.contains_key(&id) {
            crate::tick::player_eye_height(state.entities.row(slot).appearance.pose)
        } else {
            let body = state.entities.body(slot);
            let type_id = match body.kind {
                crate::entities::EntityKind::Mob { type_id }
                | crate::entities::EntityKind::Item { type_id }
                | crate::entities::EntityKind::ExperienceOrb { type_id } => type_id,
                crate::entities::EntityKind::Player { .. } => return 0.0,
            };
            mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
                .map(mc_data::entity_types::canonical_dimensions)
                .filter(|dimensions| dimensions.height > 0.0)
                .map_or(0.0, |dimensions| {
                    dimensions.eye_height.unwrap_or(0.0) * body.height / dimensions.height
                })
        }
    })
}
fn error(state: &mut TickState, context: Context, message: impl Into<String>) {
    state.push_player(
        context.connection,
        PlayerChange::Chat {
            message: message.into(),
        },
    );
}
pub(crate) fn submit(state: &mut TickState, text: &str, context: Context, changes: &mut ChangeSet) {
    if state.dimension_globals.active && !state.command_context.shared {
        if state.command_context.jobs.len() < LIMIT {
            state.command_context.jobs.push(Job {
                text: text.to_owned(),
                context,
            });
        } else {
            error(state, context, "Command queue budget exceeded");
        }
    } else {
        run(state, text, context, changes);
    }
}
/// Root quota follows pinned 26.1.2 `Commands.executeCommandInContext`,
/// which clamps the configured sequence length to at least one so a zero
/// gamerule cannot lock out the restoring gamerule command itself.
pub(crate) fn begin_sequence(state: &mut TickState) {
    if state.command_context.depth == 0 {
        state.command_context.used = 0;
        state.command_context.limit = (crate::command_rules::integer(
            state,
            crate::command_rules::IntRule::MAX_COMMAND_SEQUENCE_LENGTH,
        ) as usize)
            .max(1);
        state.command_context.forks =
            crate::command_rules::integer(state, crate::command_rules::IntRule::MAX_COMMAND_FORKS)
                as usize;
    }
}
pub(crate) fn exhausted(state: &TickState) -> bool {
    state.command_context.used >= state.command_context.limit
}
pub(crate) fn charge(state: &mut TickState) -> Result<(), String> {
    if exhausted(state) {
        return Err(format!(
            "Command chain budget exceeded ({})",
            state.command_context.limit
        ));
    }
    state.command_context.used += 1;
    Ok(())
}
fn word<'a>(text: &mut &'a str) -> Result<&'a str, String> {
    *text = text.trim_start();
    if text.is_empty() {
        return Err("Incomplete execute command".into());
    }
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    let result = &text[..end];
    *text = &text[end..];
    Ok(result)
}
fn coordinates(text: &mut &str, context: Context) -> Result<[f64; 3], String> {
    let words = [word(text)?, word(text)?, word(text)?];
    let coords = crate::commands::coords(&words).ok_or("Invalid execute coordinates")?;
    let values = crate::commands::resolve_coords(
        coords,
        context.coordinate_origin(coords[0].local),
        context.rotation,
    );
    if values
        .iter()
        .any(|value| !value.is_finite() || value.abs() > 30_000_000.0)
    {
        return Err("Execute position outside world bounds".into());
    }
    Ok(values)
}
fn entity_pose(state: &TickState, id: EntityId) -> Option<(Dimension, [f64; 3], [f32; 2], f64)> {
    let (dimension, slot) = crate::dimensions::travel::locate(state, id)?;
    let world = crate::dimensions::travel::world(state, dimension)?;
    let [yaw, pitch, _] = world.entities.rot(slot);
    Some((
        dimension,
        world.entities.pos(slot),
        [yaw, pitch],
        eye_height(world, id),
    ))
}
fn select(state: &TickState, context: Context, token: &str) -> Result<Vec<EntityId>, String> {
    let target = crate::commands::target_of(token).map_err(|error| error.message())?;
    crate::dimensions::travel::context_targets(state, context, &target)
}
fn score(
    state: &TickState,
    context: Context,
    holder: &str,
    objective: &str,
) -> Result<Option<i32>, String> {
    if !state.scoreboard.objectives.contains_key(objective) {
        return Err("Unknown score objective".into());
    }
    let name;
    let holder = if holder.starts_with('@') {
        let ids = select(state, context, holder)?;
        if ids.len() != 1 {
            return Err("Score condition requires one holder".into());
        }
        let (dimension, slot) =
            crate::dimensions::travel::locate(state, ids[0]).ok_or("Missing score holder")?;
        let world =
            crate::dimensions::travel::world(state, dimension).ok_or("Missing holder world")?;
        name = world
            .connections
            .name_of(ids[0])
            .map(str::to_owned)
            .unwrap_or_else(|| world.entities.row(slot).uuid.to_string());
        name.as_str()
    } else {
        holder
    };
    Ok(state
        .scoreboard
        .scores
        .get(holder)
        .and_then(|scores| scores.get(objective))
        .map(|score| score.value))
}
fn condition(state: &TickState, context: Context, text: &mut &str) -> Result<i32, String> {
    match word(text)? {
        "entity" => Ok(select(state, context, word(text)?)?.len() as i32),
        "block" => {
            let at = coordinates(text, context)?.map(f64::floor);
            let spec =
                crate::command_world::block_spec(word(text)?).map_err(|error| error.message())?;
            let world = crate::dimensions::travel::world(state, context.dimension)
                .ok_or("Unknown dimension")?;
            spec.validate(&world.registry.blocks)
                .map_err(|error| error.message())?;
            let block = world
                .world
                .block_at(at[0] as i32, at[1] as i32, at[2] as i32)
                .ok_or("Condition position is not loaded")?;
            Ok(i32::from(spec.matches(&world.registry.blocks, block)))
        }
        "score" => {
            let holder = word(text)?;
            let objective = word(text)?;
            let operator = word(text)?;
            let left = score(state, context, holder, objective)?;
            if operator == "matches" {
                let range = word(text)?;
                let parse = |value: &str| {
                    value
                        .parse::<i32>()
                        .map_err(|_| "Invalid score range".to_owned())
                };
                let (min, max) = if let Some((min, max)) = range.split_once("..") {
                    if min.is_empty() && max.is_empty() {
                        return Err("Empty score range".into());
                    }
                    (
                        if min.is_empty() {
                            i32::MIN
                        } else {
                            parse(min)?
                        },
                        if max.is_empty() {
                            i32::MAX
                        } else {
                            parse(max)?
                        },
                    )
                } else {
                    let value = parse(range)?;
                    (value, value)
                };
                if min > max {
                    return Err("Reversed score range".into());
                }
                Ok(i32::from(
                    left.is_some_and(|value| (min..=max).contains(&value)),
                ))
            } else {
                let right_holder = word(text)?;
                let right_objective = word(text)?;
                if !matches!(operator, "=" | "<" | "<=" | ">" | ">=") {
                    return Err("Invalid score comparison".into());
                }
                let right = score(state, context, right_holder, right_objective)?;
                Ok(i32::from(match (left, right) {
                    (Some(a), Some(b)) => match operator {
                        "=" => a == b,
                        "<" => a < b,
                        "<=" => a <= b,
                        ">" => a > b,
                        _ => a >= b,
                    },
                    _ => false,
                }))
            }
        }
        "blocks" => {
            let a = coordinates(text, context)?.map(|v| v.floor() as i32);
            let b = coordinates(text, context)?.map(|v| v.floor() as i32);
            let dest = coordinates(text, context)?.map(|v| v.floor() as i32);
            let masked = match word(text)? {
                "all" => false,
                "masked" => true,
                _ => return Err("Expected all or masked".into()),
            };
            let min = std::array::from_fn::<_, 3, _>(|i| a[i].min(b[i]));
            let size = std::array::from_fn::<_, 3, _>(|i| {
                i64::from(a[i].max(b[i])) - i64::from(min[i]) + 1
            });
            let volume = size[0]
                .checked_mul(size[1])
                .and_then(|v| v.checked_mul(size[2]))
                .ok_or("Too many blocks")?;
            let world = crate::dimensions::travel::world(state, context.dimension)
                .ok_or("Unknown dimension")?;
            let limit = i64::from(crate::command_rules::integer(
                world,
                crate::command_rules::IntRule::MAX_BLOCK_MODIFICATIONS,
            ));
            if volume > limit {
                return Err("Too many blocks".into());
            }
            let mut count = 0;
            for x in 0..size[0] {
                for y in 0..size[1] {
                    for z in 0..size[2] {
                        let source = [
                            i64::from(min[0]) + x,
                            i64::from(min[1]) + y,
                            i64::from(min[2]) + z,
                        ];
                        let target = [
                            i64::from(dest[0]) + x,
                            i64::from(dest[1]) + y,
                            i64::from(dest[2]) + z,
                        ];
                        let pos = |p: [i64; 3]| -> Result<crate::boundary::BlockPos, String> {
                            Ok(crate::boundary::BlockPos {
                                x: i32::try_from(p[0]).map_err(|_| "Position outside world")?,
                                y: i32::try_from(p[1]).map_err(|_| "Position outside world")?,
                                z: i32::try_from(p[2]).map_err(|_| "Position outside world")?,
                            })
                        };
                        let source = pos(source)?;
                        let target = pos(target)?;
                        let s = world
                            .world
                            .block_at(source.x, source.y, source.z)
                            .ok_or("Condition position is not loaded")?;
                        let d = world
                            .world
                            .block_at(target.x, target.y, target.z)
                            .ok_or("Condition position is not loaded")?;
                        if masked && crate::command_world::is_air(&world.registry.blocks, s) {
                            continue;
                        }
                        if s != d {
                            return Ok(0);
                        }
                        if mc_data::block_entity_kinds::BlockEntityKind::for_block(
                            world
                                .registry
                                .blocks
                                .by_id(mc_world::BlockStateId(s))
                                .ok_or("Unknown block state")?
                                .block
                                .id
                                .path(),
                        )
                        .is_some()
                        {
                            let normalize = |pos| -> Result<mc_nbt::Tag, String> {
                                let mut tag = crate::command_world::block_image(world, pos)?;
                                if let mc_nbt::Tag::Compound(fields) = &mut tag {
                                    fields.retain(|(name, _)| {
                                        !matches!(name.as_str(), "x" | "y" | "z")
                                    });
                                }
                                Ok(tag)
                            };
                            if !crate::command_world::nbt_equal(
                                &normalize(source)?,
                                &normalize(target)?,
                            ) {
                                return Ok(0);
                            }
                        }
                        count += 1;
                    }
                }
            }
            Ok(count)
        }
        _ => Err("Unsupported execute condition".into()),
    }
}
fn face(context: &mut Context, target: [f64; 3]) {
    let origin = context.coordinate_origin(true);
    let [x, y, z] = std::array::from_fn::<_, 3, _>(|axis| target[axis] - origin[axis]);
    context.rotation = [
        ((mc_physics::mth::atan2(z, x).to_degrees() - 90.0 + 180.0).rem_euclid(360.0) - 180.0)
            as f32,
        (-mc_physics::mth::atan2(y, (x * x + z * z).sqrt()).to_degrees()) as f32,
    ];
}
/// Iterative branches prevent a long `run execute ...` chain from using the Rust stack.
pub(crate) fn run(state: &mut TickState, text: &str, context: Context, changes: &mut ChangeSet) {
    if state.command_context.depth >= 64 {
        error(
            state,
            context,
            "Nested command/function depth exceeded (64)",
        );
        return;
    }
    if !state.datapacks.running {
        begin_sequence(state);
    }
    state.command_context.depth += 1;
    let previous = state.command_context.current;
    let mut branches = std::collections::VecDeque::from([(
        text.trim(),
        context,
        false,
        Vec::<crate::command_store::Store>::new(),
    )]);
    let mut stage = None;
    let mut produced = 0usize;
    while let Some((mut text, mut context, mut continuation, mut stores)) = branches.pop_front() {
        let mut resume = false;
        let result = (|| -> Result<Option<(bool, i32)>, String> {
            let first_in_stage = stage != Some(text.as_ptr());
            if first_in_stage {
                stage = Some(text.as_ptr());
                produced = 0;
            }
            state.command_context.current = Some(context);
            let mut rest = text;
            let root = word(&mut rest)?;
            if !continuation && root != "execute" {
                if root != "function" {
                    charge(state)?;
                }
                use crate::command_execution::CommandResult;
                return match native(state, text, context, changes, !stores.is_empty())? {
                    CommandResult::Success(value) => Ok(Some((true, value))),
                    CommandResult::Failed => Ok(Some((false, 0))),
                    CommandResult::Unavailable if stores.is_empty() => Ok(None),
                    CommandResult::Unavailable => Err("Command has no execute-store result".into()),
                };
            }
            if context.permission < 2 {
                return Err(crate::commands::CommandError::Unknown.message());
            }
            if !continuation {
                text = rest;
            }
            continuation = true;
            let subcommand = word(&mut text)?;
            if subcommand != "run" && first_in_stage {
                state.command_context.used = state.command_context.used.saturating_add(1);
            }
            match subcommand {
                "run" => {
                    continuation = false;
                }
                "as" | "at" => {
                    let selected = select(state, context, word(&mut text)?)?;
                    produced += selected.len();
                    if produced >= state.command_context.forks {
                        branches.clear();
                        return Err("Execute fork budget exceeded".into());
                    }
                    if selected.is_empty() {
                        return Ok(Some((false, 0)));
                    }
                    for id in selected {
                        let Some((dimension, position, rotation, height)) = entity_pose(state, id)
                        else {
                            continue;
                        };
                        let mut next = context;
                        if subcommand == "as" {
                            next.executor = id;
                            next.eye_height = height;
                        } else {
                            next.dimension = dimension;
                            next.position = position;
                            next.rotation = rotation;
                        }
                        branches.push_back((text, next, true, stores.clone()));
                    }
                    return Ok(None);
                }
                "positioned" | "rotated" => {
                    let mut probe = text;
                    if word(&mut probe)? == "as" {
                        text = probe;
                        let selected = select(state, context, word(&mut text)?)?;
                        produced += selected.len();
                        if produced >= state.command_context.forks {
                            branches.clear();
                            return Err("Execute fork budget exceeded".into());
                        }
                        if selected.is_empty() {
                            return Ok(Some((false, 0)));
                        }
                        for id in selected {
                            let Some((_, position, rotation, _)) = entity_pose(state, id) else {
                                continue;
                            };
                            let mut next = context;
                            if subcommand == "positioned" {
                                next.position = position;
                                next.eyes = false;
                            } else {
                                next.rotation = rotation;
                            }
                            branches.push_back((text, next, true, stores.clone()));
                        }
                        return Ok(None);
                    }
                    if subcommand == "positioned" {
                        context.position = coordinates(&mut text, context)?;
                        context.eyes = false;
                    } else {
                        let yaw = crate::commands::coord(word(&mut text)?)
                            .filter(|value| !value.local)
                            .ok_or("Invalid yaw")?;
                        let pitch = crate::commands::coord(word(&mut text)?)
                            .filter(|value| !value.local)
                            .ok_or("Invalid pitch")?;
                        context.rotation = [
                            yaw.resolve(f64::from(context.rotation[0])) as f32,
                            pitch.resolve(f64::from(context.rotation[1])) as f32,
                        ];
                    }
                }
                "facing" => {
                    let mut probe = text;
                    if word(&mut probe)? == "entity" {
                        text = probe;
                        let selected = select(state, context, word(&mut text)?)?;
                        let anchor = word(&mut text)?;
                        if !matches!(anchor, "eyes" | "feet") {
                            return Err("Invalid entity anchor".into());
                        }
                        produced += selected.len();
                        if produced >= state.command_context.forks {
                            branches.clear();
                            return Err("Execute fork budget exceeded".into());
                        }
                        if selected.is_empty() {
                            return Ok(Some((false, 0)));
                        }
                        for id in selected {
                            let Some((_, mut target, _, height)) = entity_pose(state, id) else {
                                continue;
                            };
                            if anchor == "eyes" {
                                target[1] += height;
                            }
                            let mut next = context;
                            face(&mut next, target);
                            branches.push_back((text, next, true, stores.clone()));
                        }
                        return Ok(None);
                    }
                    let target = coordinates(&mut text, context)?;
                    face(&mut context, target);
                }
                "anchored" => {
                    context.eyes = match word(&mut text)? {
                        "eyes" => true,
                        "feet" => false,
                        _ => return Err("Invalid entity anchor".into()),
                    };
                }
                "align" => {
                    let axes = word(&mut text)?;
                    let mut seen = [false; 3];
                    for axis in axes.bytes() {
                        let index = match axis {
                            b'x' => 0,
                            b'y' => 1,
                            b'z' => 2,
                            _ => return Err("Invalid alignment axes".into()),
                        };
                        if seen[index] {
                            return Err("Duplicate alignment axis".into());
                        }
                        seen[index] = true;
                        context.position[index] = context.position[index].floor();
                    }
                }
                "in" => {
                    let dimension =
                        Dimension::parse(word(&mut text)?).ok_or("Unknown execute dimension")?;
                    if crate::dimensions::travel::world(state, dimension).is_none() {
                        return Err("Execute dimension unavailable".into());
                    }
                    let scale = context.dimension.coordinate_scale() / dimension.coordinate_scale();
                    context.position[0] *= scale;
                    context.position[2] *= scale;
                    context.dimension = dimension;
                }
                "if" | "unless" => {
                    let numeric = text.split_whitespace().next() == Some("entity");
                    let count = condition(state, context, &mut text)?;
                    let passed = (count != 0) == (subcommand == "if");
                    if !passed {
                        if text.trim().is_empty() {
                            return Err(if numeric && subcommand == "unless" {
                                format!("Test failed. Count: {count}")
                            } else {
                                "Test failed".into()
                            });
                        }
                        return Ok(Some((false, 0)));
                    }
                    if text.trim().is_empty() {
                        let value = if subcommand == "if" { count } else { 1 };
                        crate::command_execution::feedback(
                            state,
                            context.connection,
                            if numeric && subcommand == "if" {
                                "commands.execute.conditional.pass_count"
                            } else {
                                "commands.execute.conditional.pass"
                            },
                            if numeric && subcommand == "if" {
                                vec![mc_nbt::Tag::String(value.to_string())]
                            } else {
                                Vec::new()
                            },
                        );
                        return Ok(Some((true, value)));
                    }
                }
                "store" => stores.push(crate::command_store::Store::parse(
                    state, context, &mut text,
                )?),
                _ => return Err("Unsupported execute subcommand".into()),
            }
            resume = true;
            Ok(None)
        })();
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(message) => {
                error(state, context, message);
                Some((false, 0))
            }
        };
        if let Some((success, value)) = outcome {
            for store in &stores {
                store.apply(state, success, value);
            }
        }
        if resume {
            branches.push_back((text, context, continuation, stores));
        }
    }
    state.command_context.current = previous;
    state.command_context.depth -= 1;
}
pub(crate) fn native(
    state: &mut TickState,
    text: &str,
    context: Context,
    changes: &mut ChangeSet,
    require_result: bool,
) -> Result<crate::command_execution::CommandResult, String> {
    if context.dimension != state.dimension {
        return crate::dimensions::travel::execute_context(state, text, context, require_result);
    }
    state.command_context.current = Some(context);
    if matches!(
        text.split_whitespace().next(),
        Some("function" | "schedule" | "reload" | "datapack" | "data")
    ) {
        if context.permission < 2 {
            return Err(crate::commands::CommandError::Unknown.message());
        }
        if require_result && !matches!(text.split_whitespace().next(), Some("data" | "datapack")) {
            return Err("Command does not support execute store".into());
        }
        return Ok(crate::datapacks::command(
            state,
            context.connection,
            context.executor,
            text,
            changes,
        ));
    }
    let command =
        crate::commands::parse(text, context.permission >= 2).map_err(|error| error.message())?;
    if require_result && !crate::command_execution::supports_store_result(&command) {
        return Err("Command does not support execute store".into());
    }
    if state.command_context.shared
        && (matches!(command, Command::Teleport { .. })
            || crate::dimensions::commands::target(&command).is_some())
    {
        let mut aggregate = std::mem::take(&mut state.command_context.changes);
        let result = crate::dimensions::travel::execute(
            state,
            crate::dimensions::travel::Request {
                connection: context.connection,
                executor: context.executor,
                dimension: context.dimension,
                command,
            },
            &mut aggregate,
        );
        state.command_context.changes = aggregate;
        result
    } else {
        let result = crate::command_execution::execute(
            state,
            context.connection,
            context.executor,
            &command,
            changes,
        );
        if state.command_context.shared {
            crate::dimensions::commands::commit(state, changes);
        }
        Ok(result)
    }
}
#[cfg(test)]
#[path = "command_context_tests.rs"]
mod tests;
