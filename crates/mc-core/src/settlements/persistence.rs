//! Settlement persistence boundary (D006 S1): per-record native rows,
//! touched-only WAL images, hydration validation, legacy refusal.
//!
//! Rows: one `settlement/<id>` native global per live record (canonical
//! nonzero decimal id, no leading zeros). No aggregate registry blob: each
//! row carries one dimension-tagged [`Row`] JSON guarded by the native 16MiB
//! envelope. Tombstones delete the row at the next checkpoint; the WAL
//! carries the deletion to crash recovery in the meantime.
//!
//! WAL: [`JournalEntry::Settlement`](crate::journal::JournalEntry::Settlement)
//! under [`JournalEntry::Dimension`](crate::journal::JournalEntry::Dimension)
//! (every dimension including Overworld), touched-only per dimension via
//! [`Registry::take_images`](crate::settlements::Registry::take_images), with
//! that registry's high water on every image. Sequence stays globally
//! monotonic: the root sequence is the max over the checkpoint row, every
//! dimension registry/WAL high water, and every live id; ids are never reused
//! after removal. Registries are per-dimension authorities; only the sequence
//! and budget swap between dimension ticks.
//!
//! Membership is journaled and checkpointed exclusively through the villager
//! `settlement` column (entity NBT/WAL); derived counts rebuild from live
//! columns after hydration and are never persisted.

use std::collections::BTreeMap;

use crate::config::CoreError;
use crate::journal::{JournalEntry, Replay};
use crate::settlements::{RecordImage, SettlementId, SettlementRecord, validate_record};
use crate::tick::TickState;
use mc_world::native::NativeDomainRecord;

/// Native global key for one live settlement record (one row per global id).
pub(crate) fn row_key(id: SettlementId) -> String {
    format!("settlement/{}", id.0)
}

/// Durable per-record row: the home dimension alongside the record. One row
/// per global id, keyed canonically; never an aggregate registry blob.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Row<T = SettlementRecord> {
    pub dimension: crate::dimensions::Dimension,
    pub value: T,
}

/// Parse a canonical `settlement/<u64>` key: nonzero, no leading zeros.
pub(crate) fn parse_row_key(key: &str) -> Option<SettlementId> {
    let suffix = key.strip_prefix("settlement/")?;
    if suffix.is_empty() || suffix.len() > 20 {
        return None;
    }
    if suffix.len() > 1 && suffix.starts_with('0') {
        return None;
    }
    if !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let id: u64 = suffix.parse().ok()?;
    if id == 0 {
        return None;
    }
    Some(SettlementId(id))
}

/// Encode every live record from every dimension as one native row each,
/// plus the shared sequence row carrying the global max. Deleted records emit
/// no row: the tombstone WAL image covers the gap until the checkpoint
/// commits, and recovery treats a missing row as removed.
pub(crate) fn checkpoint_rows(
    state: &TickState,
    tick: u64,
) -> Result<BTreeMap<String, NativeDomainRecord>, CoreError> {
    let mut rows = BTreeMap::new();
    for world in std::iter::once(state).chain(state.other_dimensions.values().map(|world| &**world))
    {
        for record in world.settlements.records() {
            let row = Row {
                dimension: world.dimension,
                value: record,
            };
            let payload =
                serde_json::to_vec(&row).map_err(|error| CoreError::Storage(error.to_string()))?;
            rows.insert(
                row_key(record.id),
                crate::native_checkpoint::record(tick, payload),
            );
        }
    }
    let sequence = std::iter::once(state)
        .chain(state.other_dimensions.values().map(|world| &**world))
        .map(|world| {
            world
                .settlement_sequence
                .max(world.settlements.high_water())
        })
        .max()
        .unwrap_or(0);
    rows.insert(
        "settlement_sequence".into(),
        crate::native_checkpoint::record(
            tick,
            serde_json::to_vec(&sequence).map_err(|error| CoreError::Storage(error.to_string()))?,
        ),
    );
    Ok(rows)
}

/// Drain this dimension registry's touched record images as raw WAL entries.
/// Every dimension drains its own registry; the caller wraps each dimension's
/// images under its [`JournalEntry::Dimension`] envelope (Overworld included).
pub(crate) fn journal_entries(state: &mut TickState, tick: u64, entries: &mut Vec<JournalEntry>) {
    let high_water = state.settlements.high_water();
    for image in state.settlements.take_images() {
        entries.push(JournalEntry::Settlement {
            image: Box::new(image),
            high_water,
            tick,
        });
    }
}

/// Hydrate one dimension's registry from the global checkpoint rows folded
/// with its dimension-routed WAL tail. Decodes and validates every row before
/// applying it; applies only rows homed to this dimension. A skipped record
/// would otherwise become permanent loss at the next checkpoint rewrite.
pub(crate) fn hydrate_dimension(
    state: &mut TickState,
    checkpoint: &BTreeMap<String, NativeDomainRecord>,
    replay: &Replay,
    sequence: u64,
) -> Result<(), CoreError> {
    let invalid = |reason: String| CoreError::Storage(format!("native settlement: {reason}"));
    for (key, record) in checkpoint {
        let Some(id) = parse_row_key(key) else {
            continue;
        };
        if record.version != 1 {
            return Err(invalid(format!("row {key} has unsupported version")));
        }
        let row: Row = serde_json::from_slice(&record.payload)
            .map_err(|error| invalid(format!("row {key} unreadable: {error}")))?;
        if row.value.id != id {
            return Err(invalid(format!("row {key} identity mismatch")));
        }
        validate_record(&row.value).map_err(|error| invalid(format!("row {key}: {error}")))?;
        super::construction::validate_pending(&state.registry, &row.value)
            .map_err(|error| invalid(format!("row {key}: {error}")))?;
        if row.dimension != state.dimension {
            continue;
        }
        state.settlements.apply_image(RecordImage {
            id,
            value: Some(row.value),
        });
    }
    // WAL tail live images converge over rows; tombstones delete rows below.
    // Duplicate-name validation runs in [`finish_recovery`] once rows, images,
    // and tombstones have converged.
    for (&id, record) in &replay.settlements {
        let image = RecordImage {
            id: SettlementId(id),
            value: Some(record.clone()),
        };
        if let Some(value) = &image.value {
            if value.id.0 != id {
                return Err(invalid(format!("WAL image identity mismatch for id {id}")));
            }
            validate_record(value)
                .map_err(|error| invalid(format!("WAL image id {id}: {error}")))?;
            super::construction::validate_pending(&state.registry, value)
                .map_err(|error| invalid(format!("WAL image id {id}: {error}")))?;
        }
        state.settlements.apply_image(image);
    }
    // Tombstones are row deletions: a checkpoint row the tail removed stays
    // removed until a later live image recreates the id (which never happens:
    // ids are never reused, so any live image here is strictly newer state).
    for id in &replay.settlement_removed {
        if !replay.settlements.contains_key(id) {
            state.settlements.apply_image(RecordImage {
                id: SettlementId(*id),
                value: None,
            });
        }
    }
    state.settlements.set_high_water(sequence);
    state
        .settlements
        .set_high_water(replay.settlement_high_water);
    // Per-dimension floor; the global max is set in [`finish_recovery`] once
    // every dimension has hydrated (monotonic, never reused across dims).
    let water = state.settlements.high_water().max(sequence);
    state.settlements.set_high_water(water);
    state.settlement_sequence = state.settlement_sequence.max(water);
    // Drained: hydration images must not re-enter the next tick's WAL.
    let _ = state.settlements.take_images();
    Ok(())
}

/// Decode the durable allocator sequence row. Tombstones and live images' settlement tombstones
/// ride the WAL tail (Replay settlements / settlement_removed).
pub(crate) fn checkpoint_sequence(
    checkpoint: &BTreeMap<String, NativeDomainRecord>,
    tick: u64,
) -> Result<u64, CoreError> {
    let record = checkpoint
        .get("settlement_sequence")
        .ok_or_else(|| CoreError::Storage("native settlement: missing sequence row".into()))?;
    if record.version != 1 || record.captured_tick != tick {
        return Err(CoreError::Storage(
            "native settlement: invalid sequence row header".into(),
        ));
    }
    serde_json::from_slice(&record.payload)
        .map_err(|error| CoreError::Storage(format!("native settlement: {error}")))
}

/// Refuse legacy `solaris-settlements` plugin state instead of silently
/// wiping it. Inspects the real keys the package persists through native
/// plugin storage (`settlement-root-v2`, `settlement-registry-v1`,
/// `settlement-v2/*/head`, `settlement-fragment-v2/*`) and its
/// settlement-associated storage families
/// (`settlement-trade-v1/*`, `player-contract-v1/*`, `paid-shipment-v1/*`,
/// `paid-blob-v1/*`, `production-root-v2`, `production-journal-v1`,
/// `production-v2/*`). NPC custody refuses only settlement-owned rows (see
/// below); unrelated NPC consumers stay admitted.
pub(crate) fn refuse_legacy_plugin_state(state: &TickState) -> Result<(), CoreError> {
    const OWNER: &str = "solaris-settlements";
    let refused = |detail: &str| {
        CoreError::Storage(format!(
            "legacy {OWNER} plugin state present ({detail}); \
             native settlements require explicit migration, refusing to open"
        ))
    };
    for (owner, key) in state.plugins.storage_owners() {
        if owner == OWNER || key_marks_settlement_state(key) {
            return Err(refused(&format!("storage {owner}/{key}")));
        }
    }
    for owner in state.plugins.domain_plugins() {
        for (key, _) in state.plugins.domain_entries(owner) {
            if owner == OWNER || key_marks_settlement_state(key) {
                return Err(refused(&format!("domain {owner}/{key}")));
            }
        }
    }
    // Native NPC custody is shared infrastructure (S1 retains the NPC API for
    // unrelated consumers): only rows owned by the legacy settlement plugin
    // refuse, inspected borrowed in place through the custody map's owner
    // metadata. A settlement-owned binding also refuses its named record row.
    // S2 moved births and courtship into native villager columns. Never erase
    // an older ledger or courtship clock silently during that cutover.
    for (key, _) in state.plugins.domain_entries("__native_npcs") {
        if key == "npc/births" || key.starts_with("courtship/") {
            return Err(refused(&format!("__native_npcs/{key}")));
        }
    }
    if let Some(custody) = state.plugins.get_domain("__native_npcs", "npc/custody")
        && let Some(rows) = custody.as_object()
    {
        for (entity, binding) in rows {
            let owned = binding
                .as_array()
                .is_some_and(|pair| pair.first().is_some_and(|owner| owner == OWNER));
            if !owned {
                continue;
            }
            let handle = binding
                .as_array()
                .and_then(|pair| pair.get(1))
                .and_then(|handle| handle.as_str())
                .unwrap_or("?");
            if state
                .plugins
                .get_domain("__native_npcs", &format!("npc/record/{handle}"))
                .is_some()
            {
                return Err(refused(&format!(
                    "__native_npcs npc/record/{handle} owned by {OWNER}"
                )));
            }
            return Err(refused(&format!(
                "__native_npcs npc/custody {entity} owned by {OWNER} (handle {handle})"
            )));
        }
    }
    Ok(())
}

fn key_marks_settlement_state(key: &str) -> bool {
    key == "settlement-root-v2"
        || key == "settlement-registry-v1"
        || key.starts_with("settlement-v2/")
        || key.starts_with("settlement-fragment-v2/")
        || key.starts_with("settlement-trade-v1")
        || key.starts_with("player-contract-v1/")
        || key.starts_with("paid-shipment-v1/")
        || key == "production-root-v2"
        || key == "production-journal-v1"
        || key.starts_with("production-v2/")
        || key.starts_with("production-page-v2/")
        || key.starts_with("production-portion-v2/")
        || key.starts_with("paid-blob-v1/")
        || key.starts_with("demography-v2/")
        || key.starts_with("development-v2/")
        || key == "demography-due-v2"
}

/// Recovery hook used by [`crate::core`] once every dimension's registry and
/// entity columns have hydrated: validate records, enforce per-dimension name
/// uniqueness (names are unique per [`Registry`](crate::settlements::Registry),
/// not globally) alongside the global identity check in
/// [`validate_memberships`](crate::settlements::validate_memberships), then
/// rebuild derived counts from live villager columns. Finally pins the root
/// allocator sequence to the max over the shared checkpoint sequence, every
/// dimension registry/WAL high water, and every live id: globally monotonic,
/// never reused across dimensions or reopens. Each registry keeps its own
/// records and high water.
pub(crate) fn finish_recovery(state: &mut TickState) -> Result<(), CoreError> {
    for world in
        std::iter::once(&*state).chain(state.other_dimensions.values().map(|world| &**world))
    {
        let mut names = std::collections::BTreeSet::new();
        for record in world.settlements.records() {
            validate_record(record)
                .map_err(|error| CoreError::Storage(format!("native settlement: {error}")))?;
            super::construction::validate_pending(&world.registry, record)
                .map_err(|error| CoreError::Storage(format!("native settlement: {error}")))?;
            if !names.insert(record.name.clone()) {
                return Err(CoreError::Storage(format!(
                    "native settlement: duplicate settlement name {}",
                    record.name
                )));
            }
        }
    }
    crate::settlements::validate_memberships(state)?;
    crate::settlements::rebuild_counts(state);
    for world in state.other_dimensions.values_mut() {
        crate::settlements::rebuild_counts(world);
    }
    super::households::rebuild_sizes(state);
    let mut water = state.settlement_sequence;
    for world in
        std::iter::once(&*state).chain(state.other_dimensions.values().map(|world| &**world))
    {
        water = water
            .max(world.settlement_sequence)
            .max(world.settlements.high_water());
        for record in world.settlements.records() {
            water = water.max(record.id.0);
        }
    }
    state.settlement_sequence = water;
    Ok(())
}

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "building_persistence_tests.rs"]
mod building_persistence_tests;
