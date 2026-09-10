mod results;

pub use results::{
    ScriptInventoryEnchantment, ScriptInventoryItem, ScriptInventoryReservationQuantity,
    ScriptInventoryReservationSnapshot, ScriptInventorySlot, ScriptOwnedInventoryResult,
    ScriptOwnedInventorySnapshot,
};

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    MAX_SCRIPT_WORLD_TIME, ScriptDtoError, check_contract_resource_id, validate_bounded_nonempty,
};

pub const MAX_OWNED_INVENTORY_TRANSFERS: usize = 16;
pub const MAX_INVENTORY_RESOURCE_TYPES: usize = 16;
pub const MAX_INVENTORY_WORK_PORTIONS: usize = 512;
pub const MAX_OWNED_INVENTORY_SLOTS: usize = 54;
const MAX_WAREHOUSE_HANDLE_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptInventoryEndpoint {
    PlayerInventory { player_id: u64 },
    Warehouse { handle: String },
}

impl ScriptInventoryEndpoint {
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::PlayerInventory { player_id } => validate_player(*player_id),
            Self::Warehouse { handle } => {
                validate_bounded_nonempty("warehouse handle", handle, MAX_WAREHOUSE_HANDLE_BYTES)
            }
        }
    }
}

/// Existing durable revision plus a derived canonical before-image hash.
/// Native inventory changes need not advance the plugin journal watermark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptInventoryFence {
    pub revision: u64,
    pub snapshot_hash: String,
}

impl ScriptInventoryFence {
    pub fn try_new(revision: u64, snapshot_hash: String) -> Result<Self, ScriptDtoError> {
        let value = Self {
            revision,
            snapshot_hash,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if self.revision > MAX_SCRIPT_WORLD_TIME
            || self.snapshot_hash.len() != 64
            || !self
                .snapshot_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptInventoryExpectedRevision {
    pub endpoint: ScriptInventoryEndpoint,
    pub fence: ScriptInventoryFence,
}

impl ScriptInventoryExpectedRevision {
    #[must_use]
    pub fn new(endpoint: ScriptInventoryEndpoint, fence: ScriptInventoryFence) -> Self {
        Self { endpoint, fence }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptOwnedItemTransfer {
    pub source: ScriptInventoryEndpoint,
    pub source_slot: u8,
    pub destination: ScriptInventoryEndpoint,
    pub destination_slot: u8,
    pub count: u32,
}

impl ScriptOwnedItemTransfer {
    #[must_use]
    pub fn new(
        source: ScriptInventoryEndpoint,
        source_slot: u8,
        destination: ScriptInventoryEndpoint,
        destination_slot: u8,
        count: u32,
    ) -> Self {
        Self {
            source,
            source_slot,
            destination,
            destination_slot,
            count,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptInventoryMaterial {
    pub resource_id: String,
    pub quantity: u64,
}

impl ScriptInventoryMaterial {
    #[must_use]
    pub fn new(resource_id: String, quantity: u64) -> Self {
        Self {
            resource_id,
            quantity,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptInventoryWorkPortion {
    pub work_units: u64,
    pub materials: Vec<ScriptInventoryMaterial>,
}

impl ScriptInventoryWorkPortion {
    #[must_use]
    pub fn new(work_units: u64, materials: Vec<ScriptInventoryMaterial>) -> Self {
        Self {
            work_units,
            materials,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptInventoryResourcePlan {
    pub portions: Vec<ScriptInventoryWorkPortion>,
}

impl ScriptInventoryResourcePlan {
    #[must_use]
    pub fn new(portions: Vec<ScriptInventoryWorkPortion>) -> Self {
        Self { portions }
    }

    pub fn canonicalize(&mut self) {
        for portion in &mut self.portions {
            portion
                .materials
                .sort_unstable_by(|left, right| left.resource_id.cmp(&right.resource_id));
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if self.portions.is_empty() || self.portions.len() > MAX_INVENTORY_WORK_PORTIONS {
            return Err(ScriptDtoError::InvalidBounds);
        }
        let mut resources = BTreeSet::new();
        for portion in &self.portions {
            if portion.work_units == 0
                || portion.work_units > MAX_SCRIPT_WORLD_TIME
                || portion.materials.len() > MAX_INVENTORY_RESOURCE_TYPES
            {
                return Err(ScriptDtoError::InvalidBounds);
            }
            let mut previous = None;
            for material in &portion.materials {
                check_contract_resource_id(&material.resource_id)?;
                if material.quantity == 0
                    || material.quantity > MAX_SCRIPT_WORLD_TIME
                    || previous.is_some_and(|value: &str| value >= material.resource_id.as_str())
                {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                previous = Some(material.resource_id.as_str());
                resources.insert(material.resource_id.as_str());
                if resources.len() > MAX_INVENTORY_RESOURCE_TYPES {
                    return Err(ScriptDtoError::InvalidBounds);
                }
            }
        }
        if resources.is_empty() {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptOwnedInventoryOperation {
    Query {
        endpoint: ScriptInventoryEndpoint,
        expected_revision: Option<u64>,
    },
    Transfer {
        operation_id: String,
        actor_id: u64,
        transfers: Vec<ScriptOwnedItemTransfer>,
        expected_revisions: Vec<ScriptInventoryExpectedRevision>,
    },
    Reserve {
        operation_id: String,
        endpoint: ScriptInventoryEndpoint,
        resource_plan: ScriptInventoryResourcePlan,
        expected_revision: ScriptInventoryFence,
    },
    ReservationStatus {
        reservation_ref: String,
    },
    Release {
        operation_id: String,
        reservation_ref: String,
        expected_revision: u64,
    },
}

impl ScriptOwnedInventoryOperation {
    pub fn operation_id(&self) -> Option<&str> {
        match self {
            Self::Transfer { operation_id, .. }
            | Self::Reserve { operation_id, .. }
            | Self::Release { operation_id, .. } => Some(operation_id),
            Self::Query { .. } | Self::ReservationStatus { .. } => None,
        }
    }

    pub fn canonicalize(&mut self) {
        match self {
            Self::Transfer {
                expected_revisions, ..
            } => {
                expected_revisions
                    .sort_unstable_by(|left, right| left.endpoint.cmp(&right.endpoint));
            }
            Self::Reserve { resource_plan, .. } => resource_plan.canonicalize(),
            _ => {}
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if let Some(operation_id) = self.operation_id() {
            crate::validate_script_id_value(operation_id)?;
        }
        match self {
            Self::Query {
                endpoint,
                expected_revision,
            } => {
                endpoint.validate()?;
                if expected_revision.is_some_and(|revision| revision > MAX_SCRIPT_WORLD_TIME) {
                    return Err(ScriptDtoError::InvalidBounds);
                }
            }
            Self::Transfer {
                actor_id,
                transfers,
                expected_revisions,
                ..
            } => {
                validate_player(*actor_id)?;
                if transfers.is_empty()
                    || transfers.len() > MAX_OWNED_INVENTORY_TRANSFERS
                    || expected_revisions.len() > MAX_OWNED_INVENTORY_TRANSFERS * 2
                {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                let mut endpoints = BTreeSet::new();
                for transfer in transfers {
                    transfer.source.validate()?;
                    transfer.destination.validate()?;
                    if transfer.count == 0
                        || transfer.count > i32::MAX as u32
                        || !valid_slot(&transfer.source, transfer.source_slot)
                        || !valid_slot(&transfer.destination, transfer.destination_slot)
                        || (transfer.source == transfer.destination
                            && transfer.source_slot == transfer.destination_slot)
                    {
                        return Err(ScriptDtoError::InvalidBounds);
                    }
                    endpoints.insert(&transfer.source);
                    endpoints.insert(&transfer.destination);
                }
                if endpoints.len() != expected_revisions.len() {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                for (endpoint, expected) in endpoints.iter().zip(expected_revisions) {
                    if **endpoint != expected.endpoint {
                        return Err(ScriptDtoError::InvalidBounds);
                    }
                    expected.fence.validate()?;
                }
            }
            Self::Reserve {
                endpoint,
                resource_plan,
                expected_revision,
                ..
            } => {
                endpoint.validate()?;
                resource_plan.validate()?;
                expected_revision.validate()?;
            }
            Self::ReservationStatus { reservation_ref } => {
                validate_reservation_ref(reservation_ref)?
            }
            Self::Release {
                reservation_ref,
                expected_revision,
                ..
            } => {
                validate_reservation_ref(reservation_ref)?;
                if *expected_revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
            }
        }
        Ok(())
    }
}

fn validate_player(player_id: u64) -> Result<(), ScriptDtoError> {
    if player_id == 0 || player_id > MAX_SCRIPT_WORLD_TIME {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

fn valid_slot(endpoint: &ScriptInventoryEndpoint, slot: u8) -> bool {
    match endpoint {
        ScriptInventoryEndpoint::PlayerInventory { .. } => (9..=44).contains(&slot),
        ScriptInventoryEndpoint::Warehouse { .. } => usize::from(slot) < MAX_OWNED_INVENTORY_SLOTS,
    }
}

fn validate_reservation_ref(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty("inventory reservation", value, 64)
}
