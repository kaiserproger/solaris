use mc_script::{
    ScriptInventoryReservationQuantity, ScriptInventoryReservationSnapshot, ScriptOperationFailure,
    ScriptOperationOutcome, ScriptOperationPayload, ScriptOwnedInventoryResult,
};

use super::PluginStorage;

/// The reservation keeps its durable projection; unknown or foreign references
/// answer `not_found` instead of a guessed empty reservation.
pub(super) fn reservation_status(
    storage: &PluginStorage,
    plugin_id: &str,
    reservation_ref: &str,
) -> ScriptOperationOutcome {
    let Some((revision, snapshot)) = storage.reservation(plugin_id, reservation_ref) else {
        return ScriptOperationOutcome::rejected(ScriptOperationFailure::NotFound);
    };
    reservation_outcome(revision, snapshot.clone())
}

/// Release returns every un-consumed unit of the reservation to the endpoint
/// and is serialized with any consumption because the storage actor owns both.
pub(super) fn release_payload(
    storage: &PluginStorage,
    plugin_id: &str,
    reservation_ref: &str,
    expected_revision: u64,
) -> Result<ScriptOperationPayload, ScriptOperationFailure> {
    let Some((revision, snapshot)) = storage.reservation(plugin_id, reservation_ref) else {
        return Err(ScriptOperationFailure::NotFound);
    };
    if revision != expected_revision || snapshot.released {
        return Err(ScriptOperationFailure::StaleRevision);
    }
    let quantities = snapshot
        .quantities
        .iter()
        .map(|quantity| {
            ScriptInventoryReservationQuantity::new(
                quantity.resource_id.clone(),
                quantity.reserved,
                quantity.consumed,
                quantity.returned.saturating_add(quantity.remaining),
                0,
            )
        })
        .collect();
    let released = ScriptInventoryReservationSnapshot::new(
        snapshot.reservation_ref.clone(),
        snapshot.endpoint.clone(),
        snapshot.resource_plan_hash.clone(),
        quantities,
        snapshot.bound_to.clone(),
        true,
        snapshot.receipt_watermark,
    );
    Ok(ScriptOperationPayload::OwnedInventory {
        result: Box::new(ScriptOwnedInventoryResult::Reservation {
            reservation: released,
        }),
    })
}

fn reservation_outcome(
    revision: u64,
    reservation: ScriptInventoryReservationSnapshot,
) -> ScriptOperationOutcome {
    ScriptOperationOutcome::committed(
        revision,
        ScriptOperationPayload::OwnedInventory {
            result: Box::new(ScriptOwnedInventoryResult::Reservation { reservation }),
        },
    )
    .expect("stored reservation projection is canonical")
}

/// Mint one opaque reservation reference that is not already held by the owner.
pub(super) fn fresh_reservation_ref(storage: &PluginStorage, plugin_id: &str) -> Option<String> {
    for _ in 0..8 {
        let reference = random_reference()?;
        if storage.reservation(plugin_id, &reference).is_none() {
            return Some(reference);
        }
    }
    None
}

fn random_reference() -> Option<String> {
    use rsa::rand_core::{OsRng, RngCore};
    let mut bytes = [0_u8; 16];
    OsRng.try_fill_bytes(&mut bytes).ok()?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut reference = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        reference.push(char::from(HEX[usize::from(byte >> 4)]));
        reference.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Some(reference)
}
