//! Inventory slot replacement for operator commands.
use crate::boundary::{BlockPos, ConnectionId, EntityId, PlayerChange};
use crate::commands::{CommandError, Coord, Target};
use crate::tick::TickState;
use mc_data::ItemStack;
use mc_nbt::Tag;

#[derive(Debug, Clone, PartialEq)]
pub struct ItemCommand {
    destination: Destination,
    source: Source,
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Destination {
    Entity(Target, String),
    Block([Coord; 3], String),
}
#[derive(Debug, Clone, PartialEq)]
enum Source {
    With(crate::command_items::ItemArgument, i32),
    From(Destination),
    Unsupported,
}
pub(crate) fn destination(args: &[&str]) -> Result<(Destination, usize), CommandError> {
    match args {
        ["entity", target, slot, ..] => Ok((
            Destination::Entity(crate::commands::target_of(target)?, (*slot).into()),
            3,
        )),
        ["block", x, y, z, slot, ..] => Ok((
            Destination::Block(
                crate::commands::coords(&[x, y, z])
                    .ok_or(CommandError::Usage("item block <pos> <slot>"))?,
                (*slot).into(),
            ),
            5,
        )),
        _ => Err(CommandError::Usage("item <destination>")),
    }
}
pub(crate) fn parse(args: &[&str]) -> Result<ItemCommand, CommandError> {
    let [action, rest @ ..] = args else {
        return Err(CommandError::Usage("item replace ..."));
    };
    if *action == "modify" {
        return Ok(ItemCommand {
            destination: Destination::Entity(Target::Executor, String::new()),
            source: Source::Unsupported,
        });
    }
    if *action != "replace" {
        return Err(CommandError::Usage("item replace ..."));
    }
    let (target, used) = destination(rest)?;
    let source = match &rest[used..] {
        ["with", item] => Source::With(crate::command_items::ItemArgument::parse(item)?, 1),
        ["with", item, count] => {
            let count = count
                .parse::<i32>()
                .ok()
                .filter(|n| (1..=99).contains(n))
                .ok_or_else(|| CommandError::Message("Invalid item count".into()))?;
            Source::With(crate::command_items::ItemArgument::parse(item)?, count)
        }
        ["from", rest @ ..] => {
            let (source, used) = destination(rest)?;
            if used != rest.len() {
                return Err(CommandError::Message(
                    "Item modifiers are not supported".into(),
                ));
            }
            Source::From(source)
        }
        _ => return Err(CommandError::Usage("item replace ...")),
    };
    Ok(ItemCommand {
        destination: target,
        source,
    })
}
pub(crate) fn position(
    state: &TickState,
    executor: EntityId,
    coords: [Coord; 3],
) -> Result<BlockPos, String> {
    let (origin, rotation) =
        crate::command_console::pose(state, executor).ok_or("No command position")?;
    let pos = crate::commands::resolve_coords(coords, origin, rotation);
    Ok(BlockPos {
        x: pos[0].floor() as i32,
        y: pos[1].floor() as i32,
        z: pos[2].floor() as i32,
    })
}
fn slot_index(name: &str, selected: usize, player: bool) -> Option<usize> {
    if let Some(n) = name.strip_prefix("container.") {
        return n.parse().ok();
    }
    if player
        && let Some(n) = name
            .strip_prefix("hotbar.")
            .and_then(|n| n.parse::<usize>().ok())
            .filter(|n| *n < 9)
    {
        return Some(n);
    }
    if player
        && let Some(n) = name
            .strip_prefix("inventory.")
            .and_then(|n| n.parse::<usize>().ok())
            .filter(|n| *n < 27)
    {
        return Some(n + 9);
    }
    match (name, player) {
        ("weapon" | "weapon.mainhand", true) => Some(selected),
        ("weapon.offhand", true) => Some(40),
        ("armor.feet", true) => Some(36),
        ("armor.legs", true) => Some(37),
        ("armor.chest", true) => Some(38),
        ("armor.head", true) => Some(39),
        ("weapon" | "weapon.mainhand", false) => Some(0),
        ("weapon.offhand", false) => Some(1),
        ("armor.feet", false) => Some(2),
        ("armor.legs", false) => Some(3),
        ("armor.chest", false) => Some(4),
        ("armor.head", false) => Some(5),
        _ => None,
    }
}
pub(crate) fn read_slot(
    state: &TickState,
    executor: EntityId,
    destination: &Destination,
) -> Result<ItemStack, String> {
    match destination {
        Destination::Block(coords, slot) => {
            let pos = position(state, executor, *coords)?;
            let index = slot_index(slot, 0, false).ok_or("Invalid slot")?;
            if !slot.starts_with("container.") {
                return Err("Invalid container slot".into());
            }
            state
                .block_entities
                .get(&(pos.x, pos.y, pos.z))
                .and_then(|b| b.slots.get(index))
                .cloned()
                .ok_or_else(|| "Container or slot not found".into())
        }
        Destination::Entity(target, slot) => {
            let entities = crate::command_execution::resolve_target(state, executor, target);
            if slot.starts_with("container.") {
                return Err("Invalid entity slot".into());
            }
            let entity = entities.first().ok_or("Source entity not found")?;
            if let Some(player) = state.players.get(entity) {
                let index = slot_index(slot, usize::from(player.inventory.selected.min(8)), true)
                    .ok_or("Invalid slot")?;
                return player
                    .inventory
                    .slots
                    .get(index)
                    .cloned()
                    .ok_or_else(|| "Invalid slot".into());
            }
            let entity_slot = state.entities.slot_of(*entity).ok_or("Entity not found")?;
            if !matches!(
                state.entities.body(entity_slot).kind,
                crate::entities::EntityKind::Mob { .. }
            ) {
                return Err("Entity has no equipment".into());
            }
            let index = slot_index(slot, 0, false)
                .filter(|n| *n < 6)
                .ok_or("Invalid slot")?;
            Ok(state.entities.row(entity_slot).appearance.equipment[index]
                .clone()
                .unwrap_or(ItemStack::EMPTY))
        }
    }
}
pub(crate) fn write_slots(
    state: &mut TickState,
    executor: EntityId,
    destination: &Destination,
    stack: &ItemStack,
) -> Result<i32, String> {
    match destination {
        Destination::Block(coords, slot) => {
            let pos = position(state, executor, *coords)?;
            if !slot.starts_with("container.") {
                return Err("Invalid container slot".into());
            }
            let index = slot_index(slot, 0, false).ok_or("Invalid slot")?;
            if crate::plugin_inventories::has_reservations_for_block(state, pos) {
                let mut candidate = state
                    .block_entities
                    .get(&(pos.x, pos.y, pos.z))
                    .ok_or("Container not found")?
                    .slots
                    .clone();
                *candidate
                    .get_mut(index)
                    .ok_or("Container or slot not found")? = stack.clone();
                if crate::plugin_inventories::below_block_floors(state, pos, &candidate) {
                    return Err("Item is reserved by a plugin".into());
                }
            }
            let current = state
                .block_entities
                .get_mut(&(pos.x, pos.y, pos.z))
                .and_then(|b| b.slots.get_mut(index))
                .ok_or("Container or slot not found")?;
            *current = stack.clone();
            if let Some(record) = state.block_entities.get(&(pos.x, pos.y, pos.z)) {
                crate::brewing::slots_replaced(
                    &mut state.brewing,
                    (pos.x, pos.y, pos.z),
                    &record.slots,
                );
            }
            crate::redstone::containers::inventory_changed(&mut state.redstone_containers, pos);
            state.world.mark_block_entity_dirty(pos);
            crate::tick::refresh_container_viewers(state, pos);
            Ok(1)
        }
        Destination::Entity(target, slot) => {
            let entities = crate::command_execution::resolve_target(state, executor, target);
            if slot.starts_with("container.") {
                return Err("Invalid entity slot".into());
            }
            let equipment = state
                .registry
                .item_facts
                .facts_for_stack(stack, &state.registry.items)
                .and_then(|facts| facts.equippable_slot.as_deref());
            let required = match slot.as_str() {
                "armor.feet" => Some("feet"),
                "armor.legs" => Some("legs"),
                "armor.chest" => Some("chest"),
                "armor.head" => Some("head"),
                _ => None,
            };
            if !stack.is_empty() && required.is_some() && equipment != required {
                return Err("Item cannot be placed in the specified armor slot".into());
            }
            let mut count = 0;
            for entity in entities {
                if let Some(player) = state.players.get(&entity) {
                    let Some(index) =
                        slot_index(slot, usize::from(player.inventory.selected.min(8)), true)
                    else {
                        continue;
                    };
                    let Some(current) = player.inventory.slots.get(index) else {
                        continue;
                    };
                    let removed = if mc_data::inventory_semantics_26_1_2::can_stack(current, stack)
                    {
                        current.count.saturating_sub(stack.count).max(0) as u32
                    } else {
                        current.count.max(0) as u32
                    };
                    if removed > 0
                        && crate::plugin_inventories::below_player_debit_floors(
                            state,
                            entity,
                            &[(index, removed)],
                            None,
                        )
                    {
                        continue;
                    }
                    state
                        .players
                        .get_mut(&entity)
                        .expect("resolved player")
                        .inventory
                        .slots[index] = stack.clone();
                    crate::projectiles::weapons::sync(state, entity);
                    count += 1;
                } else if let Some(entity_slot) = state.entities.slot_of(entity) {
                    if !matches!(
                        state.entities.body(entity_slot).kind,
                        crate::entities::EntityKind::Mob { .. }
                    ) {
                        continue;
                    }
                    let Some(index) = slot_index(slot, 0, false).filter(|n| *n < 6) else {
                        continue;
                    };
                    let row = state.entities.row_mut(entity_slot);
                    std::sync::Arc::make_mut(&mut row.appearance).equipment[index] =
                        (!stack.is_empty()).then(|| stack.clone());
                    crate::entity_loot::restore(state, entity_slot);
                    state.appearance_changed.push(entity);
                    count += 1;
                }
            }
            if count == 0 {
                Err("No entity has the specified slot".into())
            } else {
                Ok(count)
            }
        }
    }
}
pub(crate) fn execute(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    command: &ItemCommand,
) -> Result<i32, String> {
    let stack = match &command.source {
        Source::Unsupported => return Err("Item modifiers are not supported".into()),
        Source::From(source) => read_slot(state, executor, source)?,
        Source::With(item, count) => {
            let mut stack = item.stack_with(&state.registry.item_rules())?;
            if *count > state.registry.item_rules().max_stack(&stack) {
                return Err("Item count exceeds stack limit".into());
            }
            stack.count = *count;
            stack
        }
    };
    let count = write_slots(state, executor, &command.destination, &stack)?;
    state.push_player(
        connection,
        PlayerChange::StyledChat {
            content: Tag::Compound(vec![(
                "text".into(),
                Tag::String(format!("Replaced item in {count} slot(s)")),
            )]),
        },
    );
    Ok(count)
}
