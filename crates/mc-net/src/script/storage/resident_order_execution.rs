//! Execution half of C4: real resident work, squad orders, combat, equipment.
//!
//! Every accepted call commits its durable receipt together with the ledger
//! change it decided (order, work, gear). Work executes against real blocks, real
//! recipes, real item movement and real tool durability: a job that lacks its
//! tool, input, target or route commits nothing and reports the typed pause
//! reason. A squad batch is linearised by the `mc-entity` group admission before
//! any order changes; the committed admission replays every member exactly once.

use std::collections::BTreeMap;

use mc_data::items::ItemRegistry;
use mc_data::{Identifier, ItemStack};
use mc_entity::{
    EntityItemStack, EntitySnapshot, FormationKind, FormationPlacement, FormationSlots, GoalState,
    GroupAdmission, GroupMemberObservation, RegionKey, Vec3,
};
use mc_protocol::codec;
use mc_script::{
    ScriptCombatEvent, ScriptDemobilizeResult, ScriptDemobilizeState, ScriptEngagementPolicy,
    ScriptHostileCategory, ScriptInventoryEndpoint, ScriptOperation, ScriptOperationFailure,
    ScriptOperationOutcome, ScriptOperationPayload, ScriptOperationRequest,
    ScriptOrderMemberOutcome, ScriptOrderMemberState, ScriptOrderTarget, ScriptOrderTargetRef,
    ScriptResidentOrder, ScriptResidentOrderOperation, ScriptResidentOrderResult,
    ScriptResidentWorkOrder, ScriptSettlementOperation, ScriptSettlementResult,
    ScriptWorkAssignment, ScriptWorkPauseReason, ScriptWorkState,
};

use super::resident_orders::{
    DurableAdmission, DurableAssignment, DurableGarrisonSlot, DurableMemberFence,
    DurableResidentOrder, DurableResidentOrderChange, DurableResidentOrderRecord,
    DurableResidentStack, DurableResidentWork, DurableTargetRef, TARGET_REF_TTL_REVISIONS,
    item_changes, member_fence, place_formation, target_policy,
};
use super::world_inventory::{ResidentDepositCommit, ResidentWarehouseMove};
use super::{PluginStorage, PluginStorageMutationError, ScriptStoragePrepareOutcome};
use crate::play::resident_work::{RESIDENT_WORLD_DIMENSION, ResidentDrop, ResidentWorld};
use crate::play::{ResidentAttack, ResidentGoal};

/// Attack reach of one resident melee, in blocks.
const RESIDENT_MELEE_REACH: f64 = 3.0;
/// Attack reach of one resident bow shot, in blocks.
const RESIDENT_BOW_REACH: f64 = 16.0;
/// Canonical catch of resident fishing, matching the vanilla fishing loot.
const RESIDENT_FISHING_CATCH: &str = "minecraft:cod";
/// Canonical ammo one bow shot consumes.
const RESIDENT_ARROW: &str = "minecraft:arrow";
/// Canonical bow a resident archer shoots with.
const RESIDENT_BOW: &str = "minecraft:bow";
/// Upper bound of work cells one assignment may inspect in one call.
const MAX_WORK_CELLS: u64 = 4096;

/// The implicit engagement policy of a proximity order (`hold` / `patrol`): it
/// perceives hostile mobs only. An order without an explicit policy never
/// targets players, owned residents or neutral animals.
fn proximity_policy() -> ScriptEngagementPolicy {
    ScriptEngagementPolicy::new(0, Vec::new(), vec![ScriptHostileCategory::Hostile])
}

/// Aggregated, signed item changes of one work step.
#[derive(Debug, Default, Clone)]
struct ItemLedger {
    changes: BTreeMap<String, i64>,
}

impl ItemLedger {
    fn add(&mut self, item_id: &str, delta: i64) {
        *self.changes.entry(item_id.to_owned()).or_default() += delta;
    }

    fn into_changes(self) -> Vec<mc_script::ScriptItemChange> {
        item_changes(&self.changes)
    }
}

/// One planned attack, resolved before the durable commit.
struct PlannedAttack {
    target_ref: String,
    uuid: uuid::Uuid,
    expected: EntitySnapshot,
    amount: f32,
}

/// One member's resolved execution plan inside an accepted batch.
struct MemberPlan {
    handle: String,
    entity_uuid: String,
    slot: Option<u16>,
    goal: Option<GoalState>,
    targets: Vec<ScriptOrderTarget>,
    attacks: Vec<PlannedAttack>,
    /// Approved guard post this member occupies, for a garrison order.
    garrison: Option<DurableGarrisonSlot>,
}

impl super::InventoryRuntime {
    /// Execute one durable resident work/order request.
    pub(crate) async fn execute_resident_order_operation(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let ScriptOperation::ResidentOrder { operation } = request.operation() else {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        };
        let Some(operation_id) = operation.operation_id() else {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        };
        if let Some(outcome) = replay_resident_order(storage, plugin_id, request, operation_id) {
            return Ok(outcome);
        }
        match operation {
            ScriptResidentOrderOperation::AssignWork {
                handle,
                work,
                work_units,
                expected_revision,
                ..
            } => {
                self.assign_resident_work(
                    storage,
                    plugin_id,
                    request,
                    handle,
                    work,
                    *work_units,
                    *expected_revision,
                )
                .await
            }
            ScriptResidentOrderOperation::CancelWork {
                handle,
                expected_revision,
                ..
            } => self.cancel_resident_work(storage, plugin_id, request, handle, *expected_revision),
            ScriptResidentOrderOperation::IssueOrder {
                handles,
                expected_order_revisions,
                order,
                ..
            } => {
                self.issue_resident_order(
                    storage,
                    plugin_id,
                    request,
                    handles,
                    expected_order_revisions,
                    order,
                )
                .await
            }
            ScriptResidentOrderOperation::CancelOrder {
                handles,
                expected_order_revisions,
                ..
            } => {
                self.cancel_resident_order(
                    storage,
                    plugin_id,
                    request,
                    handles,
                    expected_order_revisions,
                )
                .await
            }
            ScriptResidentOrderOperation::Demobilize {
                handle,
                expected_revision,
                ..
            } => {
                self.demobilize_resident(storage, plugin_id, request, handle, *expected_revision)
                    .await
            }
            // The closed operation union is non-exhaustive to plugins.
            _ => Ok(rejected(ScriptOperationFailure::InvalidRequest)),
        }
    }

    /// Replay every committed group admission whose members still owe an engine
    /// goal push. Called once when the storage actor starts.
    pub(crate) async fn recover_resident_orders(&self, storage: &mut PluginStorage) {
        let pending = storage.resident_orders().pending_admissions();
        for (admission_id, members) in pending {
            let mut goals = Vec::new();
            for handle in &members {
                let Some(record) = storage.resident_orders().record(handle).cloned() else {
                    continue;
                };
                let Ok(uuid) = uuid::Uuid::parse_str(&record.entity_uuid) else {
                    continue;
                };
                if let Some(goal) = self.resident_order_goal(&record) {
                    goals.push(ResidentGoal { uuid, goal });
                }
            }
            if !goals.is_empty() {
                self.sessions().apply_resident_goals(goals).await;
            }
            let change = DurableResidentOrderChange::AdmissionApplied {
                revision: 0,
                admission_id,
                members,
            };
            if storage.append_resident_order_change(change).is_err() {
                return;
            }
        }
    }

    // ---------------------------------------------------------------- work

    #[allow(clippy::too_many_arguments)]
    async fn assign_resident_work(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        handle: &str,
        work: &ScriptResidentWorkOrder,
        work_units: u64,
        expected_revision: u64,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(record) = resident_order_record(storage, plugin_id, handle) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if record.revision != expected_revision {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        // A resumed assignment continues from its committed watermark, so a
        // reload never re-charges or duplicates the units already committed.
        let resumed = match &record.work {
            Some(existing)
                if existing.work == *work
                    && existing.planned == work_units
                    && existing.state != ScriptWorkState::Cancelled =>
            {
                Some(existing.done)
            }
            _ => None,
        };
        let mut next = record.clone();
        let mut ledger = ItemLedger::default();
        let planned = work_units.min(MAX_WORK_CELLS);
        let mut staged: Option<ResidentWarehouseMove> = None;
        let (done, reason) = self
            .run_resident_work(
                storage,
                plugin_id,
                &mut next,
                work,
                planned,
                resumed.unwrap_or(0),
                &mut ledger,
                &mut staged,
            )
            .await;
        let state = match reason {
            Some(_) => ScriptWorkState::Paused,
            None if done >= planned => ScriptWorkState::Committed,
            None => ScriptWorkState::Running,
        };
        let assignment = |done: u64,
                          state: ScriptWorkState,
                          reason: Option<ScriptWorkPauseReason>,
                          ledger: ItemLedger| {
            ScriptWorkAssignment::new(
                handle.to_owned(),
                state,
                reason,
                done,
                work_units,
                ledger.into_changes(),
                0,
            )
        };
        let payload = |assignment: ScriptWorkAssignment| ScriptOperationPayload::ResidentOrder {
            result: Box::new(ScriptResidentOrderResult::Work {
                assignment: Box::new(assignment),
            }),
        };
        match staged {
            None => {
                next.work = Some(Box::new(DurableResidentWork {
                    revision: 0,
                    work: work.clone(),
                    planned: work_units,
                    done,
                    state,
                    reason,
                }));
                self.commit_resident_order(
                    storage,
                    plugin_id,
                    request,
                    payload(assignment(done, state, reason, ledger)),
                    vec![DurableResidentOrderChange::Record {
                        record: Box::new(next),
                    }],
                )?;
            }
            Some(deposit) => {
                // The record change and the container move under one decision:
                // the work assignment that reports the moved units rides the
                // container's own journal append, so a worker never reports a
                // move the container did not make - and a withdrawal reports
                // what the container gave up.
                for (item_id, count) in &deposit.moved {
                    ledger.add(item_id, deposit.container_delta(*count));
                }
                let mut committed = (*deposit.record).clone();
                committed.work = Some(Box::new(DurableResidentWork {
                    revision: 0,
                    work: work.clone(),
                    planned: work_units,
                    done,
                    state,
                    reason,
                }));
                let commit = self
                    .commit_resident_warehouse_move(
                        storage,
                        plugin_id,
                        request,
                        Box::new(committed),
                        payload(assignment(done, state, reason, ledger)),
                        &deposit.container,
                    )
                    .await?;
                if commit == ResidentDepositCommit::Refused {
                    // Nothing is durable: the worker keeps its cargo, and the
                    // step reports the storage pause on the record it started
                    // from.
                    let done = resumed.unwrap_or(0);
                    let mut reverted = record.clone();
                    reverted.work = Some(Box::new(DurableResidentWork {
                        revision: 0,
                        work: work.clone(),
                        planned: work_units,
                        done,
                        state: ScriptWorkState::Paused,
                        reason: Some(ScriptWorkPauseReason::NoStorage),
                    }));
                    self.commit_resident_order(
                        storage,
                        plugin_id,
                        request,
                        payload(assignment(
                            done,
                            ScriptWorkState::Paused,
                            Some(ScriptWorkPauseReason::NoStorage),
                            ItemLedger::default(),
                        )),
                        vec![DurableResidentOrderChange::Record {
                            record: Box::new(reverted),
                        }],
                    )?;
                }
            }
        }
        let next = resident_order_record(storage, plugin_id, handle)
            .expect("the committed work assignment remains installed");
        self.project_resident_held_item(&next).await;
        Ok(self
            .resident_order_receipt_outcome(storage, plugin_id, request)
            .expect("committed resident work receipt remains installed"))
    }

    fn cancel_resident_work(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        handle: &str,
        expected_revision: u64,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(record) = resident_order_record(storage, plugin_id, handle) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if record.revision != expected_revision {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        let mut next = record.clone();
        if let Some(work) = &mut next.work {
            let mut cancelled = (**work).clone();
            cancelled.state = ScriptWorkState::Cancelled;
            cancelled.reason = None;
            cancelled.revision = 0;
            next.work = Some(Box::new(cancelled));
        }
        let payload = ScriptOperationPayload::ResidentOrder {
            result: Box::new(ScriptResidentOrderResult::WorkCancelled {
                handle: handle.to_owned(),
                revision: 0,
            }),
        };
        self.commit_resident_order(
            storage,
            plugin_id,
            request,
            payload,
            vec![DurableResidentOrderChange::Record {
                record: Box::new(next),
            }],
        )?;
        Ok(self
            .resident_order_receipt_outcome(storage, plugin_id, request)
            .expect("committed work cancellation remains installed"))
    }

    /// Execute one bounded step of a work assignment. Returns the committed work
    /// units and, when the step stopped, its typed reason. A haul into a bound
    /// warehouse is planned into `staged` instead of moved in place, because its
    /// container half commits with the record change the caller is building.
    #[allow(clippy::too_many_arguments)]
    async fn run_resident_work(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        record: &mut DurableResidentOrderRecord,
        work: &ScriptResidentWorkOrder,
        planned: u64,
        already_done: u64,
        ledger: &mut ItemLedger,
        staged: &mut Option<ResidentWarehouseMove>,
    ) -> (u64, Option<ScriptWorkPauseReason>) {
        let remaining = planned.saturating_sub(already_done);
        if remaining == 0 {
            return (already_done, None);
        }
        let Some(world) = self.resident_world() else {
            // No world adapter is installed: fail closed instead of inventing
            // resources.
            return (already_done, Some(ScriptWorkPauseReason::Unsupported));
        };
        match work {
            ScriptResidentWorkOrder::Harvest { area, tool } => {
                let tool = resource_item(tool);
                if !gear_has(record, &tool) {
                    return (already_done, Some(ScriptWorkPauseReason::MissingTool));
                }
                if world.foreign_zone_overlaps(
                    plugin_id,
                    &area.dimension,
                    bounds_min(area),
                    bounds_max(area),
                ) {
                    return (already_done, Some(ScriptWorkPauseReason::Protected));
                }
                let mut done = already_done;
                for cell in work_cells(area) {
                    if done - already_done >= remaining {
                        break;
                    }
                    let Some(block) = world.block(&area.dimension, cell) else {
                        return (done, Some(ScriptWorkPauseReason::Unloaded));
                    };
                    if !is_harvestable_crop(&block.path) {
                        continue;
                    }
                    let drops = match world.preview_break(
                        &area.dimension,
                        cell,
                        block.state,
                        Some(&tool),
                    ) {
                        Ok(drops) => drops,
                        Err(failure) => {
                            return (done, Some(work_failure_reason(failure)));
                        }
                    };
                    // The worker must be able to hold the loot *before* the
                    // crop leaves the world: a full worker leaves the field
                    // standing instead of harvesting into nothing.
                    if !self.drops_fit(record, &drops) {
                        let reason = if done == already_done {
                            ScriptWorkPauseReason::NoStorage
                        } else {
                            return (done, None);
                        };
                        return (done, Some(reason));
                    }
                    match world.break_block(&area.dimension, cell, block.state, Some(&tool)) {
                        Ok(committed) => {
                            if !self.deposit_drops(record, &committed, ledger) {
                                return (done, Some(ScriptWorkPauseReason::NoStorage));
                            }
                            wear_tool(record, &tool, ledger, 1);
                            done += 1;
                        }
                        Err(failure) => return (done, Some(work_failure_reason(failure))),
                    }
                }
                if done == already_done {
                    return (done, Some(ScriptWorkPauseReason::MissingInput));
                }
                (done, None)
            }
            ScriptResidentWorkOrder::Replant { area, seed, tool } => {
                let tool = resource_item(tool);
                let seed_item = resource_item(seed);
                if !gear_has(record, &tool) {
                    return (already_done, Some(ScriptWorkPauseReason::MissingTool));
                }
                if !gear_has(record, &seed_item) {
                    return (already_done, Some(ScriptWorkPauseReason::MissingInput));
                }
                let Some(crop) = crop_block_for_seed(&seed_item) else {
                    return (already_done, Some(ScriptWorkPauseReason::Unsupported));
                };
                let Some(crop_state) = world.state_for(crop) else {
                    return (already_done, Some(ScriptWorkPauseReason::Unsupported));
                };
                let mut done = already_done;
                for cell in work_cells(area) {
                    if done - already_done >= remaining || !gear_has(record, &seed_item) {
                        break;
                    }
                    let Some(block) = world.block(&area.dimension, cell) else {
                        return (done, Some(ScriptWorkPauseReason::Unloaded));
                    };
                    if block.path != "farmland" {
                        continue;
                    }
                    let above = [cell[0], cell[1] + 1, cell[2]];
                    let Some(above_block) = world.block(&area.dimension, above) else {
                        return (done, Some(ScriptWorkPauseReason::Unloaded));
                    };
                    if !is_air_path(&above_block.path) {
                        continue;
                    }
                    match world.place_block(&area.dimension, above, crop_state) {
                        Ok(()) => {
                            // The seed leaves the canonical slot in the same
                            // committed record as the placement.
                            wear_gear_count(record, &seed_item, 1, ledger);
                            done += 1;
                        }
                        Err(failure) => return (done, Some(work_failure_reason(failure))),
                    }
                }
                if done == already_done {
                    return (done, Some(ScriptWorkPauseReason::MissingInput));
                }
                (done, None)
            }
            ScriptResidentWorkOrder::CutTree { area, tool } => {
                let tool = resource_item(tool);
                if !gear_has(record, &tool) {
                    return (already_done, Some(ScriptWorkPauseReason::MissingTool));
                }
                if world.foreign_zone_overlaps(
                    plugin_id,
                    &area.dimension,
                    bounds_min(area),
                    bounds_max(area),
                ) {
                    return (already_done, Some(ScriptWorkPauseReason::Protected));
                }
                let mut done = already_done;
                for cell in work_cells(area) {
                    if done - already_done >= remaining {
                        break;
                    }
                    let Some(block) = world.block(&area.dimension, cell) else {
                        return (done, Some(ScriptWorkPauseReason::Unloaded));
                    };
                    if !is_log_path(&block.path) {
                        continue;
                    }
                    // A log that is not rooted in natural ground is another
                    // player's wooden build; it is never cut.
                    let below = [cell[0], cell[1] - 1, cell[2]];
                    let rooted = world.block(&area.dimension, below).is_some_and(|below| {
                        is_log_path(&below.path) || is_ground_path(&below.path)
                    });
                    if !rooted {
                        continue;
                    }
                    let drops = match world.preview_break(
                        &area.dimension,
                        cell,
                        block.state,
                        Some(&tool),
                    ) {
                        Ok(drops) => drops,
                        Err(failure) => {
                            return (done, Some(work_failure_reason(failure)));
                        }
                    };
                    if !self.drops_fit(record, &drops) {
                        if done == already_done {
                            return (done, Some(ScriptWorkPauseReason::NoStorage));
                        }
                        return (done, None);
                    }
                    match world.break_block(&area.dimension, cell, block.state, Some(&tool)) {
                        Ok(committed) => {
                            if !self.deposit_drops(record, &committed, ledger) {
                                return (done, Some(ScriptWorkPauseReason::NoStorage));
                            }
                            wear_tool(record, &tool, ledger, 1);
                            done += 1;
                        }
                        Err(failure) => return (done, Some(work_failure_reason(failure))),
                    }
                }
                if done == already_done {
                    return (done, Some(ScriptWorkPauseReason::MissingInput));
                }
                (done, None)
            }
            ScriptResidentWorkOrder::Mine { area, tool } => {
                let tool = resource_item(tool);
                if !gear_has(record, &tool) {
                    return (already_done, Some(ScriptWorkPauseReason::MissingTool));
                }
                if world.foreign_zone_overlaps(
                    plugin_id,
                    &area.dimension,
                    bounds_min(area),
                    bounds_max(area),
                ) {
                    return (already_done, Some(ScriptWorkPauseReason::Protected));
                }
                let mut done = already_done;
                for cell in work_cells(area) {
                    if done - already_done >= remaining {
                        break;
                    }
                    let Some(block) = world.block(&area.dimension, cell) else {
                        return (done, Some(ScriptWorkPauseReason::Unloaded));
                    };
                    // Only ore already inside the bounded work area is mined;
                    // core never scans the world for ore.
                    if !is_ore_path(&block.path) {
                        continue;
                    }
                    let drops = match world.preview_break(
                        &area.dimension,
                        cell,
                        block.state,
                        Some(&tool),
                    ) {
                        Ok(drops) => drops,
                        Err(failure) => {
                            return (done, Some(work_failure_reason(failure)));
                        }
                    };
                    if !self.drops_fit(record, &drops) {
                        if done == already_done {
                            return (done, Some(ScriptWorkPauseReason::NoStorage));
                        }
                        return (done, None);
                    }
                    match world.break_block(&area.dimension, cell, block.state, Some(&tool)) {
                        Ok(committed) => {
                            if !self.deposit_drops(record, &committed, ledger) {
                                return (done, Some(ScriptWorkPauseReason::NoStorage));
                            }
                            wear_tool(record, &tool, ledger, 1);
                            done += 1;
                        }
                        Err(failure) => return (done, Some(work_failure_reason(failure))),
                    }
                }
                if done == already_done {
                    return (done, Some(ScriptWorkPauseReason::MissingInput));
                }
                (done, None)
            }
            ScriptResidentWorkOrder::Fish { area, tool } => {
                let tool = resource_item(tool);
                if !gear_has(record, &tool) {
                    return (already_done, Some(ScriptWorkPauseReason::MissingTool));
                }
                let mut columns = std::collections::BTreeSet::new();
                for cell in work_cells(area) {
                    let Some(block) = world.block(&area.dimension, cell) else {
                        return (already_done, Some(ScriptWorkPauseReason::Unloaded));
                    };
                    if block.path == "water" {
                        columns.insert((cell[0], cell[2]));
                    }
                }
                if columns.is_empty() {
                    return (already_done, Some(ScriptWorkPauseReason::MissingInput));
                }
                // One canonical catch per real water column: the bounded water in
                // the named area is the only source.
                let catchable = u64::try_from(columns.len()).unwrap_or(0).min(remaining);
                let catch_id = resource_item(RESIDENT_FISHING_CATCH);
                let mut caught = 0_u64;
                for _ in 0..catchable {
                    // The catch is canonical, not a roll: one cod per water
                    // column, and it only counts when the worker can hold it.
                    let drops = [ResidentDrop {
                        item_id: catch_id.clone(),
                        count: 1,
                    }];
                    if !self.drops_fit(record, &drops) {
                        break;
                    }
                    if !self.deposit_drops(record, &drops, ledger) {
                        break;
                    }
                    caught += 1;
                }
                if caught > 0 {
                    wear_tool(record, &tool, ledger, 1);
                } else if catchable > 0 {
                    return (already_done, Some(ScriptWorkPauseReason::NoStorage));
                }
                (already_done + caught, None)
            }
            ScriptResidentWorkOrder::TendLivestock { area, feed } => {
                let feed_item = resource_item(feed);
                if !gear_has(record, &feed_item) {
                    return (already_done, Some(ScriptWorkPauseReason::MissingInput));
                }
                let center = Vec3::new(
                    (f64::from(area.min.x) + f64::from(area.max.x)) / 2.0,
                    f64::from(area.min.y),
                    (f64::from(area.min.z) + f64::from(area.max.z)) / 2.0,
                );
                let radius = f64::from(
                    (area.max.x - area.min.x)
                        .max(area.max.y - area.min.y)
                        .max(area.max.z - area.min.z)
                        + 1,
                );
                let animals = self.sessions().resident_perception(
                    center,
                    radius,
                    &[ScriptHostileCategory::NeutralAnimal],
                );
                if animals.is_empty() {
                    return (already_done, Some(ScriptWorkPauseReason::MissingInput));
                }
                let mut done = already_done;
                for _ in animals {
                    if done - already_done >= remaining || !gear_has(record, &feed_item) {
                        break;
                    }
                    // Ranching consumes feed for a real animal and produces
                    // nothing by itself.
                    wear_gear_count(record, &feed_item, 1, ledger);
                    done += 1;
                }
                (done, None)
            }
            ScriptResidentWorkOrder::Haul {
                source,
                destination,
                item,
            } => {
                // A haul with a bound warehouse on either side is not a move
                // inside the worker's own record: its other half is a real
                // container, and it commits with this step's record change as one
                // decision.
                if matches!(source, ScriptInventoryEndpoint::Warehouse { .. })
                    || matches!(destination, ScriptInventoryEndpoint::Warehouse { .. })
                {
                    let (moved, reason) = self.stage_warehouse_move(
                        storage,
                        plugin_id,
                        record,
                        source,
                        destination,
                        item.as_deref(),
                        remaining,
                        staged,
                    );
                    (already_done + moved, reason)
                } else {
                    let (moved, reason) = self.haul_resident_items(
                        record,
                        source,
                        destination,
                        item.as_deref(),
                        remaining,
                        ledger,
                    );
                    (already_done + moved, reason)
                }
            }
            ScriptResidentWorkOrder::Craft { recipe, count } => {
                self.craft_resident_items(record, recipe, *count, remaining, ledger)
            }
            ScriptResidentWorkOrder::Construct {
                structure_id,
                stage,
                expected_revision,
            } => {
                self.advance_structure_work(
                    storage,
                    plugin_id,
                    structure_id,
                    stage,
                    *expected_revision,
                    remaining,
                )
                .await
            }
            // The closed work union is non-exhaustive to plugins.
            _ => (already_done, Some(ScriptWorkPauseReason::Unsupported)),
        }
    }

    /// Plan one haul that has a bound warehouse container on either side.
    ///
    /// The step moves nothing by itself: it stages the worker's record
    /// after-image and the container images it read, and the caller commits
    /// both together under one durable decision. A container core cannot
    /// resolve, or one that cannot take the cargo (deposit) or does not hold
    /// what was asked for (withdrawal), stops the job with the reason its
    /// failure family names and leaves every item where it was.
    #[allow(clippy::too_many_arguments)]
    fn stage_warehouse_move(
        &self,
        storage: &PluginStorage,
        plugin_id: &str,
        record: &DurableResidentOrderRecord,
        source: &ScriptInventoryEndpoint,
        destination: &ScriptInventoryEndpoint,
        item: Option<&str>,
        limit: u64,
        staged: &mut Option<ResidentWarehouseMove>,
    ) -> (u64, Option<ScriptWorkPauseReason>) {
        let planned = match self.plan_resident_warehouse_move(
            storage,
            plugin_id,
            source,
            destination,
            item,
            record,
            limit,
        ) {
            Ok(planned) => planned,
            Err(failure) => {
                let reason = match failure {
                    // A container the worker cannot move through is a storage
                    // refusal: every item stays where it was.
                    ScriptOperationFailure::NotFound
                    | ScriptOperationFailure::Unloaded
                    | ScriptOperationFailure::Blocked
                    | ScriptOperationFailure::Forbidden => ScriptWorkPauseReason::NoStorage,
                    // A destination with no room is a storage refusal when the
                    // storage is the container. A worker taking items out of a
                    // container has no storage problem - its own slots are full -
                    // and the opposite move (a deposit back into the container)
                    // is what frees room, so that step is interrupted rather
                    // than refused.
                    ScriptOperationFailure::Capacity
                        if !matches!(source, ScriptInventoryEndpoint::Warehouse { .. }) =>
                    {
                        ScriptWorkPauseReason::NoStorage
                    }
                    failure => work_failure_reason(failure),
                };
                return (0, Some(reason));
            }
        };
        let units = planned.units();
        *staged = Some(planned);
        (units, None)
    }

    /// Move real items between the worker's canonical resident endpoints through
    /// the same slot semantics C1 defines, taking only the named item when the
    /// order names one. A warehouse endpoint core cannot resolve stops the job
    /// with `no_storage` and moves nothing.
    fn haul_resident_items(
        &self,
        record: &mut DurableResidentOrderRecord,
        source: &ScriptInventoryEndpoint,
        destination: &ScriptInventoryEndpoint,
        item: Option<&str>,
        limit: u64,
        ledger: &mut ItemLedger,
    ) -> (u64, Option<ScriptWorkPauseReason>) {
        let (Some(source_len), Some(destination_len)) = (
            resident_endpoint_len(source, record),
            resident_endpoint_len(destination, record),
        ) else {
            let reason = match (source, destination) {
                (ScriptInventoryEndpoint::Warehouse { .. }, _)
                | (_, ScriptInventoryEndpoint::Warehouse { .. }) => {
                    ScriptWorkPauseReason::NoStorage
                }
                _ => ScriptWorkPauseReason::Unsupported,
            };
            return (0, Some(reason));
        };
        let mut moved = 0_u64;
        'outer: for source_index in 0..source_len {
            for destination_index in 0..destination_len {
                if moved >= limit {
                    break 'outer;
                }
                // Re-read the source so a moved stack is never replayed from a
                // stale clone.
                let Some(stack) = resident_slot(record, source, source_index) else {
                    break;
                };
                if stack.count == 0 {
                    break;
                }
                if item.is_some_and(|wanted| stack.item_id.as_str() != wanted) {
                    // The order named an item this slot does not hold: it stays
                    // where it is and the next source slot is tried.
                    break;
                }
                if resident_slot(record, destination, destination_index).is_some() {
                    continue;
                }
                let count = u32::try_from(limit - moved)
                    .unwrap_or(u32::MAX)
                    .min(stack.count);
                if count == 0 {
                    continue;
                }
                let item_id = stack.item_id.clone();
                take_from_resident_slot(record, source, source_index, count);
                if put_into_resident_slot(
                    record,
                    destination,
                    destination_index,
                    DurableResidentStack {
                        item_id: item_id.clone(),
                        count,
                        damage: stack.damage,
                        enchantments: stack.enchantments.clone(),
                        custom_name: stack.custom_name.clone(),
                        item_model: stack.item_model.clone(),
                    },
                ) {
                    ledger.add(&item_id, i64::from(count));
                    moved += u64::from(count);
                }
                break;
            }
        }
        if moved == 0 {
            return (0, Some(ScriptWorkPauseReason::MissingInput));
        }
        (moved, None)
    }

    /// Craft a real recipe from the resident's own canonical slots. Inputs are
    /// consumed only for crafts that complete; a missing ingredient stops the
    /// job with `missing_input` and reports only the committed crafts.
    fn craft_resident_items(
        &self,
        record: &mut DurableResidentOrderRecord,
        recipe_id: &str,
        count: u32,
        limit: u64,
        ledger: &mut ItemLedger,
    ) -> (u64, Option<ScriptWorkPauseReason>) {
        let recipes = mc_data::recipes::solaris_required_recipes();
        let Some(recipe) = recipes
            .iter()
            .find(|recipe| recipe.id.as_str() == recipe_id)
        else {
            return (0, Some(ScriptWorkPauseReason::Unsupported));
        };
        let Some(ingredients) = recipe_ingredients(recipe) else {
            return (0, Some(ScriptWorkPauseReason::Unsupported));
        };
        if ingredients.is_empty() {
            return (0, Some(ScriptWorkPauseReason::Unsupported));
        }
        let tags = mc_data::tags::solaris_required_item_tags(self.items());
        let Some(result) = recipe.result.to_stack(self.items()) else {
            return (0, Some(ScriptWorkPauseReason::Unsupported));
        };
        let Some(result_name) = self.items().name_of(result.item_id) else {
            return (0, Some(ScriptWorkPauseReason::Unsupported));
        };
        let result_name = result_name.as_str().to_owned();
        let mut done = 0_u64;
        while done < u64::from(count) && done < limit {
            let mut taken: Vec<String> = Vec::new();
            let mut satisfied = true;
            for ingredient in &ingredients {
                match take_ingredient(record, self.items(), &tags, ingredient, ledger) {
                    Some(item_id) => taken.push(item_id),
                    None => {
                        satisfied = false;
                        break;
                    }
                }
            }
            if !satisfied {
                // Put back the ingredients of the incomplete craft; nothing was
                // committed for it.
                for item_id in taken {
                    let max_stack = self.drop_max_stack(&item_id);
                    put_resident_item(record, &item_id, 1, max_stack);
                    ledger.add(&item_id, 1);
                }
                break;
            }
            let max_stack = self.drop_max_stack(&result_name);
            let produced = u64::try_from(result.count).unwrap_or(0);
            if !put_resident_item(record, &result_name, produced, max_stack) {
                return (done, Some(ScriptWorkPauseReason::NoStorage));
            }
            ledger.add(&result_name, i64::from(result.count));
            done += 1;
        }
        if done == 0 {
            return (0, Some(ScriptWorkPauseReason::MissingInput));
        }
        (done, None)
    }

    /// Drive one prepared C2 structure stage through its committed reservation.
    async fn advance_structure_work(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        structure_id: &str,
        stage: &str,
        expected_revision: u64,
        work_units: u64,
    ) -> (u64, Option<ScriptWorkPauseReason>) {
        let Some(target) = storage.resident_builder_target(structure_id) else {
            return (0, Some(ScriptWorkPauseReason::MissingInput));
        };
        if target.plugin_id != plugin_id {
            return (0, Some(ScriptWorkPauseReason::Protected));
        }
        if target.revision != expected_revision {
            return (0, Some(ScriptWorkPauseReason::Interrupted));
        }
        // One settlement operation id per structure and committed revision, so
        // each portion of a multi-portion stage is a distinct mutation while a
        // repeated portion replays its stored receipt instead of double-charging.
        let operation_id = format!(
            "c4build{:016x}{:016x}",
            structure_hash(structure_id),
            expected_revision
        );
        let request = match ScriptOperationRequest::try_new(
            &operation_id,
            ScriptOperation::Settlement {
                operation: ScriptSettlementOperation::AdvanceStructure {
                    operation_id: operation_id.clone(),
                    structure_id: structure_id.to_owned(),
                    stage: stage.to_owned(),
                    reservation_ref: target.reservation_ref,
                    expected_revision,
                    work_units: work_units.min(mc_script::MAX_WORLD_COMMIT_PORTION as u64),
                },
            },
        ) {
            Ok(request) => request,
            Err(_) => return (0, Some(ScriptWorkPauseReason::Unsupported)),
        };
        match self
            .execute_settlement_operation(storage, plugin_id, &request)
            .await
        {
            Ok(outcome) => match outcome.payload() {
                ScriptOperationPayload::Settlement { result } => match &**result {
                    ScriptSettlementResult::Receipt { receipt } => (receipt.work_units, None),
                    _ => (0, Some(ScriptWorkPauseReason::MissingInput)),
                },
                _ => (
                    0,
                    Some(work_failure_reason(
                        outcome.failure().unwrap_or(ScriptOperationFailure::Blocked),
                    )),
                ),
            },
            Err(_) => (0, Some(ScriptWorkPauseReason::Unsupported)),
        }
    }

    // -------------------------------------------------------------- orders

    #[allow(clippy::too_many_arguments)]
    async fn issue_resident_order(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        handles: &[String],
        expected_order_revisions: &[u64],
        order: &ScriptResidentOrder,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(records) =
            resident_order_records(storage, plugin_id, handles, expected_order_revisions)
        else {
            return Ok(resident_order_batch_refusal(handles));
        };
        let Some(Some(mut admission)) = self
            .prepare_resident_order_batch(storage, request, &records)
            .await
        else {
            return Ok(resident_order_batch_refusal(handles));
        };
        if !commit_resident_order_batch(self.sessions(), &records, &mut admission).await {
            return Ok(resident_order_batch_refusal(handles));
        }
        let admission_id = admission.admission_id();
        let transaction_id = storage
            .next_transaction_id()
            .map_err(|_| PluginStorageMutationError::RevisionOverflow)?;
        let mut next_records = records.clone();
        let (mut plans, refs) = self
            .plan_member_orders(storage, plugin_id, &mut next_records, order, transaction_id)
            .await;
        // Real combat resolves before the durable commit, so the receipt carries
        // only damage the engine actually committed.
        let attacks = plans
            .iter()
            .flat_map(|plan| plan.attacks.iter())
            .map(|attack| ResidentAttack {
                uuid: attack.uuid,
                amount: attack.amount,
                expected: attack.expected.clone(),
            })
            .collect::<Vec<_>>();
        let hits = self.sessions().commit_resident_damage(attacks).await;
        let mut combat = Vec::new();
        let mut hit_index = 0_usize;
        for plan in &mut plans {
            let plan_attacks = std::mem::take(&mut plan.attacks);
            for attack in plan_attacks {
                let hit = hits.get(hit_index).and_then(Option::as_ref);
                hit_index += 1;
                let Some(hit) = hit else {
                    continue;
                };
                let damage_milli = (hit.damage * 1000.0).round().max(0.0) as u64;
                if damage_milli == 0 {
                    continue;
                }
                let event_id = (transaction_id << 6) | (combat.len() as u64 & 63);
                let Ok(event) = ok_combat_event(
                    event_id,
                    transaction_id,
                    plan.handle.clone(),
                    attack.target_ref,
                    transaction_id,
                    u32::try_from(damage_milli).unwrap_or(u32::MAX),
                    hit.killed,
                ) else {
                    continue;
                };
                combat.push(event);
            }
        }
        let mut changes = Vec::new();
        for plan in &plans {
            let Some(record) = next_records.get(&plan.handle).cloned() else {
                continue;
            };
            changes.push(DurableResidentOrderChange::Record {
                record: Box::new(member_order_record(record, order, plan)),
            });
        }
        changes.extend(
            refs.into_iter()
                .map(|reference| DurableResidentOrderChange::TargetRef {
                    revision: 0,
                    reference: Box::new(reference),
                }),
        );
        changes.push(DurableResidentOrderChange::Admission {
            admission: Box::new(durable_admission(
                admission_id,
                plugin_id,
                request
                    .operation_id()
                    .expect("resident order decision identity"),
                request_fingerprint(request),
                &records,
                &plans,
            )),
        });
        let mut members = plans
            .iter()
            .map(|plan| {
                ScriptOrderMemberOutcome::new(
                    plan.handle.clone(),
                    order_member_state(plan),
                    plan.slot,
                    plan.targets.clone(),
                )
            })
            .collect::<Vec<_>>();
        members.sort_unstable_by(|left, right| left.handle.cmp(&right.handle));
        let payload = ScriptOperationPayload::ResidentOrder {
            result: Box::new(ScriptResidentOrderResult::Order {
                order_revision: 0,
                members: members.clone(),
                combat,
            }),
        };
        self.commit_resident_order(storage, plugin_id, request, payload, changes)?;
        self.apply_order_plans(&plans).await;
        let applied = plans.iter().map(|plan| plan.handle.clone()).collect();
        self.acknowledge_resident_admission(storage, admission_id, applied);
        Ok(self
            .resident_order_receipt_outcome(storage, plugin_id, request)
            .expect("committed resident order receipt remains installed"))
    }

    async fn cancel_resident_order(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        handles: &[String],
        expected_order_revisions: &[u64],
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(records) =
            resident_order_records(storage, plugin_id, handles, expected_order_revisions)
        else {
            return Ok(resident_order_batch_refusal(handles));
        };
        let Some(Some(mut admission)) = self
            .prepare_resident_order_batch(storage, request, &records)
            .await
        else {
            return Ok(resident_order_batch_refusal(handles));
        };
        if !commit_resident_order_batch(self.sessions(), &records, &mut admission).await {
            return Ok(resident_order_batch_refusal(handles));
        }
        let admission_id = admission.admission_id();
        let mut members = Vec::new();
        let mut changes = Vec::new();
        for (index, handle) in handles.iter().enumerate() {
            let Some(record) = records.get(handle) else {
                continue;
            };
            let mut next = record.clone();
            next.order = None;
            changes.push(DurableResidentOrderChange::Record {
                record: Box::new(next),
            });
            // `formation_slot` carries the member's batch ordinal for a
            // cancellation; the closed outcome schema requires a slot on an
            // accepted member.
            members.push(ScriptOrderMemberOutcome::new(
                handle.clone(),
                ScriptOrderMemberState::Applied,
                Some(u16::try_from(index).unwrap_or(u16::MAX)),
                Vec::new(),
            ));
        }
        members.sort_unstable_by(|left, right| left.handle.cmp(&right.handle));
        changes.push(DurableResidentOrderChange::Admission {
            admission: Box::new(durable_admission(
                admission_id,
                plugin_id,
                request
                    .operation_id()
                    .expect("resident order decision identity"),
                request_fingerprint(request),
                &records,
                &[],
            )),
        });
        let payload = ScriptOperationPayload::ResidentOrder {
            result: Box::new(ScriptResidentOrderResult::OrderCancelled {
                order_revision: 0,
                members: members.clone(),
            }),
        };
        self.commit_resident_order(storage, plugin_id, request, payload, changes)?;
        let goals = records
            .values()
            .filter_map(|record| {
                uuid::Uuid::parse_str(&record.entity_uuid)
                    .ok()
                    .map(|uuid| ResidentGoal {
                        uuid,
                        goal: GoalState::Idle,
                    })
            })
            .collect::<Vec<_>>();
        self.sessions().apply_resident_goals(goals).await;
        self.acknowledge_resident_admission(storage, admission_id, handles.to_vec());
        Ok(self
            .resident_order_receipt_outcome(storage, plugin_id, request)
            .expect("committed order cancellation remains installed"))
    }

    async fn demobilize_resident(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        handle: &str,
        expected_revision: u64,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(record) = resident_order_record(storage, plugin_id, handle) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if record.revision != expected_revision {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        // Returning gear needs a writable warehouse endpoint. Core has no
        // warehouse resolver, so the resident keeps its handle, housing and
        // every item on its canonical slots and stays demobilising.
        let mut next = record.clone();
        next.assignment = DurableAssignment::Demobilizing;
        next.order = None;
        let resident = ScriptDemobilizeResult::new(
            handle.to_owned(),
            ScriptDemobilizeState::Demobilizing,
            Some(ScriptWorkPauseReason::NoStorage),
            Vec::new(),
            0,
        );
        let payload = ScriptOperationPayload::ResidentOrder {
            result: Box::new(ScriptResidentOrderResult::Demobilized {
                resident: Box::new(resident),
            }),
        };
        self.commit_resident_order(
            storage,
            plugin_id,
            request,
            payload,
            vec![DurableResidentOrderChange::Record {
                record: Box::new(next),
            }],
        )?;
        Ok(self
            .resident_order_receipt_outcome(storage, plugin_id, request)
            .expect("committed demobilisation remains installed"))
    }

    // ------------------------------------------------------------ planning

    /// Prepare the engine group admission for one batch. `None` is a hard
    /// refusal; `Some(None)` is a batch refused with per-member reasons.
    pub(super) async fn prepare_resident_order_batch(
        &self,
        storage: &mut PluginStorage,
        request: &ScriptOperationRequest,
        records: &BTreeMap<String, DurableResidentOrderRecord>,
    ) -> Option<Option<GroupAdmission<String>>> {
        let admission_id = storage.next_transaction_id().ok()?;
        let observations = observe_resident_members(self.sessions(), records).await;
        let requests = records
            .iter()
            .map(|(handle, record)| {
                let region = observations
                    .get(handle)
                    .map(|observation| observation.region)
                    .unwrap_or(RegionKey { x: 0, z: 0 });
                (
                    member_fence(handle, record, region),
                    observations.get(handle).copied(),
                )
            })
            .collect::<Vec<_>>();
        match GroupAdmission::prepare(admission_id, request_fingerprint(request), &requests) {
            Ok(admission) => Some(Some(admission)),
            Err(_) => Some(None),
        }
    }

    /// Plan every member's formation slot, engine goal and bounded attack set.
    /// Returns the plans and the durable target references the plan minted.
    async fn plan_member_orders(
        &self,
        storage: &PluginStorage,
        plugin_id: &str,
        next_records: &mut BTreeMap<String, DurableResidentOrderRecord>,
        order: &ScriptResidentOrder,
        transaction_id: u64,
    ) -> (Vec<MemberPlan>, Vec<DurableTargetRef>) {
        let member_uuids = next_records
            .values()
            .filter_map(|record| uuid::Uuid::parse_str(&record.entity_uuid).ok())
            .collect::<Vec<_>>();
        let member_snapshots = self
            .sessions()
            .resident_entity_snapshots(&member_uuids)
            .await;
        let member_positions = member_uuids
            .iter()
            .zip(member_snapshots.iter())
            .filter_map(|(uuid, snapshot)| {
                snapshot.as_ref().map(|snapshot| (*uuid, snapshot.position))
            })
            .collect::<BTreeMap<_, _>>();
        let reference_uuids = order_reference_uuids(storage, plugin_id, order);
        let reference_snapshots = self
            .sessions()
            .resident_entity_snapshots(&reference_uuids)
            .await;
        let references = reference_uuids
            .iter()
            .zip(reference_snapshots.iter())
            .filter_map(|(uuid, snapshot)| snapshot.as_ref().map(|snapshot| (*uuid, snapshot)))
            .collect::<BTreeMap<_, _>>();
        let live_players = self.sessions().resident_live_player_uuids();
        let handles = next_records.keys().cloned().collect::<Vec<_>>();
        let count = handles.len();
        // Garrison occupancy is resolved once for the whole batch: the approved
        // posts of this order and the durable claims every other live order
        // holds, so a second order never double-books a slot.
        let (garrison_posts, mut garrison_claims) = match order {
            ScriptResidentOrder::Garrison { posts, .. } => (
                self.settlement_runtime()
                    .map(|runtime| runtime.guard_posts(posts))
                    .unwrap_or_default(),
                storage
                    .resident_orders()
                    .garrison_claims(plugin_id)
                    .into_iter()
                    .map(|(post, slot, handle)| ((post, slot), handle))
                    .collect::<BTreeMap<_, _>>(),
            ),
            _ => (BTreeMap::new(), BTreeMap::new()),
        };
        let mut plans = Vec::new();
        let mut refs = Vec::new();
        for (index, handle) in handles.iter().enumerate() {
            let Some(record) = next_records.get(handle).cloned() else {
                continue;
            };
            let mut plan = MemberPlan {
                handle: handle.clone(),
                entity_uuid: record.entity_uuid.clone(),
                slot: None,
                goal: None,
                targets: Vec::new(),
                attacks: Vec::new(),
                garrison: None,
            };
            match order {
                ScriptResidentOrder::Follow { target_player, .. } => {
                    // The engine follows the moving player. A target that is no
                    // longer an online actor never leaves a chase behind.
                    if let Some((entity, position)) =
                        self.sessions().resident_actor_entity(*target_player)
                    {
                        plan.goal = Some(GoalState::FollowTarget {
                            target: entity,
                            speed: 1.0,
                        });
                        plan.slot = self
                            .formation_slot_at(order, position, index, count)
                            .map(|(slot, _)| slot)
                            .or_else(|| Some(u16::try_from(index).unwrap_or(u16::MAX)));
                    } else {
                        plan.goal = Some(GoalState::Idle);
                        plan.slot = Some(u16::try_from(index).unwrap_or(u16::MAX));
                    }
                }
                ScriptResidentOrder::Hold {
                    engagement_radius, ..
                } => {
                    plan.goal = Some(GoalState::Idle);
                    let anchor = order_anchor(order).unwrap_or(Vec3::ZERO);
                    plan.slot = self
                        .formation_slot_at(order, anchor, index, count)
                        .map(|(slot, _)| slot);
                    let (targets, minted) = self.perceived_targets(
                        storage,
                        &record,
                        &proximity_policy(),
                        f64::from(*engagement_radius),
                        member_positions.get(&uuid_of(&record)),
                        &live_players,
                        plugin_id,
                        transaction_id,
                    );
                    plan.targets = targets;
                    refs.extend(minted);
                }
                ScriptResidentOrder::Garrison {
                    engagement_radius, ..
                } => {
                    // Occupy a free approved guard post from C2's committed site
                    // layout with an engine-computed, stable position. A member
                    // with no reachable free slot honestly reports blocked_route
                    // and pushes no goal instead of refusing outright.
                    let from = member_positions.get(&uuid_of(&record));
                    let dimension = order_dimension(order);
                    let mut chosen = None;
                    if let Some(existing) = record
                        .order
                        .as_ref()
                        .and_then(|order| order.garrison.clone())
                        && let Some(position) = self.garrison_slot_position(
                            &existing.post,
                            existing.slot,
                            &garrison_posts,
                            &garrison_claims,
                            handle,
                            from,
                            dimension,
                        )
                    {
                        // A reloaded order keeps the post it already holds.
                        chosen = Some((existing, position));
                    }
                    if chosen.is_none() {
                        'posts: for (post, (_, capacity)) in &garrison_posts {
                            for slot in 0..*capacity {
                                if let Some(position) = self.garrison_slot_position(
                                    post,
                                    slot,
                                    &garrison_posts,
                                    &garrison_claims,
                                    handle,
                                    from,
                                    dimension,
                                ) {
                                    chosen = Some((
                                        DurableGarrisonSlot {
                                            post: post.clone(),
                                            slot,
                                        },
                                        position,
                                    ));
                                    break 'posts;
                                }
                            }
                        }
                    }
                    if let Some((assignment, position)) = chosen {
                        garrison_claims
                            .insert((assignment.post.clone(), assignment.slot), handle.clone());
                        plan.garrison = Some(assignment);
                        plan.slot = Some(u16::try_from(index).unwrap_or(u16::MAX));
                        plan.goal = Some(GoalState::FollowPosition {
                            target: position,
                            speed: 1.0,
                        });
                    }
                    let (targets, minted) = self.perceived_targets(
                        storage,
                        &record,
                        &proximity_policy(),
                        f64::from(*engagement_radius),
                        member_positions.get(&uuid_of(&record)),
                        &live_players,
                        plugin_id,
                        transaction_id,
                    );
                    plan.targets = targets;
                    refs.extend(minted);
                }
                ScriptResidentOrder::Move { .. } => {
                    let anchor = order_anchor(order).unwrap_or(Vec3::ZERO);
                    if let Some((slot, position)) =
                        self.formation_slot_at(order, anchor, index, count)
                    {
                        // The member must actually be able to walk there: a
                        // closed passage reports `blocked_route` and pushes no
                        // goal instead of teleporting.
                        let reachable =
                            member_positions.get(&uuid_of(&record)).is_none_or(|from| {
                                !self.route_blocked(order_dimension(order), *from, position)
                            });
                        if reachable {
                            plan.slot = Some(slot);
                            plan.goal = Some(GoalState::FollowPosition {
                                target: position,
                                speed: 1.0,
                            });
                        }
                    }
                }
                ScriptResidentOrder::Patrol {
                    waypoints,
                    engagement_radius,
                    ..
                } => {
                    let route_index = record
                        .order
                        .as_ref()
                        .map(|order| usize::from(order.route_index))
                        .unwrap_or(0)
                        % waypoints.len().max(1);
                    let anchor = waypoints
                        .get(route_index)
                        .map(block_position)
                        .unwrap_or(Vec3::ZERO);
                    if let Some((slot, position)) =
                        self.formation_slot_at(order, anchor, index, count)
                    {
                        plan.slot = Some(slot);
                        plan.goal = Some(GoalState::FollowPosition {
                            target: position,
                            speed: 1.0,
                        });
                    }
                    let (targets, minted) = self.perceived_targets(
                        storage,
                        &record,
                        &proximity_policy(),
                        f64::from(*engagement_radius),
                        member_positions.get(&uuid_of(&record)),
                        &live_players,
                        plugin_id,
                        transaction_id,
                    );
                    plan.targets = targets;
                    refs.extend(minted);
                }
                ScriptResidentOrder::Retreat { .. } => {
                    let anchor = order_anchor(order).unwrap_or(Vec3::ZERO);
                    if let Some((slot, position)) =
                        self.formation_slot_at(order, anchor, index, count)
                    {
                        plan.slot = Some(slot);
                        plan.goal = Some(GoalState::FollowPosition {
                            target: position,
                            speed: 1.2,
                        });
                    }
                }
                ScriptResidentOrder::Attack { targets, policy } => {
                    let (perceived, minted) = self.perceived_targets(
                        storage,
                        &record,
                        policy,
                        f64::from(mc_script::MAX_ENGAGEMENT_RADIUS),
                        member_positions.get(&uuid_of(&record)),
                        &live_players,
                        plugin_id,
                        transaction_id,
                    );
                    plan.targets = perceived;
                    refs.extend(minted);
                    let (attacks, refreshed) = self
                        .plan_member_attacks(
                            storage,
                            plugin_id,
                            next_records.get_mut(handle),
                            &record,
                            targets,
                            policy,
                            &references,
                            &live_players,
                            transaction_id,
                        )
                        .await;
                    plan.attacks = attacks;
                    refs.extend(refreshed);
                    if let Some(attack) = plan.attacks.first() {
                        plan.goal = Some(GoalState::FollowTarget {
                            target: mc_entity::EntityId(attack.expected.id.0),
                            speed: 1.1,
                        });
                        plan.slot = Some(u16::try_from(index).unwrap_or(u16::MAX));
                    } else {
                        plan.goal = Some(GoalState::Idle);
                        plan.slot = Some(u16::try_from(index).unwrap_or(u16::MAX));
                    }
                }
                // The closed order union is non-exhaustive to plugins.
                _ => {
                    plan.goal = Some(GoalState::Idle);
                    plan.slot = Some(u16::try_from(index).unwrap_or(u16::MAX));
                }
            }
            if matches!(order, ScriptResidentOrder::Move { .. }) && plan.slot.is_none() {
                plan.goal = None;
            }
            plans.push(plan);
        }
        // Perception issues every permitted candidate and attack resolution
        // refreshes the refs it resolved, so the same reference can be minted
        // twice in one batch; the durable ledger keys refs by (plugin, ref).
        refs.sort_unstable_by(|left, right| left.target_ref.cmp(&right.target_ref));
        refs.dedup_by(|left, right| left.target_ref == right.target_ref);
        (plans, refs)
    }

    /// Stable engine-computed position of one approved guard-post slot. The
    /// post is unknown, the slot is claimed by another member, the engine
    /// cannot stand there, or the member cannot walk there all yield `None`, so
    /// the caller reports `blocked_route` instead of stacking or teleporting.
    #[allow(clippy::too_many_arguments)]
    fn garrison_slot_position(
        &self,
        post: &str,
        slot: u16,
        resolved: &BTreeMap<String, (Vec3, u16)>,
        claims: &BTreeMap<(String, u16), String>,
        handle: &str,
        from: Option<&Vec3>,
        dimension: &str,
    ) -> Option<Vec3> {
        let (anchor, capacity) = resolved.get(post)?;
        if slot >= *capacity {
            return None;
        }
        if claims
            .get(&(post.to_owned(), slot))
            .is_some_and(|owner| owner != handle)
        {
            return None;
        }
        let position = garrison_position(*anchor, slot, *capacity)?;
        let cell = [
            position.x.floor() as i32,
            position.y.floor() as i32,
            position.z.floor() as i32,
        ];
        if !self
            .resident_world()?
            .standable(dimension, cell)
            .unwrap_or(false)
        {
            return None;
        }
        if let Some(from) = from
            && self.route_blocked(dimension, *from, position)
        {
            return None;
        }
        Some(position)
    }

    /// Distinct standable formation slot for one member, when the order builds
    /// one around `anchor`.
    fn formation_slot_at(
        &self,
        order: &ScriptResidentOrder,
        anchor: Vec3,
        index: usize,
        count: usize,
    ) -> Option<(u16, Vec3)> {
        let world = self.resident_world()?;
        let (formation, heading) = order_formation(order)?;
        match place_formation(
            formation,
            anchor,
            heading,
            count,
            order_dimension(order),
            world,
        ) {
            Some(FormationPlacement::Placed(placements)) => {
                placements.get(index).map(|placement| {
                    (
                        u16::try_from(placement.slot).unwrap_or(u16::MAX),
                        placement.position,
                    )
                })
            }
            _ => None,
        }
    }

    /// Server-issued target references for members already in local perception
    /// inside `radius`. Only the order policy's permitted categories are issued
    /// as targets, and every issued target's reference is minted here and
    /// persisted with the batch, so a forged reference can never be used later.
    #[allow(clippy::too_many_arguments)]
    fn perceived_targets(
        &self,
        storage: &PluginStorage,
        record: &DurableResidentOrderRecord,
        policy: &ScriptEngagementPolicy,
        radius: f64,
        position: Option<&Vec3>,
        live_players: &std::collections::BTreeSet<uuid::Uuid>,
        plugin_id: &str,
        transaction_id: u64,
    ) -> (Vec<ScriptOrderTarget>, Vec<DurableTargetRef>) {
        let Some(position) = position else {
            return (Vec::new(), Vec::new());
        };
        // Perception is the session's own bounded chunk index; the order policy
        // decides which of the categories it returns may be issued.
        let candidates = self
            .sessions()
            .resident_perception(*position, radius, &policy.permitted);
        let mut targets = Vec::new();
        let mut refs = Vec::new();
        for candidate in candidates {
            // Server PvP rules: a player is only a target while it is an
            // online actor, on top of the plugin's own policy.
            if candidate.category == ScriptHostileCategory::Player
                && !live_players.contains(&candidate.uuid)
            {
                continue;
            }
            // A member never targets itself, and an ally is never issued as a
            // target: a player ally is named by uuid, an owned resident ally by
            // its durable handle.
            let uuid = candidate.uuid.to_string();
            if uuid == record.entity_uuid
                || policy.allies.iter().any(|ally| {
                    ally == &uuid
                        || storage
                            .residents()
                            .record(ally)
                            .is_some_and(|resident| resident.entity_uuid == uuid)
                })
            {
                continue;
            }
            if !policy.permitted.contains(&candidate.category) {
                continue;
            }
            let target_ref = target_ref_for(&candidate.uuid);
            refs.push(DurableTargetRef {
                plugin_id: plugin_id.to_owned(),
                target_ref: target_ref.clone(),
                entity_uuid: uuid,
                category: candidate.category.as_str().to_owned(),
                policy_revision: policy.revision,
                revision: 0,
                expires_revision: transaction_id.saturating_add(TARGET_REF_TTL_REVISIONS),
            });
            targets.push(ScriptOrderTarget::new(
                target_ref,
                candidate.category,
                mc_script::ScriptBlockPosition::new(
                    candidate.position.x.floor() as i32,
                    candidate.position.y.floor() as i32,
                    candidate.position.z.floor() as i32,
                ),
            ));
        }
        (targets, refs)
    }

    /// Resolve one member's attack set against durable, server-issued target
    /// references: a forged or stale reference commits nothing, an archer
    /// without arrows or without line of sight deals no damage, and no policy
    /// ally is ever targeted.
    #[allow(clippy::too_many_arguments)]
    async fn plan_member_attacks(
        &self,
        storage: &PluginStorage,
        plugin_id: &str,
        next_record: Option<&mut DurableResidentOrderRecord>,
        record: &DurableResidentOrderRecord,
        targets: &[ScriptOrderTargetRef],
        policy: &ScriptEngagementPolicy,
        references: &BTreeMap<uuid::Uuid, &EntitySnapshot>,
        live_players: &std::collections::BTreeSet<uuid::Uuid>,
        transaction_id: u64,
    ) -> (Vec<PlannedAttack>, Vec<DurableTargetRef>) {
        let Some(world) = self.resident_world() else {
            return (Vec::new(), Vec::new());
        };
        let Some(next_record) = next_record else {
            return (Vec::new(), Vec::new());
        };
        let Some(policy) = target_policy(policy, f64::from(mc_script::MAX_ENGAGEMENT_RADIUS))
        else {
            return (Vec::new(), Vec::new());
        };
        let weapon = record
            .equipment
            .first()
            .and_then(|slot| slot.as_ref())
            .map(|stack| stack.item_id.clone());
        let ranged = weapon.as_deref() == Some(RESIDENT_BOW);
        let reach = if ranged {
            RESIDENT_BOW_REACH
        } else {
            RESIDENT_MELEE_REACH
        };
        if ranged && !gear_has(next_record, RESIDENT_ARROW) {
            // No ammo: the archer engages nothing and consumes nothing.
            return (Vec::new(), Vec::new());
        }
        let Some(attacker_position) = self
            .sessions()
            .resident_entity_snapshots(&[uuid_of(record)])
            .await
            .into_iter()
            .next()
            .flatten()
            .map(|snapshot| snapshot.position)
        else {
            return (Vec::new(), Vec::new());
        };
        let mut attacks = Vec::new();
        let mut minted = Vec::new();
        for target in targets {
            let Some(reference) = storage
                .resident_orders()
                .reference(plugin_id, &target.target_ref)
            else {
                // A reference this core never issued is rejected.
                continue;
            };
            if reference.expires_revision < storage.revision {
                continue;
            }
            let Ok(uuid) = uuid::Uuid::parse_str(&reference.entity_uuid) else {
                continue;
            };
            let Some(snapshot) = references.get(&uuid) else {
                continue;
            };
            let Some(category) = resident_category(&snapshot.type_name) else {
                continue;
            };
            if category == ScriptHostileCategory::Player && !live_players.contains(&uuid) {
                continue;
            }
            if !policy.permits(engine_category(category)) {
                continue;
            }
            // An allied resident is identified by its durable handle; the
            // candidate only carries the entity identity.
            let candidate_handle = storage
                .residents()
                .record_by_entity(&uuid.to_string())
                .map(|record| record.handle.as_str());
            if policy.is_allied(candidate_handle)
                || candidate_handle == Some(record.handle.as_str())
            {
                continue;
            }
            let distance = distance(&attacker_position, &snapshot.position);
            if distance > reach {
                continue;
            }
            let eye = Vec3::new(
                attacker_position.x,
                attacker_position.y + 1.5,
                attacker_position.z,
            );
            let target_eye = Vec3::new(
                snapshot.position.x,
                snapshot.position.y + 1.0,
                snapshot.position.z,
            );
            // A wall stops the hit: no damage through blocks.
            if !world
                .line_of_sight(RESIDENT_WORLD_DIMENSION, eye, target_eye)
                .unwrap_or(false)
            {
                continue;
            }
            let mut item_ledger = ItemLedger::default();
            if ranged {
                wear_gear_count(next_record, RESIDENT_ARROW, 1, &mut item_ledger);
            }
            let item_id = weapon
                .as_deref()
                .and_then(|weapon| codec::Identifier::parse(weapon.to_owned()).ok())
                .and_then(|name| self.items().id_of(&name));
            let amount = crate::play::resident_work::resident_weapon_damage(
                self.item_facts(),
                self.items(),
                item_id,
            );
            attacks.push(PlannedAttack {
                target_ref: reference.target_ref.clone(),
                uuid,
                expected: (*snapshot).clone(),
                amount,
            });
            minted.push(DurableTargetRef {
                plugin_id: plugin_id.to_owned(),
                target_ref: target.target_ref.clone(),
                entity_uuid: uuid.to_string(),
                category: category.as_str().to_owned(),
                policy_revision: policy.revision(),
                revision: 0,
                expires_revision: transaction_id.saturating_add(TARGET_REF_TTL_REVISIONS),
            });
        }
        attacks.sort_unstable_by(|left, right| left.target_ref.cmp(&right.target_ref));
        (attacks, minted)
    }

    async fn apply_order_plans(&self, plans: &[MemberPlan]) {
        let goals = plans
            .iter()
            .filter_map(|plan| {
                let uuid = uuid::Uuid::parse_str(&plan.entity_uuid).ok()?;
                let goal = plan.goal.clone()?;
                Some(ResidentGoal { uuid, goal })
            })
            .collect::<Vec<_>>();
        self.sessions().apply_resident_goals(goals).await;
    }

    /// Resolve the engine goal of one durable order, used by admission replay.
    fn resident_order_goal(&self, record: &DurableResidentOrderRecord) -> Option<GoalState> {
        let order = record.order.as_ref()?;
        match &order.order {
            ScriptResidentOrder::Hold { .. } => Some(GoalState::Idle),
            ScriptResidentOrder::Garrison { .. } => {
                // Return to the post the order already occupies; the durable slot
                // reconstructs the same engine-computed position after a reload.
                let slot = order.garrison.as_ref()?;
                let runtime = self.settlement_runtime()?;
                let resolved = runtime.guard_posts(std::slice::from_ref(&slot.post));
                let (anchor, capacity) = resolved.get(&slot.post)?;
                let position = garrison_position(*anchor, slot.slot, *capacity)?;
                Some(GoalState::FollowPosition {
                    target: position,
                    speed: 1.0,
                })
            }
            ScriptResidentOrder::Move { .. } | ScriptResidentOrder::Retreat { .. } => {
                let world = self.resident_world()?;
                goal_for_order(&order.order, world)
            }
            ScriptResidentOrder::Patrol { waypoints, .. } => {
                let waypoint = waypoints.get(usize::from(order.route_index))?;
                let world = self.resident_world()?;
                goal_for_position(*waypoint, world)
            }
            ScriptResidentOrder::Follow { .. } | ScriptResidentOrder::Attack { .. } => None,
            // The closed order union is non-exhaustive to plugins.
            _ => None,
        }
    }

    /// Project the canonical main-hand stack onto the resident's held item.
    async fn project_resident_held_item(&self, record: &DurableResidentOrderRecord) {
        let Ok(uuid) = uuid::Uuid::parse_str(&record.entity_uuid) else {
            return;
        };
        let held = record
            .equipment
            .first()
            .and_then(|slot| slot.as_ref())
            .and_then(|stack| entity_item_stack(self.items(), stack));
        self.sessions().set_resident_held_item(uuid, held).await;
    }

    /// Whether a member cannot walk between two positions: an absent world
    /// adapter, an unsimulated dimension, a blocked route or unloaded terrain.
    fn route_blocked(&self, dimension: &str, from: Vec3, to: Vec3) -> bool {
        let Some(world) = self.resident_world() else {
            return true;
        };
        if !world.dimension_loaded(dimension) {
            return true;
        }
        let cell = |position: Vec3| {
            [
                position.x.floor() as i32,
                position.y.floor() as i32,
                position.z.floor() as i32,
            ]
        };
        !world
            .route_open(dimension, cell(from), cell(to))
            .unwrap_or(false)
    }

    // ------------------------------------------------------------ plumbing

    fn commit_resident_order(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        payload: ScriptOperationPayload,
        changes: Vec<DurableResidentOrderChange>,
    ) -> Result<(), PluginStorageMutationError> {
        let batch = match storage
            .prepare_resident_order_operation_batch(plugin_id, request, payload, changes)?
        {
            ScriptStoragePrepareOutcome::Prepared(batch) => batch,
            ScriptStoragePrepareOutcome::Rejected => {
                return Err(PluginStorageMutationError::QuotaExceeded);
            }
        };
        storage.commit_batch(batch)
    }

    fn resident_order_receipt_outcome(
        &self,
        storage: &PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
    ) -> Option<ScriptOperationOutcome> {
        let operation_id = request.operation_id()?;
        storage
            .operation_receipt(plugin_id, operation_id)
            .map(|receipt| receipt.outcome.clone())
    }

    fn acknowledge_resident_admission(
        &self,
        storage: &mut PluginStorage,
        admission_id: u64,
        members: Vec<String>,
    ) {
        if members.is_empty() {
            return;
        }
        let change = DurableResidentOrderChange::AdmissionApplied {
            revision: 0,
            admission_id,
            members,
        };
        let _ = storage.append_resident_order_change(change);
    }
    /// One item's own stack size, so a deposit never creates an illegal stack.
    fn drop_max_stack(&self, item_id: &str) -> u32 {
        let Ok(name) = Identifier::parse(item_id.to_owned()) else {
            return 1;
        };
        let Some(id) = self.items().id_of(&name) else {
            return 1;
        };
        let stack = ItemStack::new(id, 1);
        let max = mc_data::item_semantics_26_1_2::max_stack_for_stack(
            self.item_facts(),
            self.items(),
            &stack,
        );
        u32::try_from(max.max(1)).unwrap_or(1)
    }

    /// Whether the worker's own slots can hold every drop of one break.
    ///
    /// Computed before the block is broken, so a worker who cannot hold the loot
    /// leaves the world alone instead of producing items nobody owns.
    fn drops_fit(&self, record: &DurableResidentOrderRecord, drops: &[ResidentDrop]) -> bool {
        let mut free = free_resident_slots(record);
        let mut needed: BTreeMap<&str, u64> = BTreeMap::new();
        for drop in drops {
            *needed.entry(drop.item_id.as_str()).or_default() += u64::from(drop.count);
        }
        for (item_id, count) in needed {
            let max_stack = u64::from(self.drop_max_stack(item_id));
            let room: u64 = record
                .carry
                .iter()
                .chain(record.equipment.iter())
                .flatten()
                .filter(|stack| mergeable_stack(stack, item_id))
                .map(|stack| max_stack.saturating_sub(u64::from(stack.count)))
                .sum();
            let mut remaining = count.saturating_sub(room);
            while remaining > 0 {
                if free == 0 {
                    return false;
                }
                free -= 1;
                remaining = remaining.saturating_sub(max_stack);
            }
        }
        true
    }

    /// Deposit canonical loot into the worker's own slots and report the receipt.
    ///
    /// Returns `false` when the drops do not fit; the caller then reports the typed
    /// storage pause and must not have broken the block yet.
    fn deposit_drops(
        &self,
        record: &mut DurableResidentOrderRecord,
        drops: &[ResidentDrop],
        ledger: &mut ItemLedger,
    ) -> bool {
        if !self.drops_fit(record, drops) {
            return false;
        }
        for drop in drops {
            let max_stack = self.drop_max_stack(&drop.item_id);
            if !put_resident_item(record, &drop.item_id, u64::from(drop.count), max_stack) {
                return false;
            }
            ledger.add(&drop.item_id, i64::from(drop.count));
        }
        true
    }
}

fn uuid_of(record: &DurableResidentOrderRecord) -> uuid::Uuid {
    uuid::Uuid::parse_str(&record.entity_uuid).unwrap_or(uuid::Uuid::nil())
}

/// Every entity identity one order names as an attack target.
fn order_reference_uuids(
    storage: &PluginStorage,
    plugin_id: &str,
    order: &ScriptResidentOrder,
) -> Vec<uuid::Uuid> {
    let ScriptResidentOrder::Attack { targets, .. } = order else {
        return Vec::new();
    };
    targets
        .iter()
        .filter_map(|target| {
            storage
                .resident_orders()
                .reference(plugin_id, &target.target_ref)
                .and_then(|reference| uuid::Uuid::parse_str(&reference.entity_uuid).ok())
        })
        .collect()
}

fn entity_item_stack(
    items: &ItemRegistry,
    stack: &DurableResidentStack,
) -> Option<EntityItemStack> {
    let name = codec::Identifier::parse(stack.item_id.clone()).ok()?;
    let item_id = items.id_of(&name)?;
    let mut entity_stack =
        EntityItemStack::new(item_id, i32::try_from(stack.count).unwrap_or(i32::MAX));
    entity_stack.damage = stack.damage;
    entity_stack.enchantments = stack
        .enchantments
        .iter()
        .filter_map(|enchantment| {
            let id = codec::Identifier::parse(enchantment.id.clone()).ok()?;
            Some(mc_data::ItemEnchantment {
                id,
                level: i32::from(enchantment.level),
            })
        })
        .collect();
    Some(entity_stack)
}

fn resident_category(type_name: &str) -> Option<ScriptHostileCategory> {
    if type_name == "minecraft:player" {
        return Some(ScriptHostileCategory::Player);
    }
    match mc_entity::natural_spawn_26_1_2::entity_type_facts(type_name)?.mob_category? {
        mc_data::entity_types::MobCategory::Monster => Some(ScriptHostileCategory::Hostile),
        _ => Some(ScriptHostileCategory::NeutralAnimal),
    }
}

fn engine_category(category: ScriptHostileCategory) -> mc_entity::TargetCategory {
    match category {
        ScriptHostileCategory::Hostile => mc_entity::TargetCategory::Hostile,
        ScriptHostileCategory::Player => mc_entity::TargetCategory::Player,
        ScriptHostileCategory::OwnedResident => mc_entity::TargetCategory::OwnedResident,
        ScriptHostileCategory::NeutralAnimal => mc_entity::TargetCategory::NeutralAnimal,
        _ => mc_entity::TargetCategory::Hostile,
    }
}

fn ok_combat_event(
    event_id: u64,
    revision: u64,
    attacker_handle: String,
    victim_target_ref: String,
    order_revision: u64,
    damage_milli: u32,
    killed: bool,
) -> Result<ScriptCombatEvent, mc_script::ScriptDtoError> {
    let event = ScriptCombatEvent::new(
        event_id,
        revision,
        attacker_handle,
        victim_target_ref,
        order_revision,
        damage_milli,
        killed,
    );
    event.validate()?;
    Ok(event)
}

fn distance(left: &Vec3, right: &Vec3) -> f64 {
    let dx = left.x - right.x;
    let dy = left.y - right.y;
    let dz = left.z - right.z;
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn request_fingerprint(request: &ScriptOperationRequest) -> [u8; 32] {
    super::resident_orders::resident_order_fingerprint(request.operation())
}

pub(super) fn resident_order_records(
    storage: &PluginStorage,
    plugin_id: &str,
    handles: &[String],
    order_revisions: &[u64],
) -> Option<BTreeMap<String, DurableResidentOrderRecord>> {
    if handles.len() != order_revisions.len() {
        return None;
    }
    let mut records = BTreeMap::new();
    for (handle, revision) in handles.iter().zip(order_revisions) {
        let record = resident_order_record(storage, plugin_id, handle)?;
        if record.order_revision() != *revision {
            return None;
        }
        records.insert(handle.clone(), record);
    }
    Some(records)
}

async fn observe_resident_members(
    sessions: &crate::play::SessionRegistry,
    records: &BTreeMap<String, DurableResidentOrderRecord>,
) -> BTreeMap<String, GroupMemberObservation> {
    let uuids = records
        .values()
        .filter_map(|record| uuid::Uuid::parse_str(&record.entity_uuid).ok())
        .collect::<Vec<_>>();
    let snapshots = sessions.resident_entity_snapshots(&uuids).await;
    let mut by_uuid = BTreeMap::new();
    for (uuid, snapshot) in uuids.iter().zip(snapshots) {
        by_uuid.insert(*uuid, snapshot);
    }
    let mut observations = BTreeMap::new();
    for (handle, record) in records {
        let Ok(uuid) = uuid::Uuid::parse_str(&record.entity_uuid) else {
            continue;
        };
        let Some(Some(snapshot)) = by_uuid.get(&uuid) else {
            continue;
        };
        let region =
            RegionKey::from_position(snapshot.position).unwrap_or(RegionKey { x: 0, z: 0 });
        observations.insert(
            handle.clone(),
            GroupMemberObservation {
                alive: snapshot.lifecycle == mc_entity::EntityLifecycle::Alive,
                loaded: true,
                region,
                order_revision: record.order_revision(),
            },
        );
    }
    observations
}

pub(super) async fn commit_resident_order_batch(
    sessions: &crate::play::SessionRegistry,
    records: &BTreeMap<String, DurableResidentOrderRecord>,
    admission: &mut GroupAdmission<String>,
) -> bool {
    // Re-fence at commit: a member that migrated, unloaded, died or changed its
    // order revision between prepare and commit invalidates the whole batch.
    let observations = observe_resident_members(sessions, records).await;
    admission.commit(&observations).is_ok()
}

/// Load one handle's durable order record. Identity comes from C3's resident
/// ledger; an unknown, foreign, dead or released handle has no order record.
pub(super) fn resident_order_record(
    storage: &PluginStorage,
    plugin_id: &str,
    handle: &str,
) -> Option<DurableResidentOrderRecord> {
    let resident = storage.residents().record(handle)?;
    if resident.plugin_id != plugin_id {
        return None;
    }
    let entity_uuid = resident.entity_uuid.clone();
    match storage.resident_orders().record(handle) {
        Some(record) if record.entity_uuid == entity_uuid => Some(record.clone()),
        Some(_) => None,
        None => Some(DurableResidentOrderRecord::empty(
            handle.to_owned(),
            plugin_id.to_owned(),
            entity_uuid,
            DurableAssignment::Civilian,
        )),
    }
}

/// Merge one accepted order into its member record. The order is logically
/// accepted for every fenced member; a member whose route is blocked reports
/// `blocked_route` and receives no engine goal.
fn member_order_record(
    record: DurableResidentOrderRecord,
    order: &ScriptResidentOrder,
    plan: &MemberPlan,
) -> DurableResidentOrderRecord {
    let mut next = record;
    let route_index = match order {
        ScriptResidentOrder::Patrol { waypoints, .. } => {
            let previous = next
                .order
                .as_ref()
                .map(|order| order.route_index)
                .unwrap_or(0);
            u16::try_from((usize::from(previous) + 1) % waypoints.len().max(1)).unwrap_or(0)
        }
        _ => 0,
    };
    next.order = Some(Box::new(DurableResidentOrder {
        revision: 0,
        order: order.clone(),
        route_index,
        garrison: plan.garrison.clone(),
    }));
    next.assignment = DurableAssignment::Military;
    next
}

fn durable_admission(
    admission_id: u64,
    plugin_id: &str,
    operation_id: &str,
    fingerprint: [u8; 32],
    records: &BTreeMap<String, DurableResidentOrderRecord>,
    plans: &[MemberPlan],
) -> DurableAdmission {
    // Members whose route is blocked need no engine push, so their application
    // is durable immediately; a replay never re-runs them.
    let blocked = plans
        .iter()
        .filter(|plan| plan.slot.is_none())
        .map(|plan| plan.handle.clone())
        .collect::<Vec<_>>();
    let members = records
        .iter()
        .map(|(handle, record)| DurableMemberFence {
            handle: handle.clone(),
            entity_uuid: record.entity_uuid.clone(),
            region: [0, 0],
            order_revision: record.order_revision(),
        })
        .collect();
    DurableAdmission {
        admission_id,
        plugin_id: plugin_id.to_owned(),
        operation_id: operation_id.to_owned(),
        fingerprint,
        order_revision: 0,
        committed: true,
        members,
        applied: blocked,
    }
}

fn order_member_state(plan: &MemberPlan) -> ScriptOrderMemberState {
    if plan.slot.is_some() {
        ScriptOrderMemberState::Applied
    } else {
        ScriptOrderMemberState::BlockedRoute
    }
}

pub(super) fn resident_order_batch_refusal(handles: &[String]) -> ScriptOperationOutcome {
    let mut members = handles
        .iter()
        .map(|handle| {
            ScriptOrderMemberOutcome::new(
                handle.clone(),
                ScriptOrderMemberState::StaleRevision,
                None,
                Vec::new(),
            )
        })
        .collect::<Vec<_>>();
    members.sort_unstable_by(|left, right| left.handle.cmp(&right.handle));
    members.dedup_by(|left, right| left.handle == right.handle);
    ScriptOperationOutcome::rejected_with_payload(
        ScriptOperationFailure::Blocked,
        ScriptOperationPayload::ResidentOrder {
            result: Box::new(ScriptResidentOrderResult::Order {
                order_revision: 0,
                members,
                combat: Vec::new(),
            }),
        },
    )
}

fn rejected(failure: ScriptOperationFailure) -> ScriptOperationOutcome {
    ScriptOperationOutcome::rejected(failure)
}

fn replay_resident_order(
    storage: &PluginStorage,
    plugin_id: &str,
    request: &ScriptOperationRequest,
    operation_id: &str,
) -> Option<ScriptOperationOutcome> {
    let fingerprint = request_fingerprint(request);
    storage
        .operation_receipt(plugin_id, operation_id)
        .map(|receipt| {
            if receipt.fingerprint == fingerprint {
                receipt.outcome.clone()
            } else {
                ScriptOperationOutcome::rejected(ScriptOperationFailure::OperationConflict)
            }
        })
}

// ------------------------------------------------------------- item plumbing

fn resource_item(value: &str) -> String {
    if value.contains(':') {
        value.to_owned()
    } else {
        format!("minecraft:{value}")
    }
}

fn gear_has(record: &DurableResidentOrderRecord, item_id: &str) -> bool {
    record
        .equipment
        .iter()
        .chain(&record.carry)
        .flatten()
        .any(|stack| stack.item_id == item_id && stack.count > 0)
}

fn gear_take(record: &mut DurableResidentOrderRecord, item_id: &str, want: u32) -> u32 {
    let mut remaining = want;
    let mut taken = 0;
    for slot in record.equipment.iter_mut().chain(record.carry.iter_mut()) {
        if remaining == 0 {
            break;
        }
        let Some(stack) = slot else {
            continue;
        };
        if stack.item_id != item_id {
            continue;
        }
        let count = remaining.min(stack.count);
        stack.count -= count;
        taken += count;
        remaining -= count;
        if stack.count == 0 {
            *slot = None;
        }
    }
    taken
}

/// Consume a counted item and record the negative delta.
fn wear_gear_count(
    record: &mut DurableResidentOrderRecord,
    item_id: &str,
    count: u32,
    ledger: &mut ItemLedger,
) -> u32 {
    let taken = gear_take(record, item_id, count);
    if taken > 0 {
        ledger.add(item_id, -i64::from(taken));
    }
    taken
}

fn put_resident_item(
    record: &mut DurableResidentOrderRecord,
    item_id: &str,
    count: u64,
    max_stack: u32,
) -> bool {
    if count == 0 {
        return true;
    }
    // Merge into an existing compatible stack first, then take free slots: the
    // canonical inventory never fragments one resource beyond what one item's
    // own stack size forces.
    let mut remaining = count;
    for slot in record
        .carry
        .iter_mut()
        .chain(record.equipment.iter_mut())
        .flatten()
    {
        if remaining == 0 {
            break;
        }
        if !mergeable_stack(slot, item_id) {
            continue;
        }
        let room = u64::from(max_stack.saturating_sub(slot.count));
        let placed = remaining.min(room);
        slot.count = slot
            .count
            .saturating_add(u32::try_from(placed).unwrap_or(u32::MAX));
        remaining -= placed;
    }
    while remaining > 0 {
        let placed = remaining.min(u64::from(max_stack.max(1)));
        let Some(slot) = record
            .carry
            .iter_mut()
            .chain(record.equipment.iter_mut())
            .find(|slot| slot.is_none())
        else {
            return false;
        };
        *slot = Some(DurableResidentStack::new(
            item_id.to_owned(),
            u32::try_from(placed).unwrap_or(u32::MAX),
        ));
        remaining -= placed;
    }
    true
}

/// Slots the worker can still fill: produced goods live in the same canonical
/// slots as its tools and gear, so capacity is one inventory.
fn free_resident_slots(record: &DurableResidentOrderRecord) -> usize {
    record
        .carry
        .iter()
        .chain(record.equipment.iter())
        .filter(|slot| slot.is_none())
        .count()
}

/// Whether one stack can absorb more of `item_id` without changing what it is.
///
/// Merging is by item identity *and* by components: a worn or enchanted stack
/// is never a destination for plain loot, and a named or modelled stack keeps
/// its own identity.
fn mergeable_stack(stack: &DurableResidentStack, item_id: &str) -> bool {
    stack.item_id == item_id
        && stack.damage.unwrap_or(0) == 0
        && stack.enchantments.is_empty()
        && stack.custom_name.is_none()
        && stack.item_model.is_none()
}

/// Wear one real tool by `ticks`; a tool that reaches its durability breaks and
/// leaves the resident's canonical slots.
fn wear_tool(
    record: &mut DurableResidentOrderRecord,
    item_id: &str,
    ledger: &mut ItemLedger,
    ticks: u32,
) {
    let Some(max_damage) = tool_max_damage(item_id) else {
        return;
    };
    for slot in record.equipment.iter_mut().chain(record.carry.iter_mut()) {
        let Some(stack) = slot else {
            continue;
        };
        if stack.item_id != item_id {
            continue;
        }
        let damage = stack
            .damage
            .unwrap_or(0)
            .saturating_add(i32::try_from(ticks).unwrap_or(0));
        if damage >= max_damage {
            ledger.add(item_id, -1);
            *slot = None;
        } else {
            stack.damage = Some(damage);
        }
        return;
    }
}

fn tool_max_damage(item_id: &str) -> Option<i32> {
    let path = item_id.strip_prefix("minecraft:").unwrap_or(item_id);
    mc_data::item_semantics_26_1_2::max_tool_damage_for_path(path)
}

fn take_ingredient(
    record: &mut DurableResidentOrderRecord,
    items: &ItemRegistry,
    tags: &mc_data::tags::TagsData,
    ingredient: &mc_data::recipes::Ingredient,
    ledger: &mut ItemLedger,
) -> Option<String> {
    let candidates = record
        .equipment
        .iter()
        .chain(&record.carry)
        .filter_map(|slot| slot.as_ref())
        .filter(|stack| stack.count > 0)
        .map(|stack| stack.item_id.clone())
        .collect::<Vec<_>>();
    for item_id in candidates {
        let Ok(name) = codec::Identifier::parse(item_id.clone()) else {
            continue;
        };
        let Some(item) = items.id_of(&name) else {
            continue;
        };
        if mc_data::recipes::ingredient_accepts_item(items, tags, item, ingredient) {
            let taken = gear_take(record, &item_id, 1);
            if taken == 1 {
                ledger.add(&item_id, -1);
                return Some(item_id);
            }
        }
    }
    None
}

fn recipe_ingredients(
    recipe: &mc_data::recipes::Recipe,
) -> Option<Vec<mc_data::recipes::Ingredient>> {
    match &recipe.kind {
        mc_data::recipes::RecipeKind::Shapeless(shapeless) => Some(shapeless.ingredients.clone()),
        mc_data::recipes::RecipeKind::Shaped(shaped) => {
            let mut ingredients = Vec::new();
            for row in &shaped.pattern {
                for character in row.chars().filter(|character| *character != ' ') {
                    ingredients.push(shaped.key.get(&character)?.clone());
                }
            }
            Some(ingredients)
        }
        mc_data::recipes::RecipeKind::Smelting(_)
        | mc_data::recipes::RecipeKind::Blasting(_)
        | mc_data::recipes::RecipeKind::Smoking(_)
        | mc_data::recipes::RecipeKind::CampfireCooking(_)
        | mc_data::recipes::RecipeKind::Stonecutting(_) => None,
    }
}

fn structure_hash(structure_id: &str) -> u64 {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(structure_id.as_bytes());
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(bytes)
}

fn target_ref_for(uuid: &uuid::Uuid) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(uuid.as_bytes());
    let mut hash = 0_u64;
    for byte in &digest[..8] {
        hash = (hash << 8) | u64::from(*byte);
    }
    format!("t{hash:016x}")
}

// ------------------------------------------------------------ work plumbing

fn work_cells(area: &mc_script::ScriptWorkArea) -> impl Iterator<Item = [i32; 3]> {
    let min = bounds_min(area);
    let max = bounds_max(area);
    (min[0]..=max[0]).flat_map(move |x| {
        (min[1]..=max[1]).flat_map(move |y| (min[2]..=max[2]).map(move |z| [x, y, z]))
    })
}

fn bounds_min(area: &mc_script::ScriptWorkArea) -> [i32; 3] {
    [area.min.x, area.min.y, area.min.z]
}

fn bounds_max(area: &mc_script::ScriptWorkArea) -> [i32; 3] {
    [area.max.x, area.max.y, area.max.z]
}

fn is_air_path(path: &str) -> bool {
    matches!(path, "air" | "cave_air" | "void_air")
}

fn is_harvestable_crop(path: &str) -> bool {
    matches!(
        path,
        "wheat"
            | "carrots"
            | "potatoes"
            | "beetroots"
            | "pumpkin"
            | "melon"
            | "sugar_cane"
            | "sweet_berry_bush"
            | "nether_wart"
            | "cocoa"
    )
}

fn crop_block_for_seed(seed: &str) -> Option<&'static str> {
    match seed {
        "minecraft:wheat_seeds" => Some("wheat"),
        "minecraft:carrot" => Some("carrots"),
        "minecraft:potato" => Some("potatoes"),
        "minecraft:beetroot_seeds" => Some("beetroots"),
        "minecraft:melon_seeds" => Some("melon_stem"),
        "minecraft:pumpkin_seeds" => Some("pumpkin_stem"),
        "minecraft:torchflower_seeds" => Some("torchflower_crop"),
        "minecraft:pitcher_pod" => Some("pitcher_crop"),
        _ => None,
    }
}

fn is_log_path(path: &str) -> bool {
    path.ends_with("_log")
        || path.ends_with("_wood")
        || path.ends_with("_stem")
        || path == "bamboo_block"
        || path == "mangrove_roots"
}

fn is_ground_path(path: &str) -> bool {
    matches!(
        path,
        "dirt"
            | "coarse_dirt"
            | "rooted_dirt"
            | "grass_block"
            | "podzol"
            | "mycelium"
            | "sand"
            | "red_sand"
            | "gravel"
            | "mud"
            | "moss_block"
            | "snow_block"
            | "clay"
            | "stone"
            | "deepslate"
            | "netherrack"
            | "soul_soil"
            | "crimson_nylium"
            | "warped_nylium"
    )
}

fn is_ore_path(path: &str) -> bool {
    path.ends_with("_ore") || path == "ancient_debris"
}

fn work_failure_reason(failure: ScriptOperationFailure) -> ScriptWorkPauseReason {
    match failure {
        ScriptOperationFailure::Unloaded => ScriptWorkPauseReason::Unloaded,
        ScriptOperationFailure::Forbidden => ScriptWorkPauseReason::Protected,
        ScriptOperationFailure::Blocked => ScriptWorkPauseReason::BlockedRoute,
        ScriptOperationFailure::Busy => ScriptWorkPauseReason::Interrupted,
        ScriptOperationFailure::Capacity => ScriptWorkPauseReason::Interrupted,
        ScriptOperationFailure::StaleRevision => ScriptWorkPauseReason::Interrupted,
        ScriptOperationFailure::RuntimeUnavailable => ScriptWorkPauseReason::Unsupported,
        ScriptOperationFailure::OperationConflict => ScriptWorkPauseReason::Interrupted,
        _ => ScriptWorkPauseReason::MissingInput,
    }
}

// --------------------------------------------------------- inventory slots

fn resident_endpoint_len(
    endpoint: &ScriptInventoryEndpoint,
    record: &DurableResidentOrderRecord,
) -> Option<usize> {
    match endpoint {
        ScriptInventoryEndpoint::ResidentEquipment { handle }
        | ScriptInventoryEndpoint::ResidentCarry { handle } => {
            if handle != &record.handle {
                return None;
            }
            Some(match endpoint {
                ScriptInventoryEndpoint::ResidentEquipment { .. } => record.equipment.len(),
                _ => record.carry.len(),
            })
        }
        _ => None,
    }
}

fn resident_slot(
    record: &DurableResidentOrderRecord,
    endpoint: &ScriptInventoryEndpoint,
    index: usize,
) -> Option<DurableResidentStack> {
    match endpoint {
        ScriptInventoryEndpoint::ResidentEquipment { .. } => {
            record.equipment.get(index).cloned().flatten()
        }
        ScriptInventoryEndpoint::ResidentCarry { .. } => record.carry.get(index).cloned().flatten(),
        _ => None,
    }
}

fn take_from_resident_slot(
    record: &mut DurableResidentOrderRecord,
    endpoint: &ScriptInventoryEndpoint,
    index: usize,
    count: u32,
) {
    let slot = match endpoint {
        ScriptInventoryEndpoint::ResidentEquipment { .. } => record.equipment.get_mut(index),
        ScriptInventoryEndpoint::ResidentCarry { .. } => record.carry.get_mut(index),
        _ => None,
    };
    let Some(slot) = slot else {
        return;
    };
    let Some(stack) = slot else {
        return;
    };
    stack.count = stack.count.saturating_sub(count);
    if stack.count == 0 {
        *slot = None;
    }
}

fn put_into_resident_slot(
    record: &mut DurableResidentOrderRecord,
    endpoint: &ScriptInventoryEndpoint,
    index: usize,
    stack: DurableResidentStack,
) -> bool {
    let slot = match endpoint {
        ScriptInventoryEndpoint::ResidentEquipment { .. } => record.equipment.get_mut(index),
        ScriptInventoryEndpoint::ResidentCarry { .. } => record.carry.get_mut(index),
        _ => None,
    };
    let Some(slot) = slot else {
        return false;
    };
    if slot.is_some() {
        return false;
    }
    *slot = Some(stack);
    true
}

// ------------------------------------------------------------ order plumbing

fn order_anchor(order: &ScriptResidentOrder) -> Option<Vec3> {
    match order {
        ScriptResidentOrder::Move { anchor, .. }
        | ScriptResidentOrder::Hold { anchor, .. }
        | ScriptResidentOrder::Retreat { anchor, .. } => Some(block_position(anchor)),
        ScriptResidentOrder::Patrol { waypoints, .. } => waypoints.first().map(block_position),
        _ => None,
    }
}

fn order_heading(order: &ScriptResidentOrder) -> Option<u16> {
    match order {
        ScriptResidentOrder::Move {
            heading_degrees, ..
        }
        | ScriptResidentOrder::Hold {
            heading_degrees, ..
        } => Some(*heading_degrees),
        _ => None,
    }
}

fn order_formation(
    order: &ScriptResidentOrder,
) -> Option<(mc_script::ScriptFormation, Option<u16>)> {
    match order {
        ScriptResidentOrder::Move { formation, .. }
        | ScriptResidentOrder::Hold { formation, .. }
        | ScriptResidentOrder::Patrol { formation, .. }
        | ScriptResidentOrder::Retreat { formation, .. }
        | ScriptResidentOrder::Follow { formation, .. } => Some((*formation, order_heading(order))),
        _ => None,
    }
}

fn order_dimension(order: &ScriptResidentOrder) -> &str {
    match order {
        ScriptResidentOrder::Move { dimension, .. } => dimension,
        _ => RESIDENT_WORLD_DIMENSION,
    }
}

fn goal_for_order(order: &ScriptResidentOrder, world: &dyn ResidentWorld) -> Option<GoalState> {
    match order {
        ScriptResidentOrder::Move { anchor, .. }
        | ScriptResidentOrder::Hold { anchor, .. }
        | ScriptResidentOrder::Retreat { anchor, .. } => goal_for_position(*anchor, world),
        ScriptResidentOrder::Patrol { waypoints, .. } => waypoints
            .first()
            .and_then(|anchor| goal_for_position(*anchor, world)),
        ScriptResidentOrder::Follow { .. }
        | ScriptResidentOrder::Garrison { .. }
        | ScriptResidentOrder::Attack { .. } => None,
        // The closed order union is non-exhaustive to plugins.
        _ => None,
    }
}

fn goal_for_position(
    anchor: mc_script::ScriptBlockPosition,
    world: &dyn ResidentWorld,
) -> Option<GoalState> {
    let position = block_position(&anchor);
    if world
        .standable(
            RESIDENT_WORLD_DIMENSION,
            [
                position.x.floor() as i32,
                position.y.floor() as i32,
                position.z.floor() as i32,
            ],
        )
        .unwrap_or(false)
    {
        Some(GoalState::FollowPosition {
            target: position,
            speed: 1.0,
        })
    } else {
        None
    }
}

fn block_position(position: &mc_script::ScriptBlockPosition) -> Vec3 {
    Vec3::new(
        f64::from(position.x) + 0.5,
        f64::from(position.y),
        f64::from(position.z) + 0.5,
    )
}

/// Stable engine-computed position of one guard-post slot. The slot index is
/// the same engine formation primitive a squad uses, so the same post and slot
/// reconstruct the identical coordinate across updates and a reload.
fn garrison_position(anchor: Vec3, slot: u16, capacity: u16) -> Option<Vec3> {
    let count = usize::from(capacity).min(mc_script::MAX_RESIDENT_ORDER_HANDLES);
    if count == 0 {
        return None;
    }
    let slots = FormationSlots::compute(FormationKind::Line, anchor, 0.0, 1.0, count)?;
    slots.slot(usize::from(slot))
}
