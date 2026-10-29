//! Operator-only read queries over native entity and block-entity owners.
use crate::boundary::{BlockPos, ChunkPos, ConnectionId, EntityId};
use crate::command_context::Context;
use crate::command_nbt_path::{argument, get, number, result, snbt};
use crate::tick::TickState;
use mc_nbt::{ListTag, Tag, tag_type};

fn put(fields: &mut Vec<(String, Tag)>, key: &str, value: Tag) {
    if let Some((_, old)) = fields.iter_mut().find(|(name, _)| name == key) {
        *old = value;
    } else {
        fields.push((key.into(), value));
    }
}
fn list(elements: Vec<Tag>, element_type: u8) -> Tag {
    Tag::List(ListTag {
        element_type,
        elements,
    })
}
fn public_fields(fields: &mut Vec<(String, Tag)>) {
    fields.retain(|(key, _)| {
        !key.starts_with("solaris") && !key.starts_with("Solaris") && key != "DataVersion"
    });
}

pub(crate) fn entity(state: &TickState, id: EntityId) -> Result<Tag, String> {
    let slot = state
        .entities
        .slot_of_any(id)
        .ok_or("No entity was found")?;
    let row = state.entities.row(slot);
    let mut fields = if let Some(player) = state.players.get(&id) {
        let Tag::Compound(mut fields) =
            crate::core::player_data_tag(state, id).map_err(|e| e.to_string())?
        else {
            unreachable!()
        };
        let uuid = row.uuid.as_u128();
        put(
            &mut fields,
            "UUID",
            Tag::IntArray([96, 64, 32, 0].map(|shift| (uuid >> shift) as i32).to_vec()),
        );
        put(
            &mut fields,
            "Motion",
            list(
                state
                    .entities
                    .vel(slot)
                    .map(|v| Tag::Double(v / 20.0))
                    .to_vec(),
                tag_type::DOUBLE,
            ),
        );
        put(
            &mut fields,
            "OnGround",
            Tag::Byte(i8::from(state.entities.ground(slot))),
        );
        if !fields.iter().any(|(key, _)| key == "EnderItems") {
            fields.push(("EnderItems".into(), list(Vec::new(), tag_type::COMPOUND)));
        }
        if let Some(stack) = player
            .inventory
            .slots
            .get(usize::from(player.inventory.selected))
            .filter(|stack| !stack.is_empty())
        {
            put(
                &mut fields,
                "SelectedItem",
                Tag::Compound(
                    mc_data::item_nbt::stack_to_fields(&state.registry.items, stack)
                        .map_err(|e| e.to_string())?,
                ),
            );
        }
        fields
    } else {
        let record =
            crate::core::entity_data_record(state, slot).ok_or("Entity has no vanilla data")?;
        mc_data::entity_nbt::entity_to_fields(&state.registry.items, &record)
            .map_err(|e| e.to_string())?
    };
    public_fields(&mut fields);
    // EntityDataAccessor uses NbtPredicate -> saveWithoutId, not a disk entity root.
    fields.retain(|(key, _)| key != "id");
    Ok(Tag::Compound(fields))
}

pub(crate) fn block(state: &TickState, pos: BlockPos) -> Result<Tag, String> {
    use mc_data::block_entity_kinds::BlockEntityKind;
    let chunk = state
        .world
        .chunk(ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        })
        .ok_or("That position is not loaded")?;
    let kind = state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .and_then(|block| BlockEntityKind::for_block(block.block.id.path()))
        .ok_or("The target block is not a block entity")?;
    let hive = if kind == BlockEntityKind::Beehive {
        crate::bees::loot_image(state, pos)
    } else {
        None
    };
    let tag = if let Some(bytes) = hive {
        Some(mc_nbt::read_network(&mut bytes.as_slice()).map_err(|e| e.to_string())?)
    } else {
        mc_world::anvil::chunk_nbt::block_entity_to_tag(
            chunk,
            pos,
            &state.registry.blocks,
            &state.registry.items,
        )
        .map_err(|e| e.to_string())?
    };
    let mut fields = match tag {
        Some(Tag::Compound(fields)) => fields,
        None => Vec::new(),
        _ => return Err("Invalid block entity NBT".into()),
    };
    for (key, value) in [
        ("id", Tag::String(kind.vanilla_id().into())),
        ("x", Tag::Int(pos.x)),
        ("y", Tag::Int(pos.y)),
        ("z", Tag::Int(pos.z)),
    ] {
        put(&mut fields, key, value);
    }
    let key = (pos.x, pos.y, pos.z);
    if let Some(live) = state.block_entities.get(&key) {
        if kind.inventory_size().is_some() && !fields.iter().any(|(key, _)| key == "LootTable") {
            let mut items = Vec::new();
            for (slot, stack) in live
                .slots
                .iter()
                .enumerate()
                .filter(|(_, stack)| !stack.is_empty())
            {
                let mut item = mc_data::item_nbt::stack_to_fields(&state.registry.items, stack)
                    .map_err(|e| e.to_string())?;
                item.push(("Slot".into(), Tag::Byte(slot as i8)));
                items.push(Tag::Compound(item));
            }
            put(&mut fields, "Items", list(items, tag_type::COMPOUND));
        }
        if let Some(furnace) = &live.furnace {
            for (key, value) in [
                ("lit_time_remaining", furnace.lit_remaining),
                ("lit_total_time", furnace.lit_total),
                ("cooking_time_spent", furnace.cook_progress),
                ("cooking_total_time", furnace.cook_total),
            ] {
                put(&mut fields, key, Tag::Short(value as i16));
            }
            put(
                &mut fields,
                "RecipesUsed",
                Tag::Compound(
                    furnace
                        .recipes_used
                        .iter()
                        .map(|(key, value)| (key.clone(), Tag::Int(*value)))
                        .collect(),
                ),
            );
        }
    }
    if let Some(stand) = state.brewing.get(&key) {
        put(&mut fields, "BrewTime", Tag::Short(stand.brew_time));
        put(&mut fields, "Fuel", Tag::Byte(stand.fuel as i8));
    }
    if let Some(jukebox) = state.jukeboxes.get(&key) {
        fields.retain(|(key, _)| key != "RecordItem" && key != "ticks_since_song_started");
        fields.extend(jukebox.data_fields(&state.registry.items));
    }
    public_fields(&mut fields);
    Ok(Tag::Compound(fields))
}

fn text(value: impl ToString) -> Tag {
    Tag::Compound(vec![("text".into(), Tag::String(value.to_string()))])
}

/// Returns the Brigadier result; errors remain failures even when observed by execute store.
pub(crate) fn query(
    state: &mut TickState,
    context: Context,
    mut input: &str,
) -> Result<i32, String> {
    if context.permission < 2 {
        return Err(crate::commands::CommandError::Unknown.message());
    }
    if argument(&mut input)? != "data" || argument(&mut input)? != "get" {
        return Err("Expected data get entity|block".into());
    }
    let kind = argument(&mut input)?;
    let (root, args) = match kind {
        "entity" => {
            let target =
                crate::commands::target_of(argument(&mut input)?).map_err(|e| e.message())?;
            let multiple = match &target {
                crate::commands::Target::AllPlayers => true,
                crate::commands::Target::Selector(selector) => {
                    selector.limit.unwrap_or(
                        if matches!(
                            selector.kind,
                            crate::commands::SelectorKind::AllPlayers
                                | crate::commands::SelectorKind::Entities
                        ) {
                            usize::MAX
                        } else {
                            1
                        },
                    ) > 1
                }
                _ => false,
            };
            if multiple {
                return Err(
                    "Only one entity is allowed, but the provided selector allows more than one"
                        .into(),
                );
            }
            let selected = crate::dimensions::travel::context_targets(state, context, &target)?;
            if selected.len() != 1 {
                return Err(if selected.is_empty() {
                    "No entity was found"
                } else {
                    "Only one entity is allowed, but the provided selector allows more than one"
                }
                .into());
            }
            let id = selected[0];
            let (dimension, _) =
                crate::dimensions::travel::locate(state, id).ok_or("No entity was found")?;
            let world = crate::dimensions::travel::world(state, dimension)
                .ok_or("Entity dimension unavailable")?;
            (
                entity(world, id)?,
                vec![crate::command_execution::entity_name(world, id)],
            )
        }
        "block" => {
            let coords = [
                argument(&mut input)?,
                argument(&mut input)?,
                argument(&mut input)?,
            ];
            let coords = crate::commands::coords(&coords).ok_or("Invalid block coordinates")?;
            let position = crate::commands::resolve_coords(
                coords,
                context.coordinate_origin(coords[0].local),
                context.rotation,
            );
            if position
                .iter()
                .any(|v| !v.is_finite() || *v < i32::MIN as f64 || *v > i32::MAX as f64)
            {
                return Err("Invalid block coordinates".into());
            }
            let [x, y, z] = position.map(|v| v.floor() as i32);
            let world = crate::dimensions::travel::world(state, context.dimension)
                .ok_or("Unknown dimension")?;
            (
                block(world, BlockPos { x, y, z })?,
                vec![text(x), text(y), text(z)],
            )
        }
        _ => return Err("Expected data get entity|block".into()),
    };
    let mut args = args;
    let (key, value) = if input.trim().is_empty() {
        args.push(text(snbt(&root)));
        (format!("commands.data.{kind}.query"), 1)
    } else {
        let path = argument(&mut input)?;
        let value = get(&root, path)?;
        if input.trim().is_empty() {
            args.push(text(snbt(&value)));
            (format!("commands.data.{kind}.query"), result(&value))
        } else {
            let scale = argument(&mut input)?
                .parse::<f64>()
                .map_err(|_| "Invalid scale")?;
            if !scale.is_finite() || !input.trim().is_empty() {
                return Err("Invalid scale or trailing arguments".into());
            }
            let value = (number(&value)
                .ok_or_else(|| format!("Can't get {path}; only numeric tags are allowed"))?
                * scale)
                .floor() as i32;
            args.insert(0, text(path));
            args.push(text(format!("{scale:.2}")));
            args.push(text(value));
            (format!("commands.data.{kind}.get"), value)
        }
    };
    crate::command_execution::feedback(state, context.connection, &key, args);
    Ok(value)
}

pub(crate) fn command(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    input: &str,
) -> crate::command_execution::CommandResult {
    use crate::command_execution::CommandResult::{Failed, Success};
    if input.split_whitespace().nth(2) == Some("storage") {
        let context = state
            .command_context
            .current
            .unwrap_or_else(|| Context::source(state, connection, executor, 2));
        return crate::command_storage::command(state, context, input);
    }
    if input.split_whitespace().nth(1) == Some("modify") {
        state.push_player(
            connection,
            crate::boundary::PlayerChange::Chat {
                message: "Data modify block and entity is not supported".into(),
            },
        );
        return Failed;
    }
    if input.split_whitespace().nth(1) != Some("get") {
        return crate::command_blocks::data(state, connection, executor, input);
    }
    let context = state
        .command_context
        .current
        .unwrap_or_else(|| Context::source(state, connection, executor, 2));
    match query(state, context, input) {
        Ok(value) => Success(value),
        Err(message) => {
            state.push_player(connection, crate::boundary::PlayerChange::Chat { message });
            Failed
        }
    }
}

#[cfg(test)]
#[path = "command_data_tests.rs"]
mod tests;
