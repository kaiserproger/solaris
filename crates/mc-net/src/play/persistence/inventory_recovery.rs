use std::path::Path;

use mc_data::items::ItemRegistry;
use mc_nbt::Tag;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    CARRIED_ITEM_FIELD, CRAFTING_TABLE_INPUT_FIELD, ENCHANTING_TABLE_INPUT_FIELD,
    MERCHANT_INPUT_FIELD, PlayerPersistedState, PlayerPersistenceError, field, inventory_tag,
    item_stack_projection_tag, item_stack_tag, long_field, playerdata_path, read_player_root,
    set_field, write_player_root,
};
use crate::play::inventory::PlayerInventory;

pub(super) const INVENTORY_OPERATION_REVISION_FIELD: &str = "SolarisInventoryWorldJournalLsn";

/// Canonical named-item NBT, not registry-local ids or replayable item deltas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlayerInventoryRecovery {
    pub(crate) uuid: Uuid,
    inventory_nbt: Vec<u8>,
}

impl PlayerInventoryRecovery {
    pub(crate) fn capture(
        uuid: Uuid,
        state: &PlayerPersistedState,
        inventory: &PlayerInventory,
        items: &ItemRegistry,
    ) -> Result<Self, PlayerPersistenceError> {
        let inventory = Tag::Compound(
            snapshot_owned_fields(items, state, inventory)?
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value))
                .collect(),
        );
        let mut inventory_nbt = Vec::new();
        mc_nbt::write_named(&mut inventory_nbt, "", &inventory).map_err(|source| {
            PlayerPersistenceError::Nbt {
                path: std::path::PathBuf::from("inventory recovery intent"),
                source,
            }
        })?;
        Ok(Self {
            uuid,
            inventory_nbt,
        })
    }

    pub(crate) fn validate(&self) -> std::io::Result<()> {
        self.owned_fields().map(drop)
    }

    /// Live callers hold the server save coordinator across this read/replace.
    /// Startup recovery runs before player admission and periodic saves.
    pub(crate) fn recover(
        &self,
        world_root: &Path,
        revision: u64,
    ) -> Result<(), PlayerPersistenceError> {
        let path = playerdata_path(world_root, self.uuid);
        let (name, root) = if path.is_file() {
            read_player_root(&path)?
        } else {
            (String::new(), Tag::Compound(Vec::new()))
        };
        let Tag::Compound(mut fields) = root else {
            return Err(PlayerPersistenceError::RootNotCompound { path });
        };
        let watermark = operation_revision(&fields, &path)?;
        if watermark >= revision {
            return Ok(());
        }
        let inventory = self
            .owned_fields()
            .map_err(|source| PlayerPersistenceError::Io {
                path: path.clone(),
                source,
            })?;
        let revision =
            i64::try_from(revision).map_err(|_| PlayerPersistenceError::InvalidValue {
                path: path.clone(),
                field: INVENTORY_OPERATION_REVISION_FIELD,
            })?;
        for (key, value) in inventory {
            if let Some((_, current)) = fields.iter_mut().find(|(name, _)| name == &key) {
                *current = value;
            } else {
                fields.push((key, value));
            }
        }
        set_field(
            &mut fields,
            INVENTORY_OPERATION_REVISION_FIELD,
            Tag::Long(revision),
        );
        write_player_root(&path, &name, &Tag::Compound(fields))
    }

    fn owned_fields(&self) -> std::io::Result<Vec<(String, Tag)>> {
        let mut bytes = self.inventory_nbt.as_slice();
        let (name, root) = mc_nbt::read_named(&mut bytes)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let Tag::Compound(fields) = root else {
            return Err(invalid_inventory());
        };
        if !name.is_empty() || !bytes.is_empty() || fields.len() != 5 {
            return Err(invalid_inventory());
        }
        for (name, slots) in [
            ("Inventory", 46),
            (CRAFTING_TABLE_INPUT_FIELD, 9),
            (ENCHANTING_TABLE_INPUT_FIELD, 2),
            (MERCHANT_INPUT_FIELD, 2),
        ] {
            validate_slots(field(&fields, name).ok_or_else(invalid_inventory)?, slots)?;
        }
        let Some(Tag::Compound(carried)) = field(&fields, CARRIED_ITEM_FIELD) else {
            return Err(invalid_inventory());
        };
        if !carried.is_empty() {
            validate_stack(carried)?;
        }
        Ok(fields)
    }
}

pub(super) fn snapshot_owned_fields(
    items: &ItemRegistry,
    state: &PlayerPersistedState,
    inventory: &PlayerInventory,
) -> Result<[(&'static str, Tag); 5], PlayerPersistenceError> {
    Ok([
        ("Inventory", inventory_tag(items, state, inventory)?),
        (
            CARRIED_ITEM_FIELD,
            item_stack_tag(items, &state.carried_item)?,
        ),
        (
            CRAFTING_TABLE_INPUT_FIELD,
            item_stack_projection_tag(items, state.crafting_table_input.as_deref())?,
        ),
        (
            ENCHANTING_TABLE_INPUT_FIELD,
            item_stack_projection_tag(items, state.enchanting_table_input.as_deref())?,
        ),
        (
            MERCHANT_INPUT_FIELD,
            item_stack_projection_tag(items, state.merchant_input.as_deref())?,
        ),
    ])
}

fn validate_slots(tag: &Tag, maximum: usize) -> std::io::Result<()> {
    let Tag::List(list) = tag else {
        return Err(invalid_inventory());
    };
    if list.elements.len() > maximum {
        return Err(invalid_inventory());
    }
    let mut slots = [false; 46];
    for item in &list.elements {
        let Tag::Compound(fields) = item else {
            return Err(invalid_inventory());
        };
        let Some(slot) = super::slot_field(fields).filter(|slot| *slot < maximum) else {
            return Err(invalid_inventory());
        };
        if std::mem::replace(&mut slots[slot], true) {
            return Err(invalid_inventory());
        }
        validate_stack(fields)?;
    }
    Ok(())
}

fn validate_stack(fields: &[(String, Tag)]) -> std::io::Result<()> {
    if !matches!(super::int_field(fields, "count"), Some(1..))
        || super::string_field(fields, "id")
            .is_none_or(|id| mc_data::Identifier::parse(id.to_owned()).is_err())
    {
        return Err(invalid_inventory());
    }
    Ok(())
}

pub(super) fn operation_revision(
    fields: &[(String, Tag)],
    path: &Path,
) -> Result<u64, PlayerPersistenceError> {
    if field(fields, INVENTORY_OPERATION_REVISION_FIELD).is_none() {
        return Ok(0);
    }
    long_field(fields, INVENTORY_OPERATION_REVISION_FIELD)
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| PlayerPersistenceError::InvalidValue {
            path: path.to_owned(),
            field: INVENTORY_OPERATION_REVISION_FIELD,
        })
}

fn invalid_inventory() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "invalid inventory recovery intent",
    )
}

#[cfg(test)]
#[path = "inventory_recovery_tests.rs"]
mod tests;
