//! Typed world-edit and respawn commands. Accepted edits are visible to later
//! inputs and entity collision immediately; the canonical block commit path
//! still owns their journal and container transactions.

use crate::boundary::{BlockPos, ChangeSet, ConnectionId, EntityId, PlayerChange};
use crate::commands::{CommandError, Coord, Target, coord, coords, target_of};
use crate::tick::TickState;
use mc_data::Identifier;
use mc_world::{BlockRegistry, BlockStateId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockSpec {
    pub name: Identifier,
    pub properties: Vec<(String, String)>,
    pub nbt: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetMode {
    Replace,
    Keep,
    Destroy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FillMode {
    Replace(Option<BlockSpec>),
    Destroy,
    Keep,
    Hollow,
    Outline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneFilter {
    Replace,
    Masked,
    Filtered { spec: BlockSpec, tag: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneMode {
    Normal,
    Force,
    Move,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum WorldCommand {
    SetBlock {
        position: [Coord; 3],
        block: BlockSpec,
        mode: SetMode,
    },
    Fill {
        from: [Coord; 3],
        to: [Coord; 3],
        block: BlockSpec,
        mode: FillMode,
    },
    Clone {
        from: [Coord; 3],
        to: [Coord; 3],
        destination: [Coord; 3],
        filter: CloneFilter,
        mode: CloneMode,
    },
    SpawnPoint {
        target: Target,
        position: Option<[Coord; 3]>,
        rotation: Option<[Coord; 2]>,
    },
    SetWorldSpawn {
        position: Option<[Coord; 3]>,
        rotation: Option<[Coord; 2]>,
    },
}

fn message(text: impl Into<String>) -> CommandError {
    CommandError::Message(text.into())
}

pub(crate) fn block_spec(word: &str) -> Result<BlockSpec, CommandError> {
    let (word, nbt) = if let Some(start) = word.find('{') {
        let tag = crate::command_snbt::parse(&word[start..]).map_err(message)?;
        if !matches!(tag, mc_nbt::Tag::Compound(_)) {
            return Err(message("Expected block entity compound"));
        }
        let mut bytes = Vec::new();
        mc_nbt::write_network(&mut bytes, &tag).map_err(|error| message(error.to_string()))?;
        (&word[..start], Some(bytes))
    } else {
        (word, None)
    };
    let (name, properties) = match word.split_once('[') {
        Some((name, rest)) => {
            let rest = rest
                .strip_suffix(']')
                .ok_or_else(|| message("Invalid block properties"))?;
            let mut properties = Vec::new();
            if !rest.is_empty() {
                for property in rest.split(',') {
                    let (key, value) = property
                        .split_once('=')
                        .ok_or_else(|| message("Invalid block property"))?;
                    if key.is_empty()
                        || value.is_empty()
                        || !key
                            .bytes()
                            .chain(value.bytes())
                            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
                        || properties.iter().any(|(other, _)| other == key)
                    {
                        return Err(message("Invalid or duplicate block property"));
                    }
                    properties.push((key.to_owned(), value.to_owned()));
                }
            }
            (name, properties)
        }
        None => (word, Vec::new()),
    };
    let name = if name.contains(':') {
        name.to_owned()
    } else {
        format!("minecraft:{name}")
    };
    Ok(BlockSpec {
        name: Identifier::parse(name).map_err(|_| message("Invalid block identifier"))?,
        properties,
        nbt,
    })
}

impl BlockSpec {
    pub(crate) fn validate<'a>(
        &self,
        registry: &'a BlockRegistry,
    ) -> Result<&'a mc_world::block::Block, CommandError> {
        let block = registry
            .block(&self.name)
            .ok_or_else(|| message(format!("Unknown block {}", self.name)))?;
        for (key, value) in &self.properties {
            if !block
                .properties
                .iter()
                .any(|(name, values)| name == key && values.contains(value))
            {
                return Err(message(format!(
                    "Invalid property {key}={value} for {}",
                    self.name
                )));
            }
        }
        Ok(block)
    }

    fn resolve(&self, registry: &BlockRegistry) -> Result<u32, CommandError> {
        let block = self.validate(registry)?;
        let mut properties = registry
            .by_id(block.default)
            .expect("registry default state")
            .properties
            .clone();
        for (key, value) in &self.properties {
            properties
                .iter_mut()
                .find(|(name, _)| name == key)
                .expect("validated property")
                .1
                .clone_from(value);
        }
        registry
            .by_name_and_props(&self.name, &properties)
            .map(|state| state.0)
            .ok_or_else(|| message("Invalid block state"))
    }

    pub(crate) fn matches(&self, registry: &BlockRegistry, state: u32) -> bool {
        registry.by_id(BlockStateId(state)).is_some_and(|state| {
            state.block.id == self.name
                && self
                    .properties
                    .iter()
                    .all(|property| state.properties.contains(property))
        })
    }
}

fn rotation(words: &[&str]) -> Option<[Coord; 2]> {
    let rotation = match words {
        [yaw] => [
            coord(yaw)?,
            Coord {
                relative: false,
                local: false,
                value: 0.0,
            },
        ],
        [_, _] => return crate::commands::rotation(words),
        _ => return None,
    };
    rotation
        .iter()
        .all(|coordinate| !coordinate.local)
        .then_some(rotation)
}

pub fn parse(root: &str, args: &[&str]) -> Result<WorldCommand, CommandError> {
    match root {
        "setblock" => {
            let usage = CommandError::Usage("setblock <pos> <block> [destroy|keep|replace]");
            if !(4..=5).contains(&args.len()) {
                return Err(usage);
            }
            let position = coords(&args[..3]).ok_or(usage.clone())?;
            let block = block_spec(args[3])?;
            let mode = match args.get(4).copied().unwrap_or("replace") {
                "replace" => SetMode::Replace,
                "keep" => SetMode::Keep,
                "destroy" => SetMode::Destroy,
                _ => return Err(usage),
            };
            Ok(WorldCommand::SetBlock {
                position,
                block,
                mode,
            })
        }
        "fill" => {
            let usage = CommandError::Usage(
                "fill <from> <to> <block> [replace [filter]|destroy|keep|hollow|outline]",
            );
            if !(7..=9).contains(&args.len()) {
                return Err(usage);
            }
            let from = coords(&args[..3]).ok_or(usage.clone())?;
            let to = coords(&args[3..6]).ok_or(usage.clone())?;
            let block = block_spec(args[6])?;
            let mode = match &args[7..] {
                [] | ["replace"] => FillMode::Replace(None),
                ["replace", filter] => FillMode::Replace(Some(block_spec(filter)?)),
                ["destroy"] => FillMode::Destroy,
                ["keep"] => FillMode::Keep,
                ["hollow"] => FillMode::Hollow,
                ["outline"] => FillMode::Outline,
                _ => return Err(usage),
            };
            Ok(WorldCommand::Fill {
                from,
                to,
                block,
                mode,
            })
        }
        "clone" => {
            let usage = CommandError::Usage(
                "clone <begin> <end> <destination> [replace|masked|filtered <filter>] [force|move|normal]",
            );
            if !(9..=12).contains(&args.len()) {
                return Err(usage);
            }
            let from = coords(&args[..3]).ok_or(usage.clone())?;
            let to = coords(&args[3..6]).ok_or(usage.clone())?;
            let destination = coords(&args[6..9]).ok_or(usage.clone())?;
            let (filter, rest) = match &args[9..] {
                [] => (CloneFilter::Replace, &args[9..]),
                ["replace", rest @ ..] => (CloneFilter::Replace, rest),
                ["masked", rest @ ..] => (CloneFilter::Masked, rest),
                ["filtered", filter, rest @ ..] => {
                    let tag = filter.starts_with('#');
                    (
                        CloneFilter::Filtered {
                            spec: block_spec(filter.strip_prefix('#').unwrap_or(filter))?,
                            tag,
                        },
                        rest,
                    )
                }
                _ => return Err(usage),
            };
            let mode = match rest {
                [] | ["normal"] => CloneMode::Normal,
                ["force"] => CloneMode::Force,
                ["move"] => CloneMode::Move,
                _ => return Err(usage),
            };
            Ok(WorldCommand::Clone {
                from,
                to,
                destination,
                filter,
                mode,
            })
        }
        "spawnpoint" => {
            let usage = CommandError::Usage("spawnpoint [targets] [pos] [yaw pitch]");
            if args.is_empty() {
                return Ok(WorldCommand::SpawnPoint {
                    target: Target::Executor,
                    position: None,
                    rotation: None,
                });
            }
            let (target, remaining) = if coords(args.get(..3).unwrap_or(&[])).is_some() {
                (Target::Executor, args)
            } else {
                (target_of(args[0])?, &args[1..])
            };
            let SpawnArguments { position, rotation } = spawn_args(remaining, usage)?;
            Ok(WorldCommand::SpawnPoint {
                target,
                position,
                rotation,
            })
        }
        "setworldspawn" => {
            let SpawnArguments { position, rotation } =
                spawn_args(args, CommandError::Usage("setworldspawn [pos] [yaw pitch]"))?;
            Ok(WorldCommand::SetWorldSpawn { position, rotation })
        }
        _ => Err(CommandError::Unknown),
    }
}

struct SpawnArguments {
    position: Option<[Coord; 3]>,
    rotation: Option<[Coord; 2]>,
}

fn spawn_args(args: &[&str], usage: CommandError) -> Result<SpawnArguments, CommandError> {
    if args.is_empty() {
        return Ok(SpawnArguments {
            position: None,
            rotation: None,
        });
    }
    if !(3..=5).contains(&args.len()) {
        return Err(usage);
    }
    let position = coords(&args[..3]).ok_or(usage.clone())?;
    let rotation = if args.len() == 3 {
        None
    } else {
        Some(rotation(&args[3..]).ok_or(usage)?)
    };
    Ok(SpawnArguments {
        position: Some(position),
        rotation,
    })
}

fn position(
    state: &TickState,
    coords: [Coord; 3],
    base: [f64; 3],
    rotation: [f32; 2],
    geometry: mc_world::ChunkGeometry,
) -> Result<BlockPos, CommandError> {
    let values =
        crate::command_context::resolve_coords(state, coords, base, rotation).map(f64::floor);
    if values
        .iter()
        .any(|value| !value.is_finite() || value.abs() > 30_000_000.0)
        || !(f64::from(geometry.min_y())..f64::from(geometry.max_y())).contains(&values[1])
    {
        return Err(message("Position is outside the world"));
    }
    Ok(BlockPos {
        x: values[0] as i32,
        y: values[1] as i32,
        z: values[2] as i32,
    })
}

fn resolved_rotation(rotation: Option<[Coord; 2]>, base: [f32; 2]) -> [f32; 2] {
    rotation.map_or([0.0, 0.0], |rotation| {
        let yaw = rotation[0].resolve(f64::from(base[0]));
        let yaw = (yaw + 180.0).rem_euclid(360.0) - 180.0;
        [
            yaw as f32,
            rotation[1].resolve(f64::from(base[1])).clamp(-90.0, 90.0) as f32,
        ]
    })
}

pub(crate) fn spawnpoint_location(
    state: &TickState,
    executor: EntityId,
    coordinates: Option<[Coord; 3]>,
    rotation: Option<[Coord; 2]>,
) -> Result<([i32; 3], [f32; 2]), CommandError> {
    let (base, [yaw, pitch]) = crate::command_console::pose(state, executor)
        .ok_or_else(|| message("Command source is unavailable"))?;
    let pos = position(
        state,
        coordinates.unwrap_or(
            [Coord {
                relative: true,
                local: false,
                value: 0.0,
            }; 3],
        ),
        base,
        [yaw, pitch],
        state.dimension.geometry(),
    )?;
    Ok((
        [pos.x, pos.y, pos.z],
        resolved_rotation(rotation, [yaw, pitch]),
    ))
}

pub(crate) fn execute(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    command: &WorldCommand,
    _changes: &mut ChangeSet,
) -> crate::command_execution::CommandResult {
    use crate::command_execution::CommandResult::Failed;
    let (text, result) = match execute_inner(state, executor, command) {
        Ok(reply) => reply,
        Err(error) => (
            match error {
                CommandError::Message(text) => text,
                CommandError::Usage(usage) => format!("Usage: {usage}"),
                CommandError::Unknown => "Unknown command".to_owned(),
            },
            Failed,
        ),
    };
    state.push_player(connection, PlayerChange::Chat { message: text });
    result
}

fn execute_inner(
    state: &mut TickState,
    executor: EntityId,
    command: &WorldCommand,
) -> Result<(String, crate::command_execution::CommandResult), CommandError> {
    let (base, base_rotation) = crate::command_console::pose(state, executor)
        .ok_or_else(|| message("Command source is unavailable"))?;
    match command {
        WorldCommand::SetBlock {
            position: pos,
            block,
            mode,
        } => {
            let pos = position(state, *pos, base, base_rotation, state.dimension.geometry())?;
            let nbt = block.nbt.as_deref();
            let block = block.resolve(&state.registry.blocks)?;
            let image = crate::command_block_entity::prepare(state, block, nbt).map_err(message)?;
            edit_region(
                state,
                pos,
                pos,
                block,
                image,
                &match mode {
                    SetMode::Replace => FillMode::Replace(None),
                    SetMode::Keep => FillMode::Keep,
                    SetMode::Destroy => FillMode::Destroy,
                },
            )
        }
        WorldCommand::Fill {
            from,
            to,
            block,
            mode,
        } => {
            let from = position(
                state,
                *from,
                base,
                base_rotation,
                state.dimension.geometry(),
            )?;
            let to = position(state, *to, base, base_rotation, state.dimension.geometry())?;
            let nbt = block.nbt.as_deref();
            let block = block.resolve(&state.registry.blocks)?;
            let image = crate::command_block_entity::prepare(state, block, nbt).map_err(message)?;
            if let FillMode::Replace(Some(filter)) = mode {
                filter.validate(&state.registry.blocks)?;
                if filter.nbt.is_some() {
                    return Err(message(
                        "NBT predicates are not supported in fill replacement filters",
                    ));
                }
            }
            edit_region(state, from, to, block, image, mode)
        }
        WorldCommand::Clone {
            from,
            to,
            destination,
            filter,
            mode,
        } => {
            let geometry = state.dimension.geometry();
            let from = position(state, *from, base, base_rotation, geometry)?;
            let to = position(state, *to, base, base_rotation, geometry)?;
            let destination = position(state, *destination, base, base_rotation, geometry)?;
            clone_region(state, from, to, destination, filter, *mode)
        }
        WorldCommand::SpawnPoint {
            target,
            position: pos,
            rotation,
        } => {
            let (coordinates, rotation) = spawnpoint_location(state, executor, *pos, *rotation)?;
            let pos = BlockPos {
                x: coordinates[0],
                y: coordinates[1],
                z: coordinates[2],
            };
            let targets = crate::command_execution::resolve_target(state, executor, target);
            let mut count = 0;
            for target in targets {
                if let Some(player) = state.players.get_mut(&target) {
                    player.respawn_point = Some([pos.x, pos.y, pos.z]);
                    player.respawn_dimension = state.dimension;
                    player.respawn_forced = true;
                    player.respawn_yaw = rotation[0];
                    player.respawn_pitch = rotation[1];
                    count += 1;
                }
            }
            if count == 0 {
                return Err(message("No players matched"));
            }
            Ok((
                format!(
                    "Set spawn point for {count} player(s) to {}, {}, {}",
                    pos.x, pos.y, pos.z
                ),
                crate::command_execution::CommandResult::Completed,
            ))
        }
        WorldCommand::SetWorldSpawn {
            position: pos,
            rotation,
        } => {
            let pos = position(
                state,
                pos.unwrap_or(
                    [Coord {
                        relative: true,
                        local: false,
                        value: 0.0,
                    }; 3],
                ),
                base,
                base_rotation,
                state.dimension.geometry(),
            )?;
            state.world.set_world_spawn(
                [pos.x, pos.y, pos.z],
                resolved_rotation(*rotation, base_rotation),
            );
            Ok((
                format!("Set world spawn to {}, {}, {}", pos.x, pos.y, pos.z),
                crate::command_execution::CommandResult::Completed,
            ))
        }
    }
}

pub(crate) fn block_image(state: &TickState, pos: BlockPos) -> Result<mc_nbt::Tag, String> {
    if let Some((_, image)) = state
        .merge_command_block_entities
        .iter()
        .rev()
        .find(|(cell, _)| *cell == pos)
    {
        Ok(image.tag_at(pos))
    } else {
        crate::command_data::block(state, pos)
    }
}

pub(crate) fn nbt_equal(left: &mc_nbt::Tag, right: &mc_nbt::Tag) -> bool {
    match (left, right) {
        (mc_nbt::Tag::Compound(left), mc_nbt::Tag::Compound(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right
                        .iter()
                        .any(|(other, other_value)| key == other && nbt_equal(value, other_value))
                })
        }
        (mc_nbt::Tag::List(left), mc_nbt::Tag::List(right)) => {
            left.element_type == right.element_type
                && left.elements.len() == right.elements.len()
                && left
                    .elements
                    .iter()
                    .zip(&right.elements)
                    .all(|(a, b)| nbt_equal(a, b))
        }
        _ => left == right,
    }
}

fn nbt_subset(expected: &mc_nbt::Tag, actual: &mc_nbt::Tag) -> bool {
    match (expected, actual) {
        (mc_nbt::Tag::Compound(expected), mc_nbt::Tag::Compound(actual)) => {
            expected.iter().all(|(key, value)| {
                actual
                    .iter()
                    .any(|(other, actual)| key == other && nbt_subset(value, actual))
            })
        }
        (mc_nbt::Tag::List(expected), mc_nbt::Tag::List(actual)) => {
            if expected.elements.is_empty() {
                actual.elements.is_empty()
            } else {
                expected.elements.iter().all(|item| {
                    actual
                        .elements
                        .iter()
                        .any(|actual| nbt_subset(item, actual))
                })
            }
        }
        _ => expected == actual,
    }
}

fn clone_region(
    state: &mut TickState,
    from: BlockPos,
    to: BlockPos,
    destination: BlockPos,
    filter: &CloneFilter,
    mode: CloneMode,
) -> Result<(String, crate::command_execution::CommandResult), CommandError> {
    let min = [from.x.min(to.x), from.y.min(to.y), from.z.min(to.z)];
    let max = [from.x.max(to.x), from.y.max(to.y), from.z.max(to.z)];
    let dest = [destination.x, destination.y, destination.z];
    let size = std::array::from_fn::<_, 3, _>(|i| i64::from(max[i]) - i64::from(min[i]) + 1);
    let volume = size[0]
        .checked_mul(size[1])
        .and_then(|n| n.checked_mul(size[2]))
        .ok_or_else(|| message("Too many blocks"))?;
    let limit = i64::from(crate::command_rules::integer(
        state,
        crate::command_rules::IntRule::MAX_BLOCK_MODIFICATIONS,
    ));
    if volume > limit {
        return Err(message("Too many blocks"));
    }
    let end = std::array::from_fn::<_, 3, _>(|i| i64::from(dest[i]) + size[i] - 1);
    let geometry = state.dimension.geometry();
    if end[0] > 30_000_000 || end[2] > 30_000_000 || end[1] >= i64::from(geometry.max_y()) {
        return Err(message("Position is outside the world"));
    }
    if mode == CloneMode::Normal
        && (0..3).all(|i| i64::from(dest[i]) <= i64::from(max[i]) && end[i] >= i64::from(min[i]))
    {
        return Err(message("Source and destination overlap"));
    }
    if let CloneFilter::Filtered { spec, tag } = filter {
        if *tag {
            let registry = Identifier::parse("minecraft:block").expect("built-in registry");
            if state
                .registry
                .tags
                .registries
                .get(&registry)
                .is_none_or(|tags| !tags.contains_key(&spec.name))
            {
                return Err(message(format!("Unknown block tag {}", spec.name)));
            }
        } else {
            spec.validate(&state.registry.blocks)?;
        }
    }
    let mut blocks = Vec::new();
    for x in 0..size[0] {
        for y in 0..size[1] {
            for z in 0..size[2] {
                let offset = [x, y, z];
                let source = BlockPos {
                    x: (i64::from(min[0]) + x) as i32,
                    y: (i64::from(min[1]) + y) as i32,
                    z: (i64::from(min[2]) + z) as i32,
                };
                let target = BlockPos {
                    x: (i64::from(dest[0]) + offset[0]) as i32,
                    y: (i64::from(dest[1]) + offset[1]) as i32,
                    z: (i64::from(dest[2]) + offset[2]) as i32,
                };
                let block = state
                    .world
                    .block_at(source.x, source.y, source.z)
                    .ok_or_else(|| message("Position is not loaded"))?;
                state
                    .world
                    .block_at(target.x, target.y, target.z)
                    .ok_or_else(|| message("Position is not loaded"))?;
                if matches!(filter, CloneFilter::Masked) && is_air(&state.registry.blocks, block) {
                    continue;
                }
                if let CloneFilter::Filtered { spec, tag } = filter {
                    let block_state = state
                        .registry
                        .blocks
                        .by_id(BlockStateId(block))
                        .ok_or_else(|| message("Unknown block state"))?;
                    let matches_base = if *tag {
                        state.registry.tags.contains_raw_id(
                            "minecraft:block",
                            spec.name.as_str(),
                            block_state.block.raw_id,
                        ) && spec
                            .properties
                            .iter()
                            .all(|property| block_state.properties.contains(property))
                    } else {
                        spec.matches(&state.registry.blocks, block)
                    };
                    if !matches_base {
                        continue;
                    }
                    if let Some(nbt) = &spec.nbt {
                        let expected = mc_nbt::read_network(&mut nbt.as_slice())
                            .map_err(|e| message(e.to_string()))?;
                        let actual = match block_image(state, source) {
                            Ok(tag) => tag,
                            Err(_) => continue,
                        };
                        if !nbt_subset(&expected, &actual) {
                            continue;
                        }
                    }
                }
                let image = if let Some((_, image)) = state
                    .merge_command_block_entities
                    .iter()
                    .rev()
                    .find(|(pos, _)| *pos == source)
                {
                    Some(image.clone())
                } else if state
                    .registry
                    .blocks
                    .by_id(BlockStateId(block))
                    .and_then(|s| {
                        mc_data::block_entity_kinds::BlockEntityKind::for_block(s.block.id.path())
                    })
                    .is_some()
                {
                    let tag = crate::command_data::block(state, source).map_err(message)?;
                    let mut bytes = Vec::new();
                    mc_nbt::write_network(&mut bytes, &tag).map_err(|e| message(e.to_string()))?;
                    crate::command_block_entity::prepare(state, block, Some(&bytes))
                        .map_err(message)?
                } else {
                    None
                };
                blocks.push((source, target, block, image));
            }
        }
    }
    if blocks.is_empty() {
        return Err(message("No blocks were cloned"));
    }
    let mut pending: std::collections::HashMap<BlockPos, usize> = state
        .merge_block_writes
        .iter()
        .enumerate()
        .map(|(i, (pos, _))| (*pos, i))
        .collect();
    let mut stage = |state: &mut TickState,
                     pos: BlockPos,
                     block: u32,
                     image: Option<crate::command_block_entity::Prepared>| {
        state.stage_command_container_removal(pos);
        state
            .merge_command_block_entities
            .retain(|(p, _)| *p != pos);
        if let Some(image) = image {
            state.merge_command_block_entities.push((pos, image));
        }
        if let Some(index) = pending.get(&pos) {
            state.merge_block_writes[*index].1 = block;
        } else {
            pending.insert(pos, state.merge_block_writes.len());
            state.merge_block_writes.push((pos, block));
        }
        state.world.set_block(pos.x, pos.y, pos.z, block);
    };
    if mode == CloneMode::Move {
        let air = state.registry.states.air;
        for (source, _, _, _) in &blocks {
            stage(state, *source, air, None);
        }
    }
    let count = blocks.len() as i32;
    for (_, target, block, image) in blocks {
        stage(state, target, block, image);
    }
    Ok((
        format!("Cloned {count} block(s)"),
        crate::command_execution::CommandResult::Success(count),
    ))
}

fn edit_region(
    state: &mut TickState,
    a: BlockPos,
    b: BlockPos,
    block: u32,
    image: Option<crate::command_block_entity::Prepared>,
    mode: &FillMode,
) -> Result<(String, crate::command_execution::CommandResult), CommandError> {
    let min = BlockPos {
        x: a.x.min(b.x),
        y: a.y.min(b.y),
        z: a.z.min(b.z),
    };
    let max = BlockPos {
        x: a.x.max(b.x),
        y: a.y.max(b.y),
        z: a.z.max(b.z),
    };
    let volume = (i64::from(max.x) - i64::from(min.x) + 1) as u64
        * (i64::from(max.y) - i64::from(min.y) + 1) as u64
        * (i64::from(max.z) - i64::from(min.z) + 1) as u64;
    let limit = crate::command_rules::integer(
        state,
        crate::command_rules::IntRule::MAX_BLOCK_MODIFICATIONS,
    ) as u64;
    if volume > limit {
        return Err(message(format!(
            "Too many blocks: {volume} (maximum {limit})"
        )));
    }
    let mut pending: std::collections::HashMap<BlockPos, usize> = state
        .merge_block_writes
        .iter()
        .enumerate()
        .map(|(index, (pos, _))| (*pos, index))
        .collect();
    // Preflight every cell before staging anything, including interior cells:
    // an unloaded region must never turn a command into a partial edit.
    let mut writes = Vec::with_capacity(volume as usize);
    for z in min.z..=max.z {
        for y in min.y..=max.y {
            for x in min.x..=max.x {
                let pos = BlockPos { x, y, z };
                let old = pending
                    .get(&pos)
                    .map(|index| state.merge_block_writes[*index].1)
                    .or_else(|| state.world.block_at(x, y, z))
                    .ok_or_else(|| message("Position is not loaded"))?;
                let boundary = x == min.x
                    || x == max.x
                    || y == min.y
                    || y == max.y
                    || z == min.z
                    || z == max.z;
                let new = match mode {
                    FillMode::Outline if !boundary => continue,
                    FillMode::Hollow if !boundary => state.registry.states.air,
                    FillMode::Keep if !is_air(&state.registry.blocks, old) => continue,
                    FillMode::Replace(Some(filter))
                        if !filter.matches(&state.registry.blocks, old) =>
                    {
                        continue;
                    }
                    _ => block,
                };
                let destroy =
                    matches!(mode, FillMode::Destroy) && !is_air(&state.registry.blocks, old);
                if new != old || destroy || (new == block && image.is_some()) {
                    writes.push((pos, old, new, destroy));
                }
            }
        }
    }
    if writes.is_empty() {
        return Err(message("No blocks were changed"));
    }
    let mut count = 0;
    for (pos, old, new, destroy) in writes {
        if destroy {
            if !state.stage_command_break(pos, old) {
                continue;
            }
        } else if state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(old))
            .zip(state.registry.blocks.by_id(mc_world::BlockStateId(new)))
            .is_none_or(|(old, new)| {
                old.block.id != new.block.id
                    && !crate::copper::same_family(&old.block.id, &new.block.id)
            })
        {
            state.stage_command_container_removal(pos);
        }
        state
            .merge_command_block_entities
            .retain(|(pending, _)| *pending != pos);
        if new == block
            && let Some(image) = &image
        {
            state
                .merge_command_block_entities
                .push((pos, image.clone()));
        }
        if let Some(index) = pending.get(&pos) {
            state.merge_block_writes[*index].1 = new;
        } else {
            pending.insert(pos, state.merge_block_writes.len());
            state.merge_block_writes.push((pos, new));
        }
        // A later summon in this input batch must collide with the edited
        // world, not fall into a floor that only appears in the block phase.
        state.world.set_block(pos.x, pos.y, pos.z, new);
        count += 1;
    }
    if count == 0 {
        return Err(message("No blocks were changed"));
    }
    Ok((
        format!("Changed {count} block(s)"),
        crate::command_execution::CommandResult::Success(count),
    ))
}

pub(crate) fn is_air(registry: &BlockRegistry, state: u32) -> bool {
    registry
        .by_id(BlockStateId(state))
        .is_some_and(|state| matches!(state.block.id.path(), "air" | "cave_air" | "void_air"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boundary::{ChunkPos, PlayerInput};
    use crate::registry::CoreRegistry;
    use crate::tick::{BlockEntityState, run_tick};
    use crate::world::ChunkWorld;
    use std::sync::Arc;

    fn test_state() -> TickState {
        let registry = Arc::new(CoreRegistry::load(None).expect("registry"));
        let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
            42,
            Arc::clone(&registry.blocks),
        ));
        let world = ChunkWorld::new(
            42,
            generator,
            &registry,
            BlockStateId(registry.states.air),
            [0, 200, 0],
            mc_world::OVERWORLD_GEOMETRY,
        );
        TickState::new(registry, world, 2, 512)
    }

    fn join(state: &mut TickState, connection: u64, uuid: uuid::Uuid) -> EntityId {
        run_tick(
            state,
            &[PlayerInput::Join {
                connection: ConnectionId(connection),
                uuid,
                name: format!("builder{connection}"),
                permission_level: 0,
            }],
        );
        let entity = state
            .connections
            .entity_of(ConnectionId(connection))
            .expect("joined");
        let slot = state.entities.slot_of_any(entity).unwrap();
        state.entities.set_pos(slot, [0.5, 200.0, 0.5]);
        let wanted = state.desired_chunks();
        state.world.ensure_blocking(&wanted);
        entity
    }

    fn fixture() -> (TickState, EntityId) {
        let mut state = test_state();
        let entity = join(&mut state, 1, uuid::Uuid::new_v4());
        (state, entity)
    }

    fn parsed(command: &str) -> WorldCommand {
        let mut words = command.split_whitespace();
        let root = words.next().unwrap();
        parse(root, &words.collect::<Vec<_>>()).expect("parse")
    }

    fn staged(state: &TickState, x: i32, y: i32, z: i32) -> Option<u32> {
        let pos = BlockPos { x, y, z };
        state
            .merge_block_writes
            .iter()
            .rev()
            .find(|(cell, _)| *cell == pos)
            .map(|(_, block)| *block)
    }

    fn apply(
        state: &mut TickState,
        actor: EntityId,
        command: &str,
    ) -> Result<String, CommandError> {
        execute_inner(state, actor, &parsed(command)).map(|(message, _)| message)
    }

    #[test]
    fn fill_and_setblock_redstone_prime_neighbor_tnt_after_early_overlay_write() {
        for command in [
            "fill 1 200 1 1 200 1 minecraft:redstone_block",
            "setblock 1 200 1 minecraft:redstone_block",
            "fill 2 201 1 2 201 1 minecraft:redstone_block",
            "setblock 2 201 1 minecraft:redstone_block",
        ] {
            let (mut state, actor) = fixture();
            let tnt = block_spec("minecraft:tnt")
                .unwrap()
                .resolve(&state.registry.blocks)
                .unwrap();
            state.world.set_block(1, 201, 1, tnt);
            state.world.fold_overlay();
            // Hydrate before the command, so a fresh-chunk scan cannot hide a
            // missing neighbour notification from the ordinary block phase.
            run_tick(&mut state, &[]);
            apply(&mut state, actor, command).unwrap();
            for &(pos, id) in &state.merge_block_writes {
                assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), Some(id));
            }
            let (output, _, _) = run_tick(&mut state, &[]);
            let primed: Vec<_> = state
                .entities
                .slots()
                .filter_map(|slot| {
                    let body = state.entities.body(slot);
                    let row = state.entities.row(slot);
                    row.tnt.is_some().then_some(body.id)
                })
                .collect();
            assert_eq!(primed.len(), 1, "{command}");
            assert_eq!(
                state.world.block_at(1, 201, 1),
                Some(state.registry.states.air)
            );
            assert!(output.changes.entities_spawned.contains(&primed[0]));
            assert!(output.changes.blocks.iter().any(|change| {
                change.position == BlockPos { x: 1, y: 201, z: 1 }
                    && change.state == state.registry.states.air
            }));
            run_tick(&mut state, &[]);
            assert_eq!(
                state
                    .entities
                    .slots()
                    .filter(|slot| state.entities.row(*slot).tnt.is_some())
                    .count(),
                1,
                "{command} must not prime twice"
            );
        }
    }

    #[test]
    fn fill_and_setblock_water_schedule_fluid_after_early_overlay_write() {
        for command in [
            "fill 1 202 1 1 202 1 minecraft:water",
            "setblock 1 202 1 minecraft:water",
        ] {
            let (mut state, actor) = fixture();
            run_tick(&mut state, &[]);
            apply(&mut state, actor, command).unwrap();
            let water = staged(&state, 1, 202, 1).unwrap();
            assert_eq!(state.world.block_at(1, 202, 1), Some(water));
            run_tick(&mut state, &[]);
            assert!(
                state
                    .fluid_ticks
                    .image()
                    .due
                    .iter()
                    .any(|tick| tick.pos == [1, 202, 1])
            );
            for _ in 0..5 {
                run_tick(&mut state, &[]);
            }
            assert!(
                state.world.block_at(1, 201, 1).is_some_and(|block| {
                    crate::fluids::fluid_at(&state.registry.states, block)
                        .is_some_and(|(fluid, _)| fluid == crate::fluids::Fluid::Water)
                }),
                "{command} must flow into the cell below"
            );
        }
    }

    #[test]
    fn parses_each_family_and_rejects_unsupported_syntax() {
        for command in [
            "setblock ~ ~1 ~ stone",
            "setblock 1 2 3 stone destroy",
            "setblock 0 0 0 stone{foo:1}",
            "setblock ^1 ^2 ^3 stone",
            "setblock 1 2 3 grass_block[snowy=true] keep",
            "fill 0 0 0 2 2 2 stone replace dirt",
            "fill 0 0 0 2 2 2 stone outline",
            "fill 0 0 0 2 2 2 stone hollow",
            "fill 0 0 0 2 2 2 stone keep",
            "fill 0 0 0 2 2 2 stone destroy",
            "fill ^ ^ ^ ^2 ^2 ^2 stone outline",
            "spawnpoint",
            "spawnpoint @a",
            "spawnpoint @s ~ ~ ~ 90 -20",
            "spawnpoint @s ^ ^1 ^2 ~90 ~20",
            "setworldspawn",
            "setworldspawn ~ ~ ~ ~90 ~",
            "setworldspawn ^ ^1 ^2",
        ] {
            parsed(command);
        }
        for (root, args) in [
            ("setblock", vec!["0", "0", "0", "stone{foo:"]),
            (
                "setblock",
                vec!["0", "0", "0", "grass_block[snowy=true,snowy=false]"],
            ),
            ("setblock", vec!["0", "0", "0", "stone", "strict"]),
            ("setblock", vec!["^1", "~", "^3", "stone"]),
            ("spawnpoint", vec!["@s", "0", "200", "0", "^90", "0"]),
            ("setworldspawn", vec!["0", "200", "0", "0", "^30"]),
            (
                "fill",
                vec!["0", "0", "0", "1", "1", "1", "stone", "keep", "dirt"],
            ),
            ("spawnpoint", vec!["@unsupported"]),
            ("setworldspawn", vec!["0", "0"]),
        ] {
            assert!(parse(root, &args).is_err(), "{root} {args:?}");
        }
    }

    #[test]
    fn local_positions_follow_executor_rotation_for_every_world_family() {
        let (mut state, actor) = fixture();
        let slot = state.entities.slot_of_any(actor).unwrap();
        state.entities.set_rot(slot, 90.0, 0.0);
        apply(&mut state, actor, "setblock ^1 ^2 ^3 stone").unwrap();
        assert_eq!(
            staged(&state, -3, 202, 1),
            Some(state.registry.states.stone)
        );
        apply(&mut state, actor, "fill ^ ^ ^ ^1 ^1 ^1 dirt").unwrap();
        assert_eq!(state.merge_block_writes.len(), 9);
        assert_eq!(staged(&state, -1, 201, 1), Some(state.registry.states.dirt));
        apply(&mut state, actor, "spawnpoint @s ^1 ^2 ^3 ~10 ~20").unwrap();
        assert_eq!(state.players[&actor].respawn_point, Some([-3, 202, 1]));
        assert_eq!(
            [
                state.players[&actor].respawn_yaw,
                state.players[&actor].respawn_pitch
            ],
            [100.0, 20.0]
        );
        apply(&mut state, actor, "setworldspawn ^ ^3 ^4").unwrap();
        assert_eq!(state.world.world_spawn(), [-4, 203, 0]);
    }

    #[test]
    fn setblock_resolves_properties_and_relative_coordinates_before_staging() {
        let (mut state, actor) = fixture();
        apply(&mut state, actor, "setblock ~1 ~ ~ grass_block[snowy=true]").unwrap();
        let block = state
            .registry
            .blocks
            .by_id(BlockStateId(staged(&state, 1, 200, 0).unwrap()))
            .unwrap();
        assert!(
            block
                .properties
                .contains(&("snowy".to_owned(), "true".to_owned()))
        );
        let count = state.merge_block_writes.len();
        for command in [
            "setblock 2 200 0 missing_block",
            "setblock 2 200 0 grass_block[snowy=invalid]",
            "setblock 2 200 0 stone[unknown=true]",
            "setblock 2 320 0 stone",
        ] {
            assert!(apply(&mut state, actor, command).is_err());
            assert_eq!(state.merge_block_writes.len(), count);
        }
        assert!(apply(&mut state, actor, "setblock 1 200 0 dirt keep").is_err());
        assert_eq!(state.merge_block_writes.len(), count);
    }

    #[test]
    fn setblock_snbt_installs_native_chest_contents_and_replaces_same_block() {
        let (mut state, actor) = fixture();
        for count in [7, 3] {
            apply(
                &mut state,
                actor,
                &format!("setblock 1 200 0 chest{{Items:[{{Slot:0b,id:\"minecraft:diamond\",count:{count}}}]}}"),
            )
            .unwrap();
            run_tick(&mut state, &[]);
            let slots = &state.block_entities[&(1, 200, 0)].slots;
            assert_eq!(slots[0].count, count);
            assert_eq!(
                state
                    .registry
                    .items
                    .name_of(slots[0].item_id)
                    .unwrap()
                    .as_str(),
                "minecraft:diamond"
            );
        }
        let before = state.block_entities[&(1, 200, 0)].slots.clone();
        assert!(
            apply(
                &mut state,
                actor,
                "setblock 1 200 0 chest{Items:[{Slot:0b,id:\"minecraft:dirt\",count:9}]} keep"
            )
            .is_err()
        );
        assert_eq!(state.block_entities[&(1, 200, 0)].slots, before);
    }

    #[test]
    fn fill_outline_hollow_keep_and_filter_have_distinct_footprints() {
        for mode in ["outline", "hollow", "keep", "replace stone"] {
            let (mut state, actor) = fixture();
            let stone = state.registry.states.stone;
            let dirt = state.registry.states.dirt;
            if mode == "replace stone" {
                for x in 0..=2 {
                    for y in 200..=202 {
                        for z in 0..=2 {
                            state.world.set_block(x, y, z, stone);
                        }
                    }
                }
            }
            state.world.set_block(1, 201, 1, dirt);
            let destination = if mode == "replace stone" {
                "dirt"
            } else {
                "stone"
            };
            apply(
                &mut state,
                actor,
                &format!("fill 0 200 0 2 202 2 {destination} {mode}"),
            )
            .unwrap();
            if mode == "hollow" {
                assert_eq!(state.merge_block_writes.len(), 27);
                assert_eq!(staged(&state, 1, 201, 1), Some(state.registry.states.air));
            } else {
                assert_eq!(state.merge_block_writes.len(), 26, "{mode}");
                assert_eq!(staged(&state, 1, 201, 1), None, "{mode}");
            }
        }
    }

    #[test]
    fn fill_filter_matches_only_explicit_properties() {
        let (mut state, actor) = fixture();
        let snowy = block_spec("grass_block[snowy=true]")
            .unwrap()
            .resolve(&state.registry.blocks)
            .unwrap();
        let plain = block_spec("grass_block[snowy=false]")
            .unwrap()
            .resolve(&state.registry.blocks)
            .unwrap();
        state.world.set_block(0, 200, 0, snowy);
        state.world.set_block(1, 200, 0, plain);
        apply(
            &mut state,
            actor,
            "fill 0 200 0 1 200 0 stone replace grass_block[snowy=true]",
        )
        .unwrap();
        assert_eq!(state.merge_block_writes.len(), 1);
        assert_eq!(staged(&state, 0, 200, 0), Some(state.registry.states.stone));
    }

    #[test]
    fn fill_preflights_limit_and_residency_without_partial_writes() {
        let (mut state, actor) = fixture();
        apply(&mut state, actor, "setblock 0 200 0 dirt").unwrap();
        let before = state.merge_block_writes.clone();
        assert!(apply(&mut state, actor, "fill 0 200 0 32 231 31 stone").is_err());
        assert_eq!(state.merge_block_writes, before);
        assert!(
            apply(
                &mut state,
                actor,
                "fill 0 200 0 30000000 201 30000000 stone"
            )
            .is_err()
        );
        assert_eq!(state.merge_block_writes, before);
        // Generated dirty chunks survive eviction until checkpointed. Cross
        // into a chunk that this fixture has never loaded instead.
        assert!(state.world.chunk(ChunkPos { x: 64, z: 0 }).is_none());
        assert!(apply(&mut state, actor, "fill 0 200 0 1024 200 0 stone").is_err());
        assert_eq!(state.merge_block_writes, before);
        let wanted = (0..=1)
            .flat_map(|x| (0..=1).map(move |z| (ChunkPos { x, z }, 0)))
            .collect();
        state.world.ensure_blocking(&wanted);
        apply(&mut state, actor, "fill 0 200 0 31 231 31 stone").unwrap();
        assert_eq!(state.merge_block_writes.len(), 32_768);
    }

    #[test]
    fn fill_limit_gamerule_alias_changes_admission_without_partial_mutation() {
        let (mut state, actor) = fixture();
        run_tick(
            &mut state,
            &[PlayerInput::Command {
                connection: ConnectionId(1),
                command: "gamerule commandModificationBlockLimit 2".into(),
                permission_level: 4,
            }],
        );
        let before = state.merge_block_writes.clone();
        let error = apply(&mut state, actor, "fill 0 200 0 2 200 0 stone").unwrap_err();
        assert!(matches!(error, CommandError::Message(message) if message.contains("maximum 2")));
        assert_eq!(state.merge_block_writes, before);
        apply(&mut state, actor, "fill 0 200 0 1 200 0 stone").unwrap();
        assert_eq!(state.merge_block_writes.len(), 2);
        let (output, _, _) = run_tick(&mut state, &[]);
        assert_eq!(output.changes.blocks.len(), 2);
        assert_eq!(
            state.world.block_at(0, 200, 0),
            Some(state.registry.states.stone)
        );
        assert_eq!(
            state.world.block_at(1, 200, 0),
            Some(state.registry.states.stone)
        );
    }

    #[test]
    fn block_commands_commit_and_emit_block_journal_postimages() {
        let (mut state, actor) = fixture();
        apply(&mut state, actor, "fill 0 200 0 1 200 1 stone").unwrap();
        let (output, _, _) = run_tick(&mut state, &[]);
        let entries = crate::core::journal_entries_for_tick(&mut state, &output);
        for x in 0..=1 {
            for z in 0..=1 {
                assert_eq!(
                    state.world.block_at(x, 200, z),
                    Some(state.registry.states.stone)
                );
                assert!(entries.iter().any(|entry| matches!(entry, crate::journal::JournalEntry::Block { pos, state: block } if *pos == [x, 200, z] && *block == state.registry.states.stone)));
            }
        }
        apply(&mut state, actor, "setblock 0 200 0 dirt").unwrap();
        let (output, _, _) = run_tick(&mut state, &[]);
        let entries = crate::core::journal_entries_for_tick(&mut state, &output);
        assert!(entries.iter().any(|entry| matches!(entry, crate::journal::JournalEntry::Block { pos, state: block } if *pos == [0, 200, 0] && *block == state.registry.states.dirt)));
    }

    #[test]
    fn destroy_spills_container_custody_and_replace_clears_it() {
        for (root, mode) in [
            ("setblock", "destroy"),
            ("setblock", "replace"),
            ("fill", "destroy"),
            ("fill", "replace"),
        ] {
            let (mut state, actor) = fixture();
            let chest = block_spec("chest")
                .unwrap()
                .resolve(&state.registry.blocks)
                .unwrap();
            state.world.set_block(0, 200, 0, chest);
            state.world.fold_overlay();
            let item = state
                .registry
                .items
                .id_of(&Identifier::parse("minecraft:dirt").unwrap())
                .unwrap();
            state.block_entities.insert(
                (0, 200, 0),
                BlockEntityState {
                    slots: vec![mc_data::ItemStack::new(item, 7)],
                    furnace: None,
                },
            );
            state.players.get_mut(&actor).unwrap().game_mode = crate::commands::GameMode::Creative;
            let positions = if root == "fill" {
                "0 200 0 0 200 0"
            } else {
                "0 200 0"
            };
            apply(
                &mut state,
                actor,
                &format!("{root} {positions} stone {mode}"),
            )
            .unwrap();
            assert_eq!(
                state.merge_command_broken.len(),
                usize::from(mode == "destroy")
            );
            assert_eq!(
                state.merge_container_contents[0].1.len(),
                usize::from(mode == "destroy")
            );
            let (output, _, _) = run_tick(&mut state, &[]);
            assert!(!state.block_entities.contains_key(&(0, 200, 0)));
            assert_eq!(
                state.world.block_at(0, 200, 0),
                Some(state.registry.states.stone)
            );
            if mode == "destroy" {
                assert!(!output.changes.entities_spawned.is_empty());
            }
            let entries = crate::core::journal_entries_for_tick(&mut state, &output);
            assert!(entries.iter().any(|entry| matches!(entry, crate::journal::JournalEntry::RemoveBlockEntity { pos, .. } if *pos == [0, 200, 0])));
        }
    }

    #[test]
    fn persisted_forced_spawn_restores_and_drives_actual_respawn() {
        let (mut state, actor) = fixture();
        apply(&mut state, actor, "spawnpoint @s 4 200 4 450 -30").unwrap();
        let (output, _, _) = run_tick(&mut state, &[]);
        let entries = crate::core::journal_entries_for_tick(&mut state, &output);
        let replay = crate::journal::replay_batches(
            Default::default(),
            &[(output.tick, output.tick, entries)],
        );
        let slot = state.entities.slot_of_any(actor).unwrap();
        let persisted = &replay.players[&state.entities.row(slot).uuid.to_string()];
        assert!(persisted.respawn_forced);
        assert_eq!(
            [persisted.respawn_yaw, persisted.respawn_pitch],
            [90.0, -30.0]
        );
        let record = crate::core::player_record_for(&state, actor, state.tick).unwrap();
        let dir =
            std::env::temp_dir().join(format!("solaris-command-spawn-{}", uuid::Uuid::new_v4()));
        let uuid = uuid::Uuid::new_v4();
        let config = crate::CoreConfig::new(dir.clone(), 42);
        crate::native_checkpoint::tests::write_player(
            &config,
            &uuid.to_string(),
            &record,
            &state.registry.items,
        )
        .unwrap();
        let restored = crate::native_checkpoint::tests::read_player(
            &config,
            &uuid.to_string(),
            &state.registry.items,
        )
        .unwrap();
        assert_eq!(restored.respawn, Some([4, 200, 4]));
        assert!(restored.respawn_forced);
        assert_eq!(
            [restored.respawn_yaw, restored.respawn_pitch],
            [90.0, -30.0]
        );
        let mut reopened = test_state();
        reopened.offline_players.insert(uuid, restored);
        let player = join(&mut reopened, 2, uuid);
        assert!(reopened.players[&player].respawn_forced);
        let slot = reopened.entities.slot_of_any(player).unwrap();
        reopened.entities.body_mut(slot).alive = false;
        reopened.entities.row_mut(slot).health = 0.0;
        reopened.players.get_mut(&player).unwrap().vitals.health = 0.0;
        let (_, queues, _) = run_tick(
            &mut reopened,
            &[PlayerInput::Respawn {
                connection: ConnectionId(2),
            }],
        );
        assert!(queues.iter().flat_map(|(_, queue)| queue).any(|change| matches!(change, PlayerChange::Respawned { position, yaw, pitch, .. } if *position == [4.5, 200.1, 4.5] && *yaw == 90.0 && *pitch == -30.0)));
        let slot = reopened.entities.slot_of_any(player).unwrap();
        assert_eq!(reopened.entities.rot(slot), [90.0, -30.0, 90.0]);
        reopened.players.get_mut(&player).unwrap().respawn_forced = false;
        assert_eq!(crate::sleep::respawn_position(&reopened, player), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn worldspawn_changes_default_respawn_and_checkpoint_metadata() {
        let (mut state, actor) = fixture();
        apply(&mut state, actor, "setworldspawn 12 210 13 270 25").unwrap();
        assert_eq!(state.world.world_spawn(), [12, 210, 13]);
        assert_eq!(state.world.world_spawn_rotation(), [-90.0, 25.0]);
        assert_eq!(crate::sleep::respawn_rotation(&state, actor), [-90.0, 25.0]);
        let dir = std::env::temp_dir().join(format!(
            "solaris-command-worldspawn-{}",
            uuid::Uuid::new_v4()
        ));
        let mut config = crate::config::CoreConfig::new(dir.clone(), 42);
        config.simulation_distance = 1;
        crate::core::Core::open(config.clone()).unwrap().shutdown();
        let mut baseline = crate::native_checkpoint::tests::read_manifest(&config).unwrap();
        baseline.world_spawn = Some(state.world.world_spawn());
        baseline.world_spawn_rotation = state.world.world_spawn_rotation();
        crate::native_checkpoint::tests::write_manifest(&config, &baseline).unwrap();
        let manifest = crate::native_checkpoint::tests::read_manifest(&config).unwrap();
        let mut reopened = test_state();
        reopened
            .world
            .set_world_spawn(manifest.world_spawn.unwrap(), manifest.world_spawn_rotation);
        assert_eq!(reopened.world.world_spawn_position(), [12.5, 210.0, 13.5]);
        assert_eq!(reopened.world.world_spawn_rotation(), [-90.0, 25.0]);
        let core = crate::core::Core::open(config.clone())
            .expect("mutable respawn spawn must not change generation identity");
        assert_eq!(core.world_spawn(), [12, 210, 13]);
        core.shutdown();
        let captured = crate::native_checkpoint::tests::read_manifest(&config).unwrap();
        assert_eq!(captured.world_spawn, Some([12, 210, 13]));
        assert_eq!(captured.world_spawn_rotation, [-90.0, 25.0]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn spawnpoint_targets_players_and_keeps_failed_commands_atomic() {
        let (mut state, actor) = fixture();
        let second = join(&mut state, 2, uuid::Uuid::new_v4());
        apply(&mut state, actor, "spawnpoint @a ~2 ~ ~3 ~90 100").unwrap();
        for player in [actor, second] {
            assert_eq!(state.players[&player].respawn_point, Some([2, 200, 3]));
            assert_eq!(state.players[&player].respawn_pitch, 90.0);
        }
        assert!(apply(&mut state, actor, "spawnpoint @a 0 320 0").is_err());
        assert_eq!(state.players[&second].respawn_point, Some([2, 200, 3]));
        assert!(apply(&mut state, actor, "spawnpoint nobody").is_err());
    }

    #[test]
    fn sleeping_replaces_forced_spawn_with_a_bed_dependent_point() {
        let (mut state, actor) = fixture();
        apply(&mut state, actor, "spawnpoint @s 8 200 8 90 -10").unwrap();
        state.day_time = 13_000;
        let stone = state.registry.states.stone;
        for x in 2..=7 {
            for z in 2..=6 {
                state.world.set_block(x, 199, z, stone);
            }
        }
        let foot = block_spec("red_bed[facing=east,part=foot]")
            .unwrap()
            .resolve(&state.registry.blocks)
            .unwrap();
        let head = block_spec("red_bed[facing=east,part=head]")
            .unwrap()
            .resolve(&state.registry.blocks)
            .unwrap();
        state.world.set_block(4, 200, 4, foot);
        state.world.set_block(5, 200, 4, head);
        state.world.fold_overlay();
        let slot = state.entities.slot_of(actor).unwrap();
        state.entities.set_pos(slot, [4.5, 200.0, 2.5]);
        assert!(crate::sleep::use_bed(
            &mut state,
            actor,
            ConnectionId(1),
            BlockPos { x: 4, y: 200, z: 4 }
        ));
        assert_eq!(state.players[&actor].respawn_point, Some([5, 200, 4]));
        assert!(!state.players[&actor].respawn_forced);
        assert!(crate::sleep::respawn_position(&state, actor).is_some());
        let air = state.registry.states.air;
        state.world.set_block(5, 200, 4, air);
        assert_eq!(crate::sleep::respawn_position(&state, actor), None);
    }

    #[test]
    fn remote_spawn_waits_for_residency_and_does_not_fall_back() {
        for immediate in [false, true] {
            let (mut state, actor) = fixture();
            state
                .gamerules
                .insert("immediate_respawn".to_owned(), immediate.to_string());
            apply(&mut state, actor, "spawnpoint @s 128 200 128 30 10").unwrap();
            let slot = state.entities.slot_of_any(actor).unwrap();
            state.entities.body_mut(slot).alive = false;
            state.entities.row_mut(slot).health = 0.0;
            state.players.get_mut(&actor).unwrap().vitals.health = 0.0;
            assert!(!crate::sleep::respawn_chunks_loaded(&state, actor));
            let inputs = if immediate {
                Vec::new()
            } else {
                vec![PlayerInput::Respawn {
                    connection: ConnectionId(1),
                }]
            };
            let (_, queues, _) = run_tick(&mut state, &inputs);
            assert!(
                !queues
                    .iter()
                    .flat_map(|(_, queue)| queue)
                    .any(|change| matches!(change, PlayerChange::Respawned { .. }))
            );
            let wanted = state.desired_chunks();
            state.world.ensure_blocking(&wanted);
            assert!(crate::sleep::respawn_chunks_loaded(&state, actor));
            let (_, queues, _) = run_tick(&mut state, &[]);
            assert!(queues.iter().flat_map(|(_, queue)| queue).any(|change| matches!(change, PlayerChange::Respawned { position, yaw, pitch, .. } if *position == [128.5, 200.1, 128.5] && *yaw == 30.0 && *pitch == 10.0)));
        }
    }
}
