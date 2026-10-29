//! Event-driven command blocks. Opaque NBT and scheduled ticks are native WAL owners.
use crate::boundary::{BlockPos, ChangeSet, ChunkPos, ConnectionId, EntityId, PlayerChange};
use crate::tick::TickState;
use mc_nbt::Tag;
use mc_world::{BlockStateId, ScheduledBlockTick};

pub(crate) fn is_block(path: &str) -> bool {
    matches!(
        path,
        "command_block" | "chain_command_block" | "repeating_command_block"
    )
}
pub(crate) fn can_place(state: &TickState, entity: EntityId) -> bool {
    state.players.get(&entity).is_some_and(|player| {
        player.creative() && state.plugin_players.is_operator(player.connection)
    })
}
fn kind(state: &TickState, pos: BlockPos) -> Option<&str> {
    let block = state
        .registry
        .blocks
        .by_id(BlockStateId(state.world.block_at(pos.x, pos.y, pos.z)?))?;
    is_block(block.block.id.path()).then(|| block.block.id.path())
}
fn read(state: &TickState, pos: BlockPos) -> Result<Vec<(String, Tag)>, String> {
    let chunk = state
        .world
        .chunk(ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        })
        .ok_or("Command block chunk is not loaded")?;
    let Some(bytes) = chunk.block_entities.get(&pos) else {
        return Ok(Vec::new());
    };
    let mut input = bytes.as_slice();
    match mc_nbt::read_network(&mut input) {
        Ok(Tag::Compound(fields)) if input.is_empty() => {
            for (index, (name, value)) in fields.iter().enumerate() {
                if fields[..index].iter().any(|(other, _)| other == name) {
                    return Err("Duplicate command block NBT field".into());
                }
                let valid = match name.as_str() {
                    "Command" => matches!(value, Tag::String(text) if text.len() <= 32767),
                    "auto" | "powered" => matches!(value, Tag::Byte(0 | 1)),
                    "LastExecution" => matches!(value, Tag::Long(_)),
                    _ => true,
                };
                if !valid {
                    return Err(format!("Corrupt command block field {name}"));
                }
            }
            Ok(fields)
        }
        _ => Err("Corrupt command block NBT".into()),
    }
}
fn flag(fields: &[(String, Tag)], name: &str) -> bool {
    matches!(mc_data::item_nbt::field(fields, name), Some(Tag::Byte(value)) if *value != 0)
}
fn put(fields: &mut Vec<(String, Tag)>, name: &str, value: Tag) {
    if let Some((_, old)) = fields.iter_mut().find(|(key, _)| key == name) {
        *old = value;
    } else {
        fields.push((name.into(), value));
    }
}
fn save(state: &mut TickState, pos: BlockPos, mut fields: Vec<(String, Tag)>) {
    for (key, value) in [
        ("id", Tag::String("minecraft:command_block".into())),
        ("x", Tag::Int(pos.x)),
        ("y", Tag::Int(pos.y)),
        ("z", Tag::Int(pos.z)),
    ] {
        put(&mut fields, key, value);
    }
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).expect("command block NBT");
    state
        .world
        .set_opaque_block_entity(pos, Some(bytes.clone()));
    state.opaque_changes.push((pos, Some(bytes)));
}
fn schedule(state: &mut TickState, pos: BlockPos) {
    let Some(block) = state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .and_then(|id| state.registry.blocks.by_id(BlockStateId(id)))
    else {
        return;
    };
    if !state.block_ticks.contains(pos, &block.block.id, 0) {
        state.block_ticks.schedule(ScheduledBlockTick::new(
            pos,
            block.block.id.clone(),
            crate::command_context::execution_tick(state).saturating_add(1),
            0,
        ));
    }
}
pub(crate) fn changed(state: &mut TickState, pos: BlockPos) {
    if !state.enable_command_block {
        return;
    }
    let Some(kind) = kind(state, pos) else {
        return;
    };
    let repeating = kind == "repeating_command_block";
    let chain = kind == "chain_command_block";
    let mut fields = match read(state, pos) {
        Ok(fields) => fields,
        Err(error) => {
            crate::block_drop_context::reject(state, pos, crate::CoreError::Storage(error));
            return;
        }
    };
    let old = flag(&fields, "powered");
    let powered = crate::redstone::power_at(state, pos) > 0;
    let auto = flag(&fields, "auto");
    if old != powered {
        put(&mut fields, "powered", Tag::Byte(i8::from(powered)));
        save(state, pos, fields);
    }
    if !chain && ((powered && !old) || (repeating && (auto || powered))) {
        schedule(state, pos);
    }
}
fn direction(state: &TickState, pos: BlockPos) -> Option<(i32, i32, i32)> {
    let block = state
        .registry
        .blocks
        .by_id(BlockStateId(state.world.block_at(pos.x, pos.y, pos.z)?))?;
    let facing = block
        .properties
        .iter()
        .find(|(name, _)| name == "facing")?
        .1
        .as_str();
    match facing {
        "east" => Some((1, 0, 0)),
        "west" => Some((-1, 0, 0)),
        "up" => Some((0, 1, 0)),
        "down" => Some((0, -1, 0)),
        "south" => Some((0, 0, 1)),
        "north" => Some((0, 0, -1)),
        _ => None,
    }
}
fn conditional(state: &TickState, pos: BlockPos) -> bool {
    state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .and_then(|id| state.registry.blocks.by_id(BlockStateId(id)))
        .is_some_and(|block| {
            block
                .properties
                .iter()
                .any(|(name, value)| name == "conditional" && value == "true")
        })
}
fn condition_met(state: &TickState, pos: BlockPos) -> bool {
    if !conditional(state, pos) {
        return true;
    }
    let Some((x, y, z)) = direction(state, pos) else {
        return false;
    };
    let Some(behind) = pos
        .x
        .checked_sub(x)
        .zip(pos.y.checked_sub(y))
        .zip(pos.z.checked_sub(z))
        .map(|((x, y), z)| BlockPos { x, y, z })
    else {
        return false;
    };
    kind(state, behind).is_some() && read(state, behind).is_ok_and(|fields| {
        matches!(
            mc_data::item_nbt::field(&fields, "SuccessCount"), Some(Tag::Int(count)) if *count > 0
        )
    })
}

/// Runs immediately after redstone drained the durable queue, before the block commit.
pub(crate) fn run(state: &mut TickState, changes: &mut ChangeSet) {
    let mut due = std::mem::take(&mut state.command_blocks_due);
    if state.enable_command_block {
        let mut remaining = 1024;
        for start in due.drain(..) {
            if state.dimension_globals.active && !state.command_context.shared {
                if state.command_context.jobs.len() < crate::command_context::LIMIT {
                    let context = crate::command_context::Context::source(
                        state,
                        crate::command_console::CONNECTION,
                        crate::command_console::EXECUTOR,
                        2,
                    );
                    state
                        .command_context
                        .jobs
                        .push(crate::command_context::Job {
                            text: String::new(),
                            context,
                            block: Some(start),
                        });
                }
                continue;
            }
            if remaining == 0 {
                schedule(state, start);
                continue;
            }
            let mut pos = start;
            loop {
                if remaining == 0 {
                    break;
                }
                remaining -= 1;
                let Some(path) = kind(state, pos) else {
                    break;
                };
                let repeating = path == "repeating_command_block";
                let chain = path == "chain_command_block";
                let mut fields = match read(state, pos) {
                    Ok(fields) => fields,
                    Err(error) => {
                        crate::block_drop_context::reject(
                            state,
                            pos,
                            crate::CoreError::Storage(error),
                        );
                        break;
                    }
                };
                let enabled = flag(&fields, "auto") || flag(&fields, "powered");
                if repeating && enabled {
                    schedule(state, pos);
                }
                if ((!chain && !repeating) || enabled)
                    && conditional(state, pos)
                    && !condition_met(state, pos)
                {
                    put(&mut fields, "SuccessCount", Tag::Int(0));
                    save(state, pos, fields);
                    if !chain {
                        break;
                    }
                } else if (!chain && !repeating) || enabled {
                    if matches!(mc_data::item_nbt::field(&fields,"LastExecution"),Some(Tag::Long(tick)) if *tick == state.tick as i64)
                    {
                        break;
                    }
                    let command = match mc_data::item_nbt::field(&fields, "Command") {
                        Some(Tag::String(command)) => Some(command.clone()),
                        _ => None,
                    };
                    put(&mut fields, "LastExecution", Tag::Long(state.tick as i64));
                    put(&mut fields, "SuccessCount", Tag::Int(0));
                    save(state, pos, fields);
                    if let Some(command) = command {
                        let previous = state.datapacks.origin.replace([
                            pos.x as f64 + 0.5,
                            pos.y as f64 + 0.5,
                            pos.z as f64 + 0.5,
                        ]);
                        let context = crate::command_context::Context::source(
                            state,
                            crate::command_console::CONNECTION,
                            crate::command_console::EXECUTOR,
                            2,
                        );
                        let successes = crate::command_context::run(
                            state,
                            command.trim_start_matches('/'),
                            context,
                            changes,
                        );
                        if let Ok(mut fields) = read(state, pos) {
                            put(&mut fields, "SuccessCount", Tag::Int(successes));
                            save(state, pos, fields);
                        }
                        state.datapacks.origin = previous;
                    }
                }
                let Some((x, y, z)) = direction(state, pos) else {
                    break;
                };
                let Some(next) = pos
                    .x
                    .checked_add(x)
                    .zip(pos.y.checked_add(y))
                    .zip(pos.z.checked_add(z))
                    .map(|((x, y), z)| BlockPos { x, y, z })
                else {
                    break;
                };
                if kind(state, next) != Some("chain_command_block") {
                    break;
                }
                pos = next;
            }
        }
    }
    due.clear();
    state.command_blocks_due = due;
}

/// Restricted /data merge block surface; other NBT owners are never bypassed.
pub(crate) fn data(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    text: &str,
) -> crate::command_execution::CommandResult {
    let result = (|| -> Result<String, String> {
        let mut words = text.split_whitespace();
        words.next();
        let mode = words.next().ok_or("Expected merge")?;
        if mode != "merge" || words.next() != Some("block") {
            return Err(
                "Supported writes: data merge block <x> <y> <z> <compound> (command blocks only)"
                    .into(),
            );
        }
        let coords = [
            words.next().ok_or("Missing x")?,
            words.next().ok_or("Missing y")?,
            words.next().ok_or("Missing z")?,
        ];
        let parsed = crate::commands::coords(&coords).ok_or("Invalid coordinates")?;
        let (origin, rotation) =
            crate::command_console::pose(state, executor).ok_or("Missing command source")?;
        let [x, y, z] = crate::command_context::resolve_coords(state, parsed, origin, rotation);
        if [x, y, z]
            .iter()
            .any(|value| !value.is_finite() || *value < i32::MIN as f64 || *value > i32::MAX as f64)
        {
            return Err("Invalid coordinates".into());
        }
        let pos = BlockPos {
            x: x.floor() as i32,
            y: y.floor() as i32,
            z: z.floor() as i32,
        };
        if kind(state, pos).is_none() {
            return Err("Data editing currently supports command blocks only".into());
        }
        let mut fields = read(state, pos)?;
        let raw = &text[text.find('{').ok_or("Expected NBT compound")?..];
        let Tag::Compound(patch) = crate::command_snbt::parse(raw).map_err(str::to_owned)? else {
            return Err("Expected NBT compound".into());
        };
        for (index, (key, value)) in patch.iter().enumerate() {
            if patch[..index].iter().any(|(old, _)| old == key) {
                return Err("Duplicate NBT field".into());
            }
            match (key.as_str(), value) {
                ("Command", Tag::String(value)) if value.len() <= 32767 => {}
                ("auto", Tag::Byte(0 | 1)) => {}
                _ => return Err(format!("Unsupported command block field {key}")),
            }
        }
        if !patch.iter().any(|(key, value)| {
            fields
                .iter()
                .find(|(name, _)| name == key)
                .is_none_or(|(_, old)| old != value)
        }) {
            return Err("The target block's data did not change".into());
        }
        let old_auto = flag(&fields, "auto");
        for (key, value) in patch {
            put(&mut fields, &key, value);
        }
        let start = !old_auto && flag(&fields, "auto") && kind(state, pos) == Some("command_block");
        save(state, pos, fields);
        changed(state, pos);
        if state.enable_command_block && start {
            schedule(state, pos);
        }
        Ok("Command block data updated".into())
    })();
    let (message, outcome) = match result {
        Ok(message) => (message, crate::command_execution::CommandResult::Success(1)),
        Err(message) => (message, crate::command_execution::CommandResult::Failed),
    };
    state.push_player(connection, PlayerChange::Chat { message });
    outcome
}

#[cfg(test)]
#[path = "command_blocks_tests.rs"]
mod tests;
