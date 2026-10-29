//! Operator attribute commands over retained mob attribute state.
use crate::boundary::{ConnectionId, EntityId};
use crate::commands::{CommandError, Target};
use crate::entities::EntityKind;
use crate::entity_loot::{EntityAttribute, EntityAttributeModifier};
use crate::tick::TickState;
use mc_data::Identifier;
use mc_nbt::Tag;

#[derive(Debug, Clone, PartialEq)]
pub struct AttributeCommand {
    target: Target,
    attribute: Identifier,
    action: Action,
}
#[derive(Debug, Clone, PartialEq)]
enum Action {
    Get(f64),
    BaseGet(f64),
    BaseSet(f64),
    BaseReset,
    Add(Identifier, f64, String),
    Remove(Identifier),
    ModifierGet(Identifier, f64),
}
fn number(raw: &str) -> Result<f64, CommandError> {
    raw.parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
        .ok_or_else(|| CommandError::Message(format!("Invalid number '{raw}'")))
}
fn identifier(raw: &str) -> Result<Identifier, CommandError> {
    Identifier::parse(raw).map_err(|e| CommandError::Message(e.to_string()))
}
pub(crate) fn parse(args: &[&str]) -> Result<AttributeCommand, CommandError> {
    let [target, attribute, rest @ ..] = args else {
        return Err(CommandError::Usage("attribute <target> <attribute> ..."));
    };
    let action = match rest {
        ["get"] => Action::Get(1.0),
        ["get", scale] => Action::Get(number(scale)?),
        ["base", "get"] => Action::BaseGet(1.0),
        ["base", "get", scale] => Action::BaseGet(number(scale)?),
        ["base", "set", value] => Action::BaseSet(number(value)?),
        ["base", "reset"] => Action::BaseReset,
        ["modifier", "add", id, value, operation]
            if matches!(
                *operation,
                "add_value" | "add_multiplied_base" | "add_multiplied_total"
            ) =>
        {
            Action::Add(identifier(id)?, number(value)?, (*operation).into())
        }
        ["modifier", "remove", id] => Action::Remove(identifier(id)?),
        ["modifier", "value", "get", id] => Action::ModifierGet(identifier(id)?, 1.0),
        ["modifier", "value", "get", id, scale] => {
            Action::ModifierGet(identifier(id)?, number(scale)?)
        }
        _ => return Err(CommandError::Usage("attribute <target> <attribute> ...")),
    };
    Ok(AttributeCommand {
        target: crate::commands::target_of(target)?,
        attribute: identifier(attribute)?,
        action,
    })
}
pub(crate) fn execute(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    command: &AttributeCommand,
) -> Result<i32, String> {
    let targets = crate::command_execution::resolve_target(state, executor, &command.target);
    let [entity] = targets.as_slice() else {
        return Err("Attribute requires exactly one entity".into());
    };
    let slot = state.entities.slot_of(*entity).ok_or("Entity not found")?;
    let EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
        return Err("Attribute commands for players are not supported".into());
    };
    let blueprint = state
        .registry
        .blueprint(type_id)
        .ok_or("Entity has no attributes")?;
    let name = command
        .attribute
        .as_str()
        .strip_prefix("minecraft:")
        .ok_or("Unknown attribute")?;
    let default = match name {
        "max_health" => f64::from(blueprint.params.max_health),
        "movement_speed" => f64::from(blueprint.params.speed) / 10.0,
        "attack_damage" => f64::from(blueprint.params.attack_damage),
        "armor" => blueprint.params.armor,
        "armor_toughness" => 0.0,
        "follow_range" => f64::from(blueprint.sensors.sight_range),
        "knockback_resistance" => blueprint.params.knockback_resistance,
        _ => {
            return Err(format!(
                "Unknown or unsupported attribute '{}'",
                command.attribute
            ));
        }
    };
    let row = state.entities.row(slot);
    let existing = row
        .loot
        .attributes
        .iter()
        .find(|a| a.id == command.attribute.as_str());
    let base = existing.map_or(default, |a| a.base);
    let mut changed = false;
    let (value, scale, message) = match &command.action {
        Action::Get(scale) => (
            crate::entity_loot::effective_attribute(
                &state.registry.item_rules(),
                row,
                name,
                default,
            ),
            *scale,
            "value",
        ),
        Action::BaseGet(scale) => (base, *scale, "base value"),
        Action::ModifierGet(id, scale) => (
            existing
                .and_then(|a| a.modifiers.iter().find(|m| m.id == id.as_str()))
                .ok_or("Unknown attribute modifier")?
                .amount,
            *scale,
            "modifier value",
        ),
        action => {
            let attributes = &mut state.entities.row_mut(slot).loot.attributes;
            let index = attributes
                .iter()
                .position(|a| a.id == command.attribute.as_str());
            if let Action::Remove(id) = action {
                let attr = index
                    .and_then(|i| attributes.get_mut(i))
                    .ok_or("Unknown attribute modifier")?;
                let before = attr.modifiers.len();
                attr.modifiers.retain(|m| m.id != id.as_str());
                if before == attr.modifiers.len() {
                    return Err("Unknown attribute modifier".into());
                }
            } else {
                if let Action::Add(id, _, _) = action
                    && index.is_some_and(|i| {
                        attributes[i].modifiers.iter().any(|m| m.id == id.as_str())
                    })
                {
                    return Err("Attribute modifier already exists".into());
                }
                let index = index.unwrap_or_else(|| {
                    attributes.push(EntityAttribute {
                        id: command.attribute.to_string(),
                        base: default,
                        modifiers: Vec::new(),
                    });
                    attributes.len() - 1
                });
                let attr = &mut attributes[index];
                match action {
                    Action::BaseSet(value) => attr.base = *value,
                    Action::BaseReset => attr.base = default,
                    Action::Add(id, amount, operation) => {
                        attr.modifiers.push(EntityAttributeModifier {
                            id: id.to_string(),
                            amount: *amount,
                            operation: operation.clone(),
                        })
                    }
                    _ => unreachable!(),
                }
            }
            changed = true;
            (1.0, 1.0, "updated")
        }
    };
    if changed {
        crate::entity_loot::restore(state, slot);
        state.appearance_changed.push(*entity);
    }
    state.push_player(
        connection,
        crate::boundary::PlayerChange::StyledChat {
            content: Tag::Compound(vec![(
                "text".into(),
                Tag::String(format!(
                    "Attribute {} {message}: {value}",
                    command.attribute
                )),
            )]),
        },
    );
    Ok((value * scale) as i32)
}
