//! Scoreboard tags use the retained vanilla entity and player NBT owners.
use crate::command_context::Context;
use crate::command_execution::CommandResult;
use crate::tick::TickState;
use mc_nbt::{ListTag, Tag, tag_type};

pub(crate) fn execute(state: &mut TickState, context: Context, input: &str) -> CommandResult {
    let result = run(state, context, input);
    match result {
        Ok((count, message)) => {
            state.push_player(
                context.connection,
                crate::boundary::PlayerChange::Chat { message },
            );
            CommandResult::Success(count)
        }
        Err(message) => {
            state.push_player(
                context.connection,
                crate::boundary::PlayerChange::Chat { message },
            );
            CommandResult::Failed
        }
    }
}

fn run(state: &mut TickState, context: Context, input: &str) -> Result<(i32, String), String> {
    let mut input = input;
    let word = crate::command_nbt_path::argument;
    if word(&mut input)? != "tag" {
        return Err("Expected tag command".into());
    }
    let target = crate::commands::target_of(word(&mut input)?).map_err(|e| e.message())?;
    let action = word(&mut input)?;
    let name = if action == "list" {
        None
    } else {
        Some(word(&mut input)?)
    };
    if !input.trim().is_empty() || !matches!(action, "add" | "remove" | "list") {
        return Err("Expected tag <targets> add|remove <name> or list".into());
    }
    let targets = crate::dimensions::travel::context_targets(state, context, &target)?;
    if targets.is_empty() {
        return Err("No entities were found".into());
    }
    let mut count = 0;
    let mut listed = std::collections::BTreeSet::new();
    for entity in targets {
        let (dimension, slot) =
            crate::dimensions::travel::locate(state, entity).ok_or("No entity was found")?;
        let world = if dimension == state.dimension {
            &mut *state
        } else {
            state
                .other_dimensions
                .get_mut(&dimension)
                .ok_or("Unknown dimension")?
        };
        let bytes = if let Some(player) = world.players.get_mut(&entity) {
            &mut player.opaque_player_fields
        } else {
            &mut world.entities.row_mut(slot).opaque_entity_fields
        };
        let mut fields = if bytes.is_empty() {
            Vec::new()
        } else {
            match mc_nbt::read_named(&mut bytes.as_slice())
                .map_err(|e| e.to_string())?
                .1
            {
                Tag::Compound(fields) => fields,
                _ => return Err("Invalid retained entity data".into()),
            }
        };
        let index = fields.iter().position(|(key, _)| key == "Tags");
        let mut tags = match index.map(|i| &fields[i].1) {
            None => Vec::new(),
            Some(Tag::List(list))
                if list.element_type == tag_type::STRING || list.elements.is_empty() =>
            {
                list.elements.clone()
            }
            _ => return Err("Invalid entity tags".into()),
        };
        if action == "list" {
            for tag in tags {
                if let Tag::String(tag) = tag {
                    listed.insert(tag);
                }
            }
            continue;
        }
        let name = name.expect("name for add/remove");
        let present = tags
            .iter()
            .position(|tag| matches!(tag, Tag::String(value) if value == name));
        match (action, present) {
            ("add", None) if tags.len() < 1024 => tags.push(Tag::String(name.into())),
            ("remove", Some(index)) => {
                tags.remove(index);
            }
            _ => continue,
        }
        let value = Tag::List(ListTag {
            element_type: tag_type::STRING,
            elements: tags,
        });
        if let Some(index) = index {
            fields[index].1 = value;
        } else {
            fields.push(("Tags".into(), value));
        }
        let mut encoded = Vec::new();
        mc_nbt::write_named(&mut encoded, "", &Tag::Compound(fields)).map_err(|e| e.to_string())?;
        *bytes = encoded;
        count += 1;
    }
    if action == "list" {
        return Ok((
            listed.len() as i32,
            format!(
                "Tags: {}",
                listed.into_iter().collect::<Vec<_>>().join(", ")
            ),
        ));
    }
    if count == 0 {
        return Err(format!("No entity tags were {action}ed"));
    }
    Ok((count, format!("Updated tags on {count} entities")))
}
