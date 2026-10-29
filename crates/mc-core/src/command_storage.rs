//! World-global command storage, retained as vanilla NBT compounds.
use crate::command_context::Context;
use crate::command_execution::CommandResult;
use crate::tick::TickState;
use mc_nbt::Tag;
use std::collections::BTreeMap;

pub(crate) type Storage = BTreeMap<String, Tag>;

pub(crate) fn to_tag(storage: &Storage) -> Tag {
    Tag::Compound(
        storage
            .iter()
            .map(|(name, tag)| (name.clone(), tag.clone()))
            .collect(),
    )
}
pub(crate) fn from_tag(tag: Tag) -> Result<Storage, String> {
    let Tag::Compound(fields) = tag else {
        return Err("Command storage must be a compound".into());
    };
    let mut result = Storage::new();
    for (name, value) in fields {
        if !matches!(value, Tag::Compound(_))
            || mc_data::Identifier::parse(&name).is_err()
            || result.insert(name, value).is_some()
        {
            return Err("Invalid command storage entry".into());
        }
    }
    Ok(result)
}
pub(crate) fn encode(storage: &Storage) -> Vec<u8> {
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", &to_tag(storage)).expect("valid storage NBT");
    bytes
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Storage, String> {
    let mut input = bytes;
    let (_, tag) = mc_nbt::read_named(&mut input).map_err(|e| e.to_string())?;
    if !input.is_empty() {
        return Err("Trailing command storage bytes".into());
    }
    from_tag(tag)
}
fn id(input: &mut &str) -> Result<String, String> {
    let name = crate::command_nbt_path::argument(input)?;
    Ok(mc_data::Identifier::parse(name)
        .map_err(|e| e.to_string())?
        .as_str()
        .to_owned())
}
fn source(state: &TickState, input: &mut &str) -> Result<Tag, String> {
    if crate::command_nbt_path::argument(input)? != "storage" {
        return Err("Only storage sources are supported".into());
    }
    let name = id(input)?;
    let empty = Tag::Compound(Vec::new());
    let root = state.command_storage.get(&name).unwrap_or(&empty);
    if input.trim().is_empty() {
        Ok(root.clone())
    } else {
        let path = crate::command_nbt_path::argument(input)?;
        Ok(crate::command_nbt_path::get(root, path)?.into_owned())
    }
}
pub(crate) fn command(state: &mut TickState, context: Context, input: &str) -> CommandResult {
    match run(state, input) {
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
fn run(state: &mut TickState, mut input: &str) -> Result<(i32, String), String> {
    let word = crate::command_nbt_path::argument;
    if word(&mut input)? != "data" {
        return Err("Expected data".into());
    }
    let action = word(&mut input)?;
    if word(&mut input)? != "storage" {
        return Err("Only storage data editing is supported".into());
    }
    let name = id(&mut input)?;
    let mut root = state
        .command_storage
        .get(&name)
        .cloned()
        .unwrap_or_else(|| Tag::Compound(Vec::new()));
    if action == "get" {
        let (result, message) = if input.trim().is_empty() {
            (1, crate::command_nbt_path::snbt(&root))
        } else {
            let path = word(&mut input)?;
            let value = crate::command_nbt_path::get(&root, path)?;
            if input.trim().is_empty() {
                (
                    crate::command_nbt_path::result(&value),
                    crate::command_nbt_path::snbt(&value),
                )
            } else {
                let scale: f64 = word(&mut input)?.parse().map_err(|_| "Invalid scale")?;
                if !scale.is_finite() {
                    return Err("Invalid scale".into());
                }
                let result = (crate::command_nbt_path::number(&value)
                    .ok_or("Expected numeric NBT")?
                    * scale)
                    .floor() as i32;
                (result, result.to_string())
            }
        };
        if !input.trim().is_empty() {
            return Err("Trailing arguments".into());
        }
        return Ok((result, message));
    }
    let previous = root.clone();
    let count = match action {
        "merge" => {
            let patch = crate::command_snbt::parse(input.trim()).map_err(str::to_owned)?;
            let (Tag::Compound(fields), Tag::Compound(patch)) = (&mut root, patch) else {
                return Err("Expected compound".into());
            };
            fn merge(fields: &mut Vec<(String, Tag)>, patch: Vec<(String, Tag)>) {
                for (key, value) in patch {
                    if let Some((_, old)) = fields.iter_mut().find(|(name, _)| *name == key) {
                        if let (Tag::Compound(a), Tag::Compound(b)) = (&mut *old, &value) {
                            merge(a, b.clone());
                        } else {
                            *old = value;
                        }
                    } else {
                        fields.push((key, value));
                    }
                }
            }
            merge(fields, patch);
            i32::from(root != previous)
        }
        "remove" => {
            let path = word(&mut input)?;
            if !input.trim().is_empty() {
                return Err("Trailing arguments".into());
            }
            crate::command_nbt_path::modify(&mut root, path, "remove", None, 0)?
        }
        "modify" => {
            let path = word(&mut input)?;
            let operation = word(&mut input)?;
            if !matches!(operation, "set" | "merge" | "append" | "prepend" | "insert") {
                return Err("Invalid data operation".into());
            }
            let index = if operation == "insert" {
                word(&mut input)?
                    .parse()
                    .map_err(|_| "Invalid list index")?
            } else {
                0
            };
            let value = match word(&mut input)? {
                "value" => {
                    let (value, consumed) = crate::command_snbt::parse_prefix(input.trim_start())
                        .map_err(str::to_owned)?;
                    input = &input.trim_start()[consumed..];
                    value
                }
                "from" => source(state, &mut input)?,
                _ => return Err("Expected value or from".into()),
            };
            if !input.trim().is_empty() {
                return Err("Trailing arguments".into());
            }
            crate::command_nbt_path::modify(&mut root, path, operation, Some(&value), index)?
        }
        _ => return Err("Expected data get, merge, modify or remove".into()),
    };
    if count == 0 || root == previous {
        return Err("Data did not change".into());
    }
    state.command_storage.insert(name, root);
    state.command_storage_dirty = true;
    Ok((count, "Storage data updated".into()))
}
