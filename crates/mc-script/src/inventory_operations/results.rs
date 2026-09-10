use serde::{Deserialize, Serialize};

use super::{
    MAX_INVENTORY_RESOURCE_TYPES, MAX_OWNED_INVENTORY_SLOTS, MAX_OWNED_INVENTORY_TRANSFERS,
    ScriptInventoryEndpoint, ScriptInventoryExpectedRevision, ScriptInventoryFence, valid_slot,
};
use crate::{MAX_SCRIPT_WORLD_TIME, ScriptDtoError, check_contract_resource_id};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptInventoryEnchantment {
    pub resource_id: String,
    pub level: i32,
}

impl ScriptInventoryEnchantment {
    #[must_use]
    pub fn new(resource_id: String, level: i32) -> Self {
        Self { resource_id, level }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptInventoryItem {
    pub resource_id: String,
    pub count: u32,
    pub damage: Option<i32>,
    pub enchantments: Vec<ScriptInventoryEnchantment>,
    pub custom_name: Option<String>,
    pub item_model: Option<String>,
}

impl ScriptInventoryItem {
    #[must_use]
    pub fn new(
        resource_id: String,
        count: u32,
        damage: Option<i32>,
        enchantments: Vec<ScriptInventoryEnchantment>,
        custom_name: Option<String>,
        item_model: Option<String>,
    ) -> Self {
        Self {
            resource_id,
            count,
            damage,
            enchantments,
            custom_name,
            item_model,
        }
    }

    fn validate(&self) -> Result<(), ScriptDtoError> {
        check_contract_resource_id(&self.resource_id)?;
        if self.count == 0 || self.count > i32::MAX as u32 {
            return Err(ScriptDtoError::InvalidBounds);
        }
        for enchantment in &self.enchantments {
            check_contract_resource_id(&enchantment.resource_id)?;
        }
        if let Some(model) = &self.item_model {
            check_contract_resource_id(model)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptInventorySlot {
    pub slot: u8,
    pub item: Option<ScriptInventoryItem>,
}

impl ScriptInventorySlot {
    #[must_use]
    pub fn new(slot: u8, item: Option<ScriptInventoryItem>) -> Self {
        Self { slot, item }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptOwnedInventorySnapshot {
    pub endpoint: ScriptInventoryEndpoint,
    pub fence: ScriptInventoryFence,
    pub slots: Vec<ScriptInventorySlot>,
}

impl ScriptOwnedInventorySnapshot {
    #[must_use]
    pub fn new(
        endpoint: ScriptInventoryEndpoint,
        fence: ScriptInventoryFence,
        slots: Vec<ScriptInventorySlot>,
    ) -> Self {
        Self {
            endpoint,
            fence,
            slots,
        }
    }

    fn validate(&self) -> Result<(), ScriptDtoError> {
        self.endpoint.validate()?;
        self.fence.validate()?;
        if self.slots.len() > MAX_OWNED_INVENTORY_SLOTS {
            return Err(ScriptDtoError::InvalidBounds);
        }
        let mut occupied = 0_u64;
        for slot in &self.slots {
            if !valid_slot(&self.endpoint, slot.slot) || occupied & (1_u64 << slot.slot) != 0 {
                return Err(ScriptDtoError::InvalidBounds);
            }
            occupied |= 1_u64 << slot.slot;
            if let Some(item) = &slot.item {
                item.validate()?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptInventoryReservationQuantity {
    pub resource_id: String,
    pub reserved: u64,
    pub consumed: u64,
    pub returned: u64,
    pub remaining: u64,
}

impl ScriptInventoryReservationQuantity {
    #[must_use]
    pub fn new(
        resource_id: String,
        reserved: u64,
        consumed: u64,
        returned: u64,
        remaining: u64,
    ) -> Self {
        Self {
            resource_id,
            reserved,
            consumed,
            returned,
            remaining,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptInventoryReservationSnapshot {
    pub reservation_ref: String,
    pub resource_plan_hash: String,
    pub quantities: Vec<ScriptInventoryReservationQuantity>,
    pub bound_to: Option<String>,
    pub released: bool,
    pub receipt_watermark: u64,
}

impl ScriptInventoryReservationSnapshot {
    #[must_use]
    pub fn new(
        reservation_ref: String,
        resource_plan_hash: String,
        quantities: Vec<ScriptInventoryReservationQuantity>,
        bound_to: Option<String>,
        released: bool,
        receipt_watermark: u64,
    ) -> Self {
        Self {
            reservation_ref,
            resource_plan_hash,
            quantities,
            bound_to,
            released,
            receipt_watermark,
        }
    }

    fn validate(&self) -> Result<(), ScriptDtoError> {
        super::validate_reservation_ref(&self.reservation_ref)?;
        if self.resource_plan_hash.len() != 64
            || !self
                .resource_plan_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || self.receipt_watermark > MAX_SCRIPT_WORLD_TIME
            || self.quantities.len() > MAX_INVENTORY_RESOURCE_TYPES
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        let mut previous = None;
        for quantity in &self.quantities {
            check_contract_resource_id(&quantity.resource_id)?;
            if quantity.reserved > MAX_SCRIPT_WORLD_TIME
                || quantity
                    .consumed
                    .checked_add(quantity.returned)
                    .and_then(|total| total.checked_add(quantity.remaining))
                    != Some(quantity.reserved)
                || (self.released && quantity.remaining != 0)
                || previous.is_some_and(|value: &str| value >= quantity.resource_id.as_str())
            {
                return Err(ScriptDtoError::InvalidBounds);
            }
            previous = Some(quantity.resource_id.as_str());
        }
        if let Some(binding) = &self.bound_to {
            crate::validate_bounded_nonempty("inventory reservation binding", binding, 128)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptOwnedInventoryResult {
    Snapshot {
        inventory: ScriptOwnedInventorySnapshot,
    },
    Transfer {
        inventories: Vec<ScriptInventoryExpectedRevision>,
    },
    Reservation {
        reservation: ScriptInventoryReservationSnapshot,
    },
}

impl ScriptOwnedInventoryResult {
    pub(crate) fn validate(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::Snapshot { inventory } => inventory.validate(),
            Self::Transfer { inventories } => {
                if inventories.is_empty() || inventories.len() > MAX_OWNED_INVENTORY_TRANSFERS * 2 {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                let mut previous = None;
                for inventory in inventories {
                    inventory.endpoint.validate()?;
                    inventory.fence.validate()?;
                    if previous.is_some_and(|value| value >= &inventory.endpoint) {
                        return Err(ScriptDtoError::InvalidBounds);
                    }
                    previous = Some(&inventory.endpoint);
                }
                Ok(())
            }
            Self::Reservation { reservation } => reservation.validate(),
        }
    }
}
