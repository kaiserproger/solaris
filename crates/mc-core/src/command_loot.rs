//! Operator loot generation against the native catalogs.
use crate::boundary::{ConnectionId, EntityId, PlayerChange};
use crate::command_item_replace::{self, Destination};
use crate::commands::{CommandError, Coord, Target};
use crate::tick::TickState;
use mc_data::{Identifier, ItemStack};
use mc_nbt::Tag;

#[derive(Debug, Clone, PartialEq)]
pub struct LootCommand {
    target: Output,
    source: Source,
}
#[derive(Debug, Clone, PartialEq)]
enum Output {
    Give(Target),
    Insert([Coord; 3]),
    Spawn([Coord; 3]),
    Replace(Destination, Option<usize>),
}
#[derive(Debug, Clone, PartialEq)]
enum Source {
    Table(Identifier),
    Kill(Target),
    Unsupported,
}
pub(crate) fn parse(args: &[&str]) -> Result<LootCommand, CommandError> {
    let (target, rest) = match args {
        ["give", target, rest @ ..] => (Output::Give(crate::commands::target_of(target)?), rest),
        ["insert", x, y, z, rest @ ..] => (
            Output::Insert(
                crate::commands::coords(&[x, y, z])
                    .ok_or(CommandError::Usage("loot insert <pos>"))?,
            ),
            rest,
        ),
        ["spawn", x, y, z, rest @ ..] => (
            Output::Spawn(
                crate::commands::coords(&[x, y, z])
                    .ok_or(CommandError::Usage("loot spawn <pos>"))?,
            ),
            rest,
        ),
        ["replace", rest @ ..] => {
            let (dest, used) = command_item_replace::destination(rest)?;
            let mut rest = &rest[used..];
            let count = rest.first().and_then(|s| s.parse::<usize>().ok());
            if count.is_some() {
                rest = &rest[1..];
            }
            (Output::Replace(dest, count), rest)
        }
        _ => return Err(CommandError::Usage("loot <target> <source>")),
    };
    let source = match rest {
        ["loot", table] => Source::Table(
            Identifier::parse(*table).map_err(|e| CommandError::Message(e.to_string()))?,
        ),
        ["kill", target] => Source::Kill(crate::commands::target_of(target)?),
        ["mine" | "fish", ..] => Source::Unsupported,
        _ => return Err(CommandError::Usage("loot <target> <source>")),
    };
    Ok(LootCommand { target, source })
}
fn generate(
    state: &TickState,
    executor: EntityId,
    source: &Source,
) -> Result<Vec<ItemStack>, String> {
    match source {
        Source::Unsupported => Err("Loot mine and fish contexts are not supported".into()),
        Source::Table(id) => {
            let table = state
                .registry
                .chest_loot
                .get(id)
                .ok_or_else(|| format!("Unknown or unsupported loot table '{id}'"))?;
            table
                .roll(
                    &mut mc_data::loot::chest_26_1_2::ChestRng::new(state.tick),
                    &state.registry.items,
                    0.0,
                )
                .map_err(|e| e.to_string())
        }
        Source::Kill(target) => {
            let entities = crate::command_execution::resolve_target(state, executor, target);
            let [entity] = entities.as_slice() else {
                return Err("Loot kill requires exactly one entity".into());
            };
            let slot = state.entities.slot_of(*entity).ok_or("Entity not found")?;
            let crate::entities::EntityKind::Mob { type_id } = state.entities.body(slot).kind
            else {
                return Err("Entity has no loot table".into());
            };
            let blueprint = state
                .registry
                .blueprint(type_id)
                .ok_or("Entity has no loot table")?;
            let table = blueprint
                .loot_table
                .as_ref()
                .ok_or("Entity has no loot table")?;
            let entity = crate::entity_loot::loot_entity(state, *entity)
                .ok_or("Entity has no loot context")?;
            let attacker = crate::entity_loot::loot_entity(state, executor);
            let credit = attacker.clone().and_then(|attacker| {
                mc_data::loot::entity_26_1_2::EntityLootPlayerAttribution::try_new(attacker).ok()
            });
            let attack = attacker.map_or(
                mc_data::loot::entity_26_1_2::EntityLootAttack::None,
                mc_data::loot::entity_26_1_2::EntityLootAttack::Direct,
            );
            let cause = mc_data::loot::entity_26_1_2::EntityDeathCause::new(
                Default::default(),
                attack,
                credit,
            );
            let pos = crate::command_console::pose(state, executor)
                .map(|p| p.0)
                .ok_or("No command position")?;
            Ok(crate::mob_loot::mob_drops(
                &state.registry,
                entity,
                table,
                pos,
                state.tick,
                None,
                cause,
            ))
        }
    }
}
fn offset(destination: &Destination, offset: usize) -> Option<Destination> {
    let (slot, block) = match destination {
        Destination::Block(_, slot) => (slot, true),
        Destination::Entity(_, slot) => (slot, false),
    };
    if offset == 0 {
        return Some(destination.clone());
    }
    let (prefix, index) = slot.rsplit_once('.')?;
    let index = index.parse::<usize>().ok()?.checked_add(offset)?;
    let name = format!("{prefix}.{index}");
    Some(match destination {
        Destination::Block(coords, _) if block => Destination::Block(*coords, name),
        Destination::Entity(target, _) => Destination::Entity(target.clone(), name),
        _ => return None,
    })
}
pub(crate) fn execute(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    command: &LootCommand,
) -> Result<i32, String> {
    let stacks = generate(state, executor, &command.source)?;
    let mut count = 0;
    match &command.target {
        Output::Give(target) => {
            let entities = crate::command_execution::player_targets(state, executor, target);
            if entities.is_empty() {
                return Err("No players found".into());
            }
            for entity in entities {
                let rules = state.registry.item_rules();
                let player = state.players.get_mut(&entity).ok_or("Player not found")?;
                for stack in &stacks {
                    let mut remaining = stack.clone();
                    player.inventory.add(&rules, &mut remaining);
                    if remaining.count != stack.count {
                        count += 1;
                    }
                }
                crate::projectiles::weapons::sync(state, entity);
            }
        }
        Output::Insert(coords) => {
            let pos = command_item_replace::position(state, executor, *coords)?;
            if !state.block_entities.contains_key(&(pos.x, pos.y, pos.z)) {
                return Err("Container not found".into());
            }
            for stack in &stacks {
                let maximum = state.registry.item_rules().max_stack(stack).min(99);
                let mut remaining = stack.count;
                let record = state
                    .block_entities
                    .get_mut(&(pos.x, pos.y, pos.z))
                    .expect("container checked");
                for current in &mut record.slots {
                    if remaining == 0 {
                        break;
                    }
                    if current.is_empty() {
                        let amount = remaining.min(maximum);
                        *current = stack.clone();
                        current.count = amount;
                        remaining -= amount;
                    } else if mc_data::inventory_semantics_26_1_2::can_stack(current, stack) {
                        let amount = remaining.min((maximum - current.count).max(0));
                        current.count += amount;
                        remaining -= amount;
                    }
                }
                if remaining != stack.count {
                    count += 1;
                }
            }
            if count > 0 {
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
            }
        }
        Output::Spawn(coords) => {
            let (origin, rotation) =
                crate::command_console::pose(state, executor).ok_or("No command position")?;
            let pos = crate::commands::resolve_coords(*coords, origin, rotation);
            for stack in &stacks {
                crate::tick::spawn_item_entity(state, pos, stack.clone());
                count += 1;
            }
        }
        Output::Replace(destination, limit) => {
            let length = limit.unwrap_or(stacks.len());
            for i in 0..length {
                let Some(dest) = offset(destination, i) else {
                    break;
                };
                let empty = ItemStack::EMPTY;
                let stack = stacks.get(i).unwrap_or(&empty);
                count += command_item_replace::write_slots(state, executor, &dest, stack)?;
            }
        }
    }
    state.push_player(
        connection,
        PlayerChange::StyledChat {
            content: Tag::Compound(vec![(
                "text".into(),
                Tag::String(format!("Distributed {count} loot stack(s)")),
            )]),
        },
    );
    Ok(count)
}
