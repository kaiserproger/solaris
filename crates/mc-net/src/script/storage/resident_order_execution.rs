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
    ScriptAxisAlignedZone, ScriptCombatEvent, ScriptDemobilizeResult, ScriptDemobilizeState,
    ScriptEngagementPolicy, ScriptHostileCategory, ScriptInventoryEndpoint, ScriptOperation,
    ScriptOperationFailure, ScriptOperationOutcome, ScriptOperationPayload, ScriptOperationRequest,
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
use crate::play::resident_work::{
    RESIDENT_WORLD_DIMENSION, ResidentDrop, ResidentWorld, ResidentWorldEdit,
};
use crate::play::{MAX_BLOCK_EDIT_COMMAND_EDITS, ResidentAttack, ResidentGoal};

/// One core-scheduled work resumption whose durable outcome must reach the
/// owning plugin after the triggering world edit has become visible.
pub(super) struct NativeResidentWorkResume {
    pub(super) plugin_id: String,
    pub(super) request: ScriptOperationRequest,
    pub(super) outcome: ScriptOperationOutcome,
}

/// Attack reach of one resident melee, in blocks.
const RESIDENT_MELEE_REACH: f64 = 3.0;
/// Attack reach of one resident bow shot, in blocks.
const RESIDENT_BOW_REACH: f64 = 16.0;
const RESIDENT_ATTACK_COOLDOWN_TICKS: u64 = 20;
/// Deterministic resident-fishing output. This is a bounded worker abstraction,
/// not a claim to simulate vanilla fishing loot or timing.
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
    shooter: EntitySnapshot,
    expected: EntitySnapshot,
    amount: f32,
    ranged: bool,
}

/// One member's resolved execution plan inside an accepted batch.
struct MemberPlan {
    handle: String,
    entity_uuid: String,
    slot: Option<u16>,
    goal: Option<GoalState>,
    targets: Vec<ScriptOrderTarget>,
    attacks: Vec<PlannedAttack>,
    active_target_ref: Option<String>,
    /// Approved guard post this member occupies, for a garrison order.
    garrison: Option<DurableGarrisonSlot>,
    observed_health: Option<f32>,
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
            ScriptResidentOrderOperation::Capture {
                handle,
                custodian,
                expected_revision,
                ..
            } => {
                self.capture_resident(
                    storage,
                    plugin_id,
                    request,
                    handle,
                    custodian,
                    *expected_revision,
                )
                .await
            }
            // The closed operation union is non-exhaustive to plugins.
            _ => Ok(rejected(ScriptOperationFailure::InvalidRequest)),
        }
    }

    /// Replay every committed group admission whose members still owe an engine
    /// goal push. Called once when the storage actor starts.
    pub(crate) async fn recover_resident_orders(
        &self,
        storage: &mut PluginStorage,
    ) -> Result<(), PluginStorageMutationError> {
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
            storage.append_resident_order_change(change)?;
        }
        self.advance_active_morale(storage).await
    }
    /// Continue accepted attacks when a pushed simulation tick finds an active
    /// guard ready. The guest never polls or submits a damage command. A target
    /// reference is trusted here only if it was authenticated at admission;
    /// current owner, live actors, policy, weapon, reach and LOS are rechecked.
    pub(crate) async fn continue_active_resident_combat(
        &self,
        storage: &mut PluginStorage,
        tick: u64,
    ) -> Result<(), PluginStorageMutationError> {
        self.advance_active_morale(storage).await?;
        let ready = storage
            .resident_orders()
            .active_attack_records(tick, RESIDENT_ATTACK_COOLDOWN_TICKS);
        for mut record in ready {
            if !storage
                .residents()
                .record(&record.handle)
                .is_some_and(|resident| {
                    resident.plugin_id == record.plugin_id
                        && resident.entity_uuid == record.entity_uuid
                })
            {
                continue;
            }
            let Some(order) = record.order.as_ref() else {
                continue;
            };
            let ScriptResidentOrder::Attack { targets, policy } = &order.order else {
                continue;
            };
            let Some(target) = order
                .active_target_ref
                .as_ref()
                .and_then(|approved| targets.iter().find(|target| &target.target_ref == approved))
            else {
                continue;
            };
            let Some(reference) = storage
                .resident_orders()
                .reference(&record.plugin_id, &target.target_ref)
            else {
                continue;
            };
            let Ok(uuid) = uuid::Uuid::parse_str(&reference.entity_uuid) else {
                continue;
            };
            let Some(snapshot) = self
                .sessions()
                .resident_entity_snapshots(&[uuid])
                .await
                .into_iter()
                .next()
                .flatten()
            else {
                continue;
            };
            let references = BTreeMap::from([(uuid, &snapshot)]);
            let (attacks, _) = self
                .plan_member_attacks(
                    storage,
                    &record.plugin_id,
                    &record,
                    std::slice::from_ref(target),
                    policy,
                    &references,
                    &self.sessions().resident_live_player_uuids(),
                    storage.revision.saturating_add(1),
                    tick,
                    true,
                )
                .await;
            let Some(attack) = attacks.into_iter().next() else {
                continue;
            };
            let applied = if attack.ranged {
                self.sessions()
                    .launch_resident_arrow(&attack.shooter, &attack.expected)
            } else {
                self.sessions()
                    .commit_resident_damage(
                        &record.plugin_id,
                        vec![ResidentAttack {
                            uuid: attack.uuid,
                            amount: attack.amount,
                            expected: attack.expected,
                        }],
                    )
                    .await
                    .into_iter()
                    .next()
                    .flatten()
                    .is_some_and(|hit| hit.damage > 0.0)
            };
            if !applied {
                continue;
            }
            if attack.ranged && gear_take(&mut record, RESIDENT_ARROW, 1) != 1 {
                return Err(PluginStorageMutationError::QuotaExceeded);
            }
            record.last_attack_tick = Some(tick);
            storage.append_resident_order_change(DurableResidentOrderChange::CombatProgress {
                record: Box::new(record),
            })?;
        }
        Ok(())
    }

    /// Resume the same durable assignments whose bounded work regions received
    /// a real world event. One event gets one attempt per matching assignment;
    /// a still-missing prerequisite simply remains paused until another event.
    pub(super) async fn resume_paused_work_for_chunks(
        &self,
        storage: &mut PluginStorage,
        dimension: &str,
        chunks: &[[i32; 2]],
        plugin_is_active: impl Fn(&str) -> bool,
    ) -> Result<Vec<NativeResidentWorkResume>, PluginStorageMutationError> {
        let candidates = storage
            .resident_orders()
            .paused_records_for_chunks(dimension, chunks);
        self.resume_paused_work_records(storage, candidates, plugin_is_active)
            .await
    }

    /// Resume protected work only when the zone definition that overlaps its
    /// bounded work region changed.
    pub(super) async fn resume_paused_work_for_zone(
        &self,
        storage: &mut PluginStorage,
        zone: &ScriptAxisAlignedZone,
        plugin_is_active: impl Fn(&str) -> bool,
    ) -> Result<Vec<NativeResidentWorkResume>, PluginStorageMutationError> {
        let candidates = storage.resident_orders().paused_records_for_zone(zone);
        self.resume_paused_work_records(storage, candidates, plugin_is_active)
            .await
    }

    /// Resume tool/input waits for addressed residents and no-storage hauls
    /// whose exact warehouse destination changed in the same transfer.
    pub(super) async fn resume_paused_work_for_inventory_change(
        &self,
        storage: &mut PluginStorage,
        handles: &[String],
        endpoints: &[ScriptInventoryEndpoint],
        plugin_is_active: impl Fn(&str) -> bool,
    ) -> Result<Vec<NativeResidentWorkResume>, PluginStorageMutationError> {
        let candidates = storage
            .resident_orders()
            .paused_records_for_inventory_change(handles, endpoints);
        self.resume_paused_work_records(storage, candidates, plugin_is_active)
            .await
    }

    async fn resume_paused_work_records(
        &self,
        storage: &mut PluginStorage,
        candidates: Vec<DurableResidentOrderRecord>,
        plugin_is_active: impl Fn(&str) -> bool,
    ) -> Result<Vec<NativeResidentWorkResume>, PluginStorageMutationError> {
        let mut resumed = Vec::with_capacity(candidates.len());
        for record in candidates {
            let Some(work) = record.work.as_ref() else {
                continue;
            };
            if work.state != ScriptWorkState::Paused {
                continue;
            }
            if !plugin_is_active(&record.plugin_id) {
                continue;
            }
            let operation_id = format!("native-resume-{}", record.revision);
            let request = ScriptOperationRequest::try_new(
                &operation_id,
                ScriptOperation::ResidentOrder {
                    operation: ScriptResidentOrderOperation::AssignWork {
                        operation_id: operation_id.clone(),
                        handle: record.handle.clone(),
                        work: work.work.clone(),
                        work_units: work.planned,
                        expected_revision: record.revision,
                    },
                },
            )
            .expect("durable resident work remains a valid native resume request");
            let outcome = self
                .execute_resident_order_operation(storage, &record.plugin_id, &request)
                .await?;
            resumed.push(NativeResidentWorkResume {
                plugin_id: record.plugin_id,
                request,
                outcome,
            });
        }
        Ok(resumed)
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
        let mut staged_edits = Vec::new();
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
                &mut staged_edits,
            )
            .await;
        let world_input_wait = source_missing_input_wait(&next, work, reason);
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
                    world_input_wait,
                }));
                let payload = payload(assignment(done, state, reason, ledger));
                if staged_edits.is_empty() {
                    self.commit_resident_order(
                        storage,
                        plugin_id,
                        request,
                        payload,
                        vec![DurableResidentOrderChange::Record {
                            record: Box::new(next),
                        }],
                    )?;
                } else {
                    let dimension = match work {
                        ScriptResidentWorkOrder::Harvest { area, .. }
                        | ScriptResidentWorkOrder::Replant { area, .. }
                        | ScriptResidentWorkOrder::CutTree { area, .. }
                        | ScriptResidentWorkOrder::Mine { area, .. } => area.dimension.as_str(),
                        _ => unreachable!("only world work stages a world decision"),
                    };
                    let batch = match storage.prepare_resident_order_operation_batch(
                        plugin_id,
                        request,
                        payload,
                        vec![DurableResidentOrderChange::Record {
                            record: Box::new(next),
                        }],
                    )? {
                        ScriptStoragePrepareOutcome::Prepared(batch) => batch,
                        ScriptStoragePrepareOutcome::Rejected => {
                            return Err(PluginStorageMutationError::QuotaExceeded);
                        }
                    };
                    let commit = self
                        .commit_prepared_resident_edits(
                            storage,
                            plugin_id,
                            dimension,
                            &staged_edits,
                            batch,
                        )
                        .await;
                    match commit? {
                        super::world_inventory::PreparedResidentEditCommit::Committed => {}
                        super::world_inventory::PreparedResidentEditCommit::Refused(failure) => {
                            return Ok(rejected(failure));
                        }
                    }
                }
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
                    world_input_wait,
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
                        world_input_wait: false,
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
            cancelled.world_input_wait = false;
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
        staged_edits: &mut Vec<ResidentWorldEdit>,
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
                    if done - already_done >= remaining
                        || staged_edits.len() >= MAX_BLOCK_EDIT_COMMAND_EDITS
                    {
                        break;
                    }
                    let Some(block) = world.block(&area.dimension, cell) else {
                        return (done, Some(ScriptWorkPauseReason::Unloaded));
                    };
                    if !is_harvestable_crop(&block.path) {
                        continue;
                    }
                    if !world.crop_is_mature(block.state) {
                        continue;
                    }
                    if staged_edits.first().is_some_and(|first| {
                        !same_resident_work_region(
                            [
                                first.precondition.pos.x,
                                first.precondition.pos.y,
                                first.precondition.pos.z,
                            ],
                            cell,
                        )
                    }) {
                        return (done, None);
                    }
                    let preview = match world.preview_break(
                        &area.dimension,
                        cell,
                        block.state,
                        Some(&tool),
                    ) {
                        Ok(preview) => preview,
                        Err(failure) => {
                            return (done, Some(work_failure_reason(failure)));
                        }
                    };
                    // The worker must be able to hold the loot *before* the
                    // crop leaves the world: a full worker leaves the field
                    // standing instead of harvesting into nothing.
                    if !self.drops_fit(record, &preview.drops) {
                        let reason = if done == already_done {
                            ScriptWorkPauseReason::NoStorage
                        } else {
                            return (done, None);
                        };
                        return (done, Some(reason));
                    }
                    if !self.deposit_drops(record, &preview.drops, ledger) {
                        return (done, Some(ScriptWorkPauseReason::NoStorage));
                    }
                    wear_tool(record, &tool, ledger, 1);
                    staged_edits.push(preview);
                    done += 1;
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
                if world.foreign_zone_overlaps(
                    plugin_id,
                    &area.dimension,
                    bounds_min(area),
                    bounds_max(area),
                ) {
                    return (already_done, Some(ScriptWorkPauseReason::Protected));
                }
                let Some(crop) = crop_block_for_seed(&seed_item) else {
                    return (already_done, Some(ScriptWorkPauseReason::Unsupported));
                };
                let Some(crop_state) = world.state_for(crop) else {
                    return (already_done, Some(ScriptWorkPauseReason::Unsupported));
                };
                let mut done = already_done;
                for cell in work_cells(area) {
                    if done - already_done >= remaining
                        || staged_edits.len() >= MAX_BLOCK_EDIT_COMMAND_EDITS
                        || !gear_has(record, &seed_item)
                    {
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
                    let preview = match world.preview_place(&area.dimension, above, crop_state) {
                        Ok(preview) => preview,
                        Err(failure) => {
                            return (done, Some(work_failure_reason(failure)));
                        }
                    };
                    if staged_edits.first().is_some_and(|first| {
                        !same_resident_work_region(
                            [
                                first.precondition.pos.x,
                                first.precondition.pos.y,
                                first.precondition.pos.z,
                            ],
                            above,
                        )
                    }) {
                        return (done, None);
                    }
                    // The seed leaves the canonical slot in the same committed
                    // record and receipt-bearing world decision as the crop.
                    wear_gear_count(record, &seed_item, 1, ledger);
                    staged_edits.push(preview);
                    done += 1;
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
                    if done - already_done >= remaining
                        || staged_edits.len() >= MAX_BLOCK_EDIT_COMMAND_EDITS
                    {
                        break;
                    }
                    let Some(block) = world.block(&area.dimension, cell) else {
                        return (done, Some(ScriptWorkPauseReason::Unloaded));
                    };
                    if !is_log_path(&block.path) {
                        continue;
                    }
                    // A log must belong to a bounded rooted trunk with a real
                    // canopy. That admits the fixture's natural tree without
                    // treating every grounded log column as harvestable wood.
                    match rooted_tree_with_canopy(world, &area.dimension, cell) {
                        Ok(true) => {}
                        Ok(false) => continue,
                        Err(reason) => return (done, Some(reason)),
                    }
                    // Do not let a work receipt teleport a logger into its
                    // target. A real, bounded route must reach a standable
                    // neighbour of this trunk cell.
                    match self
                        .tree_cell_reachable(world, &area.dimension, record, cell)
                        .await
                    {
                        Ok(true) => {}
                        Ok(false) => return (done, Some(ScriptWorkPauseReason::BlockedRoute)),
                        Err(reason) => return (done, Some(reason)),
                    }
                    if staged_edits.first().is_some_and(|first| {
                        !same_resident_work_region(
                            [
                                first.precondition.pos.x,
                                first.precondition.pos.y,
                                first.precondition.pos.z,
                            ],
                            cell,
                        )
                    }) {
                        return (done, None);
                    }
                    let preview = match world.preview_break(
                        &area.dimension,
                        cell,
                        block.state,
                        Some(&tool),
                    ) {
                        Ok(preview) => preview,
                        Err(failure) => {
                            return (done, Some(work_failure_reason(failure)));
                        }
                    };
                    if !self.drops_fit(record, &preview.drops) {
                        if done == already_done {
                            return (done, Some(ScriptWorkPauseReason::NoStorage));
                        }
                        return (done, None);
                    }
                    if !self.deposit_drops(record, &preview.drops, ledger) {
                        return (done, Some(ScriptWorkPauseReason::NoStorage));
                    }
                    wear_tool(record, &tool, ledger, 1);
                    staged_edits.push(preview);
                    done += 1;
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
                    if done - already_done >= remaining
                        || staged_edits.len() >= MAX_BLOCK_EDIT_COMMAND_EDITS
                    {
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
                    // A mine order names a bounded area, not permission to
                    // extract unseen ore through solid terrain.
                    match self
                        .mine_cell_reachable(world, &area.dimension, record, cell)
                        .await
                    {
                        Ok(true) => {}
                        Ok(false) => {
                            return (done, Some(ScriptWorkPauseReason::BlockedRoute));
                        }
                        Err(reason) => return (done, Some(reason)),
                    }
                    if staged_edits.first().is_some_and(|first| {
                        !same_resident_work_region(
                            [
                                first.precondition.pos.x,
                                first.precondition.pos.y,
                                first.precondition.pos.z,
                            ],
                            cell,
                        )
                    }) {
                        return (done, None);
                    }
                    let preview = match world.preview_break(
                        &area.dimension,
                        cell,
                        block.state,
                        Some(&tool),
                    ) {
                        Ok(preview) => preview,
                        Err(failure) => {
                            return (done, Some(work_failure_reason(failure)));
                        }
                    };
                    if preview.drops.is_empty() {
                        return (done, Some(ScriptWorkPauseReason::MissingTool));
                    }
                    if !self.drops_fit(record, &preview.drops) {
                        if done == already_done {
                            return (done, Some(ScriptWorkPauseReason::NoStorage));
                        }
                        return (done, None);
                    }
                    if !self.deposit_drops(record, &preview.drops, ledger) {
                        return (done, Some(ScriptWorkPauseReason::NoStorage));
                    }
                    wear_tool(record, &tool, ledger, 1);
                    staged_edits.push(preview);
                    done += 1;
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
                let Some(from) = self.resident_work_cell(record).await else {
                    return (already_done, Some(ScriptWorkPauseReason::BlockedRoute));
                };
                let mut columns = std::collections::BTreeSet::new();
                let mut has_water = false;
                for cell in work_cells(area) {
                    let Some(block) = world.block(&area.dimension, cell) else {
                        return (already_done, Some(ScriptWorkPauseReason::Unloaded));
                    };
                    if block.path != "water" {
                        continue;
                    }
                    has_water = true;
                    let column = (cell[0], cell[2]);
                    if columns.contains(&column) {
                        continue;
                    }
                    match self.fishing_cell_reachable(world, &area.dimension, from, cell) {
                        Ok(true) => {
                            columns.insert(column);
                        }
                        Ok(false) => {}
                        Err(reason) => return (already_done, Some(reason)),
                    }
                }
                if !has_water {
                    return (already_done, Some(ScriptWorkPauseReason::MissingInput));
                }
                if columns.is_empty() {
                    return (already_done, Some(ScriptWorkPauseReason::BlockedRoute));
                }
                // One deterministic catch per physically reachable water column.
                // This is a bounded worker abstraction, not vanilla fishing loot
                // or timing simulation.
                let catchable = u64::try_from(columns.len()).unwrap_or(0).min(remaining);
                let catch_id = resource_item(RESIDENT_FISHING_CATCH);
                let mut caught = 0_u64;
                for _ in 0..catchable {
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
                let Some(feed_id) = Identifier::parse(feed_item.clone())
                    .ok()
                    .and_then(|name| self.items().id_of(&name))
                else {
                    return (already_done, Some(ScriptWorkPauseReason::MissingInput));
                };
                let feed_targets = crate::play::AnimalFeedTargets::from_tags(
                    &mc_data::tags::solaris_required_item_tags(self.items()),
                    feed_id,
                );
                if feed_targets.is_empty() {
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
                let animals = self
                    .sessions()
                    .resident_perception(center, radius, &[ScriptHostileCategory::NeutralAnimal])
                    .into_iter()
                    .filter(|animal| {
                        feed_targets.accepts(&animal.type_name)
                            && animal
                                .animal
                                .is_some_and(mc_entity::AnimalBreedingState::can_fall_in_love)
                    })
                    .collect::<Vec<_>>();
                if animals.is_empty() {
                    return (already_done, Some(ScriptWorkPauseReason::MissingInput));
                }
                let Some(from) = self.resident_work_cell(record).await else {
                    return (already_done, Some(ScriptWorkPauseReason::BlockedRoute));
                };
                let mut reachable = 0_u64;
                for animal in animals {
                    match self.animal_cell_reachable(world, &area.dimension, from, animal.position)
                    {
                        Ok(true) => reachable += 1,
                        Ok(false) => {}
                        Err(reason) => return (already_done, Some(reason)),
                    }
                }
                if reachable == 0 {
                    return (already_done, Some(ScriptWorkPauseReason::BlockedRoute));
                }
                let mut done = already_done;
                for _ in 0..reachable {
                    if done - already_done >= remaining || !gear_has(record, &feed_item) {
                        break;
                    }
                    // Tending consumes resident-owned feed only for a live,
                    // mature, compatible animal the resident can reach; it
                    // never fabricates livestock goods.
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
            ScriptResidentWorkOrder::Craft {
                recipe,
                count,
                station,
            } => {
                let recipes = mc_data::recipes::solaris_required_recipes();
                let Some(recipe) = recipes
                    .iter()
                    .find(|candidate| candidate.id.as_str() == recipe)
                else {
                    return (already_done, Some(ScriptWorkPauseReason::Unsupported));
                };
                let Some(required_station) = resident_recipe_station(recipe) else {
                    // Furnace-backed recipes consume fuel and preserve burn progress
                    // in the live furnace. A worker craft cannot replace that state
                    // machine with a free instantaneous conversion.
                    return (already_done, Some(ScriptWorkPauseReason::Unsupported));
                };
                if world.foreign_zone_overlaps(
                    plugin_id,
                    &station.dimension,
                    bounds_min(station),
                    bounds_max(station),
                ) {
                    return (already_done, Some(ScriptWorkPauseReason::Protected));
                }
                let Some(block) = world.block(&station.dimension, bounds_min(station)) else {
                    return (already_done, Some(ScriptWorkPauseReason::Unloaded));
                };
                if block.path != required_station
                    || (required_station == "campfire" && !block.is_lit_campfire)
                {
                    return (already_done, Some(ScriptWorkPauseReason::MissingStation));
                }
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

    /// Craft one registry recipe from the resident's canonical slots. Inputs,
    /// output, and recipe remainders become one resident-record after-image, so
    /// a missing input or recipient capacity commits no partial craft.
    fn craft_resident_items(
        &self,
        record: &mut DurableResidentOrderRecord,
        recipe: &mc_data::recipes::Recipe,
        count: u32,
        limit: u64,
        ledger: &mut ItemLedger,
    ) -> (u64, Option<ScriptWorkPauseReason>) {
        let Some(ingredients) = recipe_ingredients(recipe) else {
            return (0, Some(ScriptWorkPauseReason::Unsupported));
        };
        if ingredients.is_empty() {
            return (0, Some(ScriptWorkPauseReason::Unsupported));
        }
        let tags = mc_data::tags::solaris_required_item_tags(self.items());
        if self.item_facts().custom(&recipe.result.item).is_some() {
            return (0, Some(ScriptWorkPauseReason::Unsupported));
        }
        let Some(result) = recipe.result.to_stack(self.items(), self.item_facts()) else {
            return (0, Some(ScriptWorkPauseReason::Unsupported));
        };
        let Some(result_name) = self.items().name_of(result.item_id) else {
            return (0, Some(ScriptWorkPauseReason::Unsupported));
        };
        let result_name = result_name.as_str().to_owned();
        let mut done = 0_u64;
        while done < u64::from(count) && done < limit {
            // Work on a complete after-image. Component-bearing inputs must be
            // restored exactly as read when output or remainder capacity fails.
            let mut crafted = record.clone();
            let mut taken: Vec<String> = Vec::new();
            let mut satisfied = true;
            for ingredient in &ingredients {
                match take_ingredient(&mut crafted, self.items(), &tags, ingredient) {
                    Some(item_id) => taken.push(item_id),
                    None => {
                        satisfied = false;
                        break;
                    }
                }
            }
            if !satisfied {
                break;
            }
            let mut outputs = vec![(
                result_name.clone(),
                u64::try_from(result.count).unwrap_or(0),
            )];
            outputs.extend(taken.iter().filter_map(|item_id| {
                crafting_remainder(self.item_facts(), item_id).map(|remainder| (remainder, 1))
            }));
            let outputs_fit = outputs.iter().all(|(item_id, amount)| {
                put_resident_item(&mut crafted, item_id, *amount, self.drop_max_stack(item_id))
            });
            if !outputs_fit {
                return (done, Some(ScriptWorkPauseReason::NoStorage));
            }
            *record = crafted;
            for item_id in taken {
                ledger.add(&item_id, -1);
            }
            for (item_id, amount) in outputs {
                ledger.add(&item_id, i64::try_from(amount).unwrap_or(i64::MAX));
            }
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
        if matches!(order, ScriptResidentOrder::Attack { .. })
            && records.values().any(|record| {
                record.morale.as_ref().is_some_and(|morale| {
                    matches!(
                        morale.phase,
                        super::resident_morale::MoralePhase::Routing
                            | super::resident_morale::MoralePhase::Surrendered
                    )
                })
            })
        {
            return Ok(resident_order_batch_refusal(handles));
        }
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
        let attack_tick = self.sessions().simulation_tick();
        let (mut plans, refs) = self
            .plan_member_orders(
                storage,
                plugin_id,
                &mut next_records,
                order,
                transaction_id,
                attack_tick,
            )
            .await;
        // A melee receipt records only committed damage. A bow shot launches
        // one world projectile; impact damage belongs to the arrow kernel.
        let attacks = plans
            .iter()
            .flat_map(|plan| plan.attacks.iter())
            .filter(|attack| !attack.ranged)
            .map(|attack| ResidentAttack {
                uuid: attack.uuid,
                amount: attack.amount,
                expected: attack.expected.clone(),
            })
            .collect::<Vec<_>>();
        let mut hits = self
            .sessions()
            .commit_resident_damage(plugin_id, attacks)
            .await
            .into_iter();
        let mut combat = Vec::new();
        for plan in &mut plans {
            for attack in std::mem::take(&mut plan.attacks) {
                if attack.ranged {
                    if self
                        .sessions()
                        .launch_resident_arrow(&attack.shooter, &attack.expected)
                        && let Some(record) = next_records.get_mut(&plan.handle)
                    {
                        record.last_attack_tick = Some(attack_tick);
                        let _ = gear_take(record, RESIDENT_ARROW, 1);
                    }
                    continue;
                }
                let Some(hit) = hits.next().flatten() else {
                    continue;
                };
                let damage_milli = (hit.damage * 1000.0).round().max(0.0) as u64;
                if damage_milli == 0 {
                    continue;
                }
                if let Some(record) = next_records.get_mut(&plan.handle) {
                    record.last_attack_tick = Some(attack_tick);
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
            next.morale = None;
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
        let records = BTreeMap::from([(handle.to_owned(), record.clone())]);
        let Some(Some(mut admission)) = self
            .prepare_resident_order_batch(storage, request, &records)
            .await
        else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if !commit_resident_order_batch(self.sessions(), &records, &mut admission).await {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        }
        let admission_id = admission.admission_id();
        // The first decision stops service without touching items. The guest
        // returns issued gear through fenced owned-inventory transfers before
        // asking for the second, civilian decision; personal items stay put.
        let complete = record.assignment == DurableAssignment::Demobilizing;
        let mut next = record.clone();
        next.assignment = if complete {
            DurableAssignment::Civilian
        } else {
            DurableAssignment::Demobilizing
        };
        next.order = None;
        next.work = None;
        next.morale = None;
        let resident = ScriptDemobilizeResult::new(
            handle.to_owned(),
            if complete {
                ScriptDemobilizeState::Civilian
            } else {
                ScriptDemobilizeState::Demobilizing
            },
            (!complete).then_some(ScriptWorkPauseReason::NoStorage),
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
            vec![
                DurableResidentOrderChange::Record {
                    record: Box::new(next),
                },
                DurableResidentOrderChange::Admission {
                    admission: Box::new(durable_admission(
                        admission_id,
                        plugin_id,
                        request
                            .operation_id()
                            .expect("demobilization decision identity"),
                        request_fingerprint(request),
                        &records,
                        &[],
                    )),
                },
            ],
        )?;
        if let Ok(uuid) = uuid::Uuid::parse_str(&record.entity_uuid) {
            self.sessions()
                .apply_resident_goals(vec![ResidentGoal {
                    uuid,
                    goal: GoalState::Idle,
                }])
                .await;
        }
        self.acknowledge_resident_admission(storage, admission_id, vec![handle.to_owned()]);
        Ok(self
            .resident_order_receipt_outcome(storage, plugin_id, request)
            .expect("committed demobilisation remains installed"))
    }

    async fn capture_resident(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        handle: &str,
        custodian: &str,
        expected_revision: u64,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        use super::resident_morale::MoralePhase;

        let Some(record) = resident_order_record(storage, plugin_id, handle) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if record.order_revision() != expected_revision {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        let Some(guard) = resident_order_record(storage, plugin_id, custodian) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if record.assignment != DurableAssignment::Military
            || record.morale.as_ref().map(|morale| morale.phase) != Some(MoralePhase::Routing)
            || !record
                .order
                .as_ref()
                .is_some_and(|order| matches!(order.order, ScriptResidentOrder::Attack { .. }))
            || guard.assignment != DurableAssignment::Military
            || guard.morale.as_ref().is_some_and(|morale| {
                matches!(
                    morale.phase,
                    MoralePhase::Routing | MoralePhase::Surrendered
                )
            })
        {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        }
        let Ok(victim_uuid) = uuid::Uuid::parse_str(&record.entity_uuid) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        let Ok(guard_uuid) = uuid::Uuid::parse_str(&guard.entity_uuid) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        let snapshots = self
            .sessions()
            .resident_entity_snapshots(&[victim_uuid, guard_uuid])
            .await;
        let Some((Some(victim), Some(guard_snapshot))) = snapshots
            .first()
            .zip(snapshots.get(1))
            .map(|(victim, guard)| (victim.as_ref(), guard.as_ref()))
        else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if !super::residents::resident_entity_is_current(victim)
            || !super::residents::resident_entity_is_current(guard_snapshot)
            || victim.health <= 0.0
            || guard_snapshot.health <= 0.0
        {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        }
        let close = |victim: &EntitySnapshot, guard: &EntitySnapshot| {
            let dx = victim.position.x - guard.position.x;
            let dy = victim.position.y - guard.position.y;
            let dz = victim.position.z - guard.position.z;
            dx * dx + dy * dy + dz * dz <= 9.0
        };
        if !close(victim, guard_snapshot) {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        }
        let records = BTreeMap::from([(handle.to_owned(), record.clone())]);
        let Some(Some(mut admission)) = self
            .prepare_resident_order_batch(storage, request, &records)
            .await
        else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if !commit_resident_order_batch(self.sessions(), &records, &mut admission).await {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        }
        let now = self
            .sessions()
            .resident_entity_snapshots(&[victim_uuid, guard_uuid])
            .await;
        if !matches!(now.as_slice(), [Some(victim_now), Some(guard_now)]
            if super::residents::resident_entity_is_current(victim_now)
                && super::residents::resident_entity_is_current(guard_now)
                && victim_now.health > 0.0
                && guard_now.health > 0.0
                && close(victim_now, guard_now))
        {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        }
        let admission_id = admission.admission_id();
        let mut next = record.clone();
        next.assignment = DurableAssignment::Prisoner;
        next.custodian = Some(custodian.to_owned());
        next.order = None;
        next.work = None;
        if let Some(morale) = &mut next.morale {
            morale.phase = MoralePhase::Surrendered;
            morale.goal_applied = false;
        }
        self.commit_resident_order(
            storage,
            plugin_id,
            request,
            ScriptOperationPayload::ResidentOrder {
                result: Box::new(ScriptResidentOrderResult::Captured {
                    handle: handle.to_owned(),
                    custodian: custodian.to_owned(),
                    revision: 0,
                }),
            },
            vec![
                DurableResidentOrderChange::Record {
                    record: Box::new(next),
                },
                DurableResidentOrderChange::Admission {
                    admission: Box::new(durable_admission(
                        admission_id,
                        plugin_id,
                        request.operation_id().expect("capture decision identity"),
                        request_fingerprint(request),
                        &records,
                        &[],
                    )),
                },
            ],
        )?;
        if self
            .sessions()
            .apply_resident_goals(vec![ResidentGoal {
                uuid: victim_uuid,
                goal: GoalState::Idle,
            }])
            .await
            == 1
        {
            self.acknowledge_resident_admission(storage, admission_id, vec![handle.to_owned()]);
        }
        Ok(self
            .resident_order_receipt_outcome(storage, plugin_id, request)
            .expect("committed physical capture remains installed"))
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
        attack_tick: u64,
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
        // Every distinct anchor is placed once per batch, not once per member.
        // Patrol members may have different route indices; cache each anchor.
        let mut formations = Vec::new();
        let mut formation_slot_at = |anchor: Vec3, index: usize| {
            let cached = formations
                .iter()
                .position(|(position, _)| *position == anchor)
                .unwrap_or_else(|| {
                    formations.push((anchor, self.formation_slots_at(order, anchor, count)));
                    formations.len() - 1
                });
            formations[cached].1.as_ref()?.get(index).copied()
        };
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
                active_target_ref: None,
                garrison: None,
                observed_health: member_uuids
                    .iter()
                    .position(|uuid| *uuid == uuid_of(&record))
                    .and_then(|index| {
                        member_snapshots[index]
                            .as_ref()
                            .map(|snapshot| snapshot.health)
                    }),
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
                        plan.slot = formation_slot_at(position, index)
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
                    plan.slot = formation_slot_at(anchor, index).map(|(slot, _)| slot);
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
                    if let Some((slot, position)) = formation_slot_at(anchor, index) {
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
                    if let Some((slot, position)) = formation_slot_at(anchor, index) {
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
                    if let Some((slot, position)) = formation_slot_at(anchor, index) {
                        plan.slot = Some(slot);
                        plan.goal = Some(GoalState::FollowPosition {
                            target: position,
                            speed: 1.2,
                        });
                    } else {
                        // An unplaceable rally must still stop the interrupted
                        // chase: the member keeps its honest `blocked_route`
                        // report and stops instead of leaving the old attack
                        // goal installed.
                        plan.goal = Some(GoalState::Idle);
                    }
                }
                ScriptResidentOrder::Attack { targets, policy } => {
                    let rally = policy.rally.expect("accepted attack has a rally point");
                    let Some(world) = self.resident_world() else {
                        plans.push(plan);
                        continue;
                    };
                    let reachable = member_positions.get(&uuid_of(&record)).is_some_and(|from| {
                        goal_for_position(rally, world).is_some()
                            && !self.route_blocked(
                                RESIDENT_WORLD_DIMENSION,
                                *from,
                                block_position(&rally),
                            )
                    });
                    if !reachable {
                        plans.push(plan);
                        continue;
                    }
                    if let Some(officer) = &policy.officer {
                        let available = storage
                            .resident_orders()
                            .record(officer)
                            .filter(|officer_record| {
                                officer_record.plugin_id == plugin_id
                                    && officer_record.assignment == DurableAssignment::Military
                            })
                            .and_then(|officer_record| {
                                uuid::Uuid::parse_str(&officer_record.entity_uuid).ok()
                            });
                        let Some(officer_uuid) = available else {
                            plans.push(plan);
                            continue;
                        };
                        if !self
                            .sessions()
                            .resident_entity_snapshots(&[officer_uuid])
                            .await
                            .into_iter()
                            .flatten()
                            .any(|officer| officer.lifecycle == mc_entity::EntityLifecycle::Alive)
                        {
                            plans.push(plan);
                            continue;
                        }
                    }
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
                            &record,
                            targets,
                            policy,
                            &references,
                            &live_players,
                            transaction_id,
                            attack_tick,
                            false,
                        )
                        .await;
                    plan.attacks = attacks;
                    refs.extend(refreshed);
                    // Cooldown suppresses damage, not a valid chase toward
                    // the member's still-permitted, server-issued target.
                    let chase = plan
                        .attacks
                        .first()
                        .map(|attack| (attack.expected.id, attack.target_ref.clone()))
                        .or_else(|| {
                            plan.targets.iter().find_map(|candidate| {
                                let requested = targets.iter().find(|requested| {
                                    requested.target_ref == candidate.target_ref
                                })?;
                                let reference = storage
                                    .resident_orders()
                                    .reference(plugin_id, &candidate.target_ref)?;
                                if reference.expires_revision < storage.revision
                                    || reference.expires_revision != requested.expires_revision
                                    || reference.policy_revision != requested.policy_revision
                                    || reference.policy_revision != policy.revision
                                    || reference.category != candidate.category.as_str()
                                {
                                    return None;
                                }
                                let uuid = uuid::Uuid::parse_str(&reference.entity_uuid).ok()?;
                                let target = references.get(&uuid)?;
                                (target.lifecycle == mc_entity::EntityLifecycle::Alive)
                                    .then(|| (target.id, candidate.target_ref.clone()))
                            })
                        });
                    plan.active_target_ref =
                        chase.as_ref().map(|(_, target_ref)| target_ref.clone());
                    plan.goal = Some(chase.map_or(GoalState::Idle, |(target, _)| {
                        GoalState::FollowTarget { target, speed: 1.1 }
                    }));
                    plan.slot = Some(u16::try_from(index).unwrap_or(u16::MAX));
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

    /// Place one formation at an anchor and retain its indexed member slots.
    fn formation_slots_at(
        &self,
        order: &ScriptResidentOrder,
        anchor: Vec3,
        count: usize,
    ) -> Option<Vec<(u16, Vec3)>> {
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
            Some(FormationPlacement::Placed(placements)) => Some(
                placements
                    .into_iter()
                    .map(|placement| {
                        (
                            u16::try_from(placement.slot).unwrap_or(u16::MAX),
                            placement.position,
                        )
                    })
                    .collect(),
            ),
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
            let expires_revision = transaction_id.saturating_add(TARGET_REF_TTL_REVISIONS);
            refs.push(DurableTargetRef {
                plugin_id: plugin_id.to_owned(),
                target_ref: target_ref.clone(),
                entity_uuid: uuid,
                category: candidate.category.as_str().to_owned(),
                policy_revision: policy.revision,
                revision: 0,
                expires_revision,
            });
            targets.push(ScriptOrderTarget::new(
                target_ref,
                policy.revision,
                expires_revision,
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
        record: &DurableResidentOrderRecord,
        targets: &[ScriptOrderTargetRef],
        policy: &ScriptEngagementPolicy,
        references: &BTreeMap<uuid::Uuid, &EntitySnapshot>,
        live_players: &std::collections::BTreeSet<uuid::Uuid>,
        transaction_id: u64,
        attack_tick: u64,
        accepted_order: bool,
    ) -> (Vec<PlannedAttack>, Vec<DurableTargetRef>) {
        let Some(world) = self.resident_world() else {
            return (Vec::new(), Vec::new());
        };
        if record
            .last_attack_tick
            .is_some_and(|last| attack_tick.saturating_sub(last) < RESIDENT_ATTACK_COOLDOWN_TICKS)
        {
            return (Vec::new(), Vec::new());
        }
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
        if ranged && !gear_has(record, RESIDENT_ARROW) {
            // No ammo: the archer engages nothing and consumes nothing.
            return (Vec::new(), Vec::new());
        }
        let Some(attacker) = self
            .sessions()
            .resident_entity_snapshots(&[uuid_of(record)])
            .await
            .into_iter()
            .next()
            .flatten()
        else {
            return (Vec::new(), Vec::new());
        };
        if attacker.lifecycle != mc_entity::EntityLifecycle::Alive {
            return (Vec::new(), Vec::new());
        }
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
            if (!accepted_order
                && (reference.expires_revision < storage.revision
                    || reference.expires_revision != target.expires_revision))
                || reference.policy_revision != target.policy_revision
                || reference.policy_revision != policy.revision()
            {
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
            if snapshot.lifecycle != mc_entity::EntityLifecycle::Alive
                || reference.category != category.as_str()
            {
                continue;
            }
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
            let distance = distance(&attacker.position, &snapshot.position);
            if distance > reach {
                continue;
            }
            let eye = Vec3::new(
                attacker.position.x,
                attacker.position.y + 1.5,
                attacker.position.z,
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
                shooter: attacker.clone(),
                amount,
                ranged,
            });
            if !accepted_order {
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
            break; // one native attack per member inside its cooldown
        }
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

    /// Resolve the engine goal of one durable order or cancellation, used by
    /// admission replay.
    fn resident_order_goal(&self, record: &DurableResidentOrderRecord) -> Option<GoalState> {
        let Some(order) = record.order.as_ref() else {
            return Some(GoalState::Idle);
        };
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
                if let Some([x, y, z]) = order.formation_destination {
                    return Some(GoalState::FollowPosition {
                        target: Vec3::new(f64::from_bits(x), f64::from_bits(y), f64::from_bits(z)),
                        speed: if matches!(order.order, ScriptResidentOrder::Retreat { .. }) {
                            1.2
                        } else {
                            1.0
                        },
                    });
                }
                let world = self.resident_world()?;
                let goal = goal_for_order(&order.order, world);
                if goal.is_none() && matches!(order.order, ScriptResidentOrder::Retreat { .. }) {
                    // A replay of an unplaceable rally stops the interrupted
                    // chase the same way the accepted order did.
                    return Some(GoalState::Idle);
                }
                goal
            }
            ScriptResidentOrder::Patrol { waypoints, .. } => {
                if let Some([x, y, z]) = order.formation_destination {
                    return Some(GoalState::FollowPosition {
                        target: Vec3::new(f64::from_bits(x), f64::from_bits(y), f64::from_bits(z)),
                        speed: 1.0,
                    });
                }
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

    /// Whether the live resident can walk to a standable neighbour of one
    /// bounded tree cell. Missing snapshots or an unavailable route fail closed
    /// instead of granting remote block work.
    async fn tree_cell_reachable(
        &self,
        world: &dyn ResidentWorld,
        dimension: &str,
        record: &DurableResidentOrderRecord,
        tree_cell: [i32; 3],
    ) -> Result<bool, ScriptWorkPauseReason> {
        let Some(from) = self
            .sessions()
            .resident_entity_snapshots(&[uuid_of(record)])
            .await
            .into_iter()
            .next()
            .flatten()
            .map(|snapshot| {
                [
                    snapshot.position.x.floor() as i32,
                    snapshot.position.y.floor() as i32,
                    snapshot.position.z.floor() as i32,
                ]
            })
        else {
            return Ok(false);
        };
        let mut root = tree_cell;
        for _ in 0..16 {
            let below = [root[0], root[1] - 1, root[2]];
            let block = world
                .block(dimension, below)
                .ok_or(ScriptWorkPauseReason::Unloaded)?;
            if !is_log_path(&block.path) {
                break;
            }
            root = below;
        }
        for [dx, dz] in [[-1, 0], [1, 0], [0, -1], [0, 1]] {
            match world.route_open(dimension, from, [root[0] + dx, root[1], root[2] + dz]) {
                Some(true) => return Ok(true),
                Some(false) => {}
                None => return Err(ScriptWorkPauseReason::Unloaded),
            }
        }
        Ok(false)
    }

    /// Whether the live resident can walk to a bounded mining stance beside or
    /// directly atop one ore cell. All candidate routes must be visible in the
    /// loaded world; an absent snapshot or route never grants remote extraction.
    async fn mine_cell_reachable(
        &self,
        world: &dyn ResidentWorld,
        dimension: &str,
        record: &DurableResidentOrderRecord,
        ore_cell: [i32; 3],
    ) -> Result<bool, ScriptWorkPauseReason> {
        let Some(from) = self
            .sessions()
            .resident_entity_snapshots(&[uuid_of(record)])
            .await
            .into_iter()
            .next()
            .flatten()
            .map(|snapshot| {
                [
                    snapshot.position.x.floor() as i32,
                    snapshot.position.y.floor() as i32,
                    snapshot.position.z.floor() as i32,
                ]
            })
        else {
            return Ok(false);
        };
        for target in [
            [ore_cell[0] - 1, ore_cell[1], ore_cell[2]],
            [ore_cell[0] + 1, ore_cell[1], ore_cell[2]],
            [ore_cell[0], ore_cell[1], ore_cell[2] - 1],
            [ore_cell[0], ore_cell[1], ore_cell[2] + 1],
            [ore_cell[0], ore_cell[1] + 1, ore_cell[2]],
        ] {
            match world.route_open(dimension, from, target) {
                Some(true) => return Ok(true),
                Some(false) => {}
                None => return Err(ScriptWorkPauseReason::Unloaded),
            }
        }
        Ok(false)
    }

    /// Current standable work cell of a live resident. A missing actor snapshot
    /// is never permission to complete a remote resource job.
    async fn resident_work_cell(&self, record: &DurableResidentOrderRecord) -> Option<[i32; 3]> {
        self.sessions()
            .resident_entity_snapshots(&[uuid_of(record)])
            .await
            .into_iter()
            .next()
            .flatten()
            .map(|snapshot| {
                [
                    snapshot.position.x.floor() as i32,
                    snapshot.position.y.floor() as i32,
                    snapshot.position.z.floor() as i32,
                ]
            })
    }

    /// A fisher stands on shore one block above an adjacent water source.
    fn fishing_cell_reachable(
        &self,
        world: &dyn ResidentWorld,
        dimension: &str,
        from: [i32; 3],
        water_cell: [i32; 3],
    ) -> Result<bool, ScriptWorkPauseReason> {
        let mut saw_unloaded = false;
        for [dx, dz] in [[-1, 0], [1, 0], [0, -1], [0, 1]] {
            match world.route_open(
                dimension,
                from,
                [water_cell[0] + dx, water_cell[1] + 1, water_cell[2] + dz],
            ) {
                Some(true) => return Ok(true),
                Some(false) => {}
                None => saw_unloaded = true,
            }
        }
        if saw_unloaded {
            Err(ScriptWorkPauseReason::Unloaded)
        } else {
            Ok(false)
        }
    }

    /// A tender stands beside, not inside, the live animal it feeds.
    fn animal_cell_reachable(
        &self,
        world: &dyn ResidentWorld,
        dimension: &str,
        from: [i32; 3],
        animal_position: Vec3,
    ) -> Result<bool, ScriptWorkPauseReason> {
        let cell = [
            animal_position.x.floor() as i32,
            animal_position.y.floor() as i32,
            animal_position.z.floor() as i32,
        ];
        let mut saw_unloaded = false;
        for [dx, dz] in [[-1, 0], [1, 0], [0, -1], [0, 1]] {
            match world.route_open(dimension, from, [cell[0] + dx, cell[1], cell[2] + dz]) {
                Some(true) => return Ok(true),
                Some(false) => {}
                None => saw_unloaded = true,
            }
        }
        if saw_unloaded {
            Err(ScriptWorkPauseReason::Unloaded)
        } else {
            Ok(false)
        }
    }

    /// Whether a member cannot walk between two positions: an absent world
    /// adapter, an unsimulated dimension, a blocked route or unloaded terrain.
    pub(super) fn route_blocked(&self, dimension: &str, from: Vec3, to: Vec3) -> bool {
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
                alive: super::residents::resident_entity_is_current(snapshot),
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
    if resident.plugin_id != plugin_id
        || resident.disposition != super::residents::ResidentDisposition::Alive
    {
        return None;
    }
    let entity_uuid = resident.entity_uuid.clone();
    match storage.resident_orders().record(handle) {
        Some(record)
            if record.entity_uuid == entity_uuid
                && record.assignment != DurableAssignment::Prisoner =>
        {
            Some(record.clone())
        }
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
        active_target_ref: plan.active_target_ref.clone(),
        formation_destination: if matches!(
            order,
            ScriptResidentOrder::Move { .. }
                | ScriptResidentOrder::Patrol { .. }
                | ScriptResidentOrder::Retreat { .. }
        ) {
            match plan.goal {
                Some(GoalState::FollowPosition { target, .. }) => {
                    Some([target.x.to_bits(), target.y.to_bits(), target.z.to_bits()])
                }
                _ => None,
            }
        } else {
            None
        },
    }));
    next.morale = match order {
        ScriptResidentOrder::Attack { policy, .. } if plan.slot.is_some() => {
            plan.observed_health.map(|health| {
                super::resident_morale::OperationalMorale::new(
                    health,
                    policy.rally.expect("accepted attack has a rally point"),
                    policy.officer.clone(),
                )
            })
        }
        ScriptResidentOrder::Attack { .. } => None,
        _ => None,
    };
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

/// Whether one item can fit without leaving a partial mutation behind.
fn resident_item_fits(
    record: &DurableResidentOrderRecord,
    item_id: &str,
    count: u64,
    max_stack: u32,
) -> bool {
    let max_stack = max_stack.max(1);
    let existing_room = record
        .carry
        .iter()
        .chain(record.equipment.iter())
        .flatten()
        .filter(|stack| mergeable_stack(stack, item_id))
        .map(|stack| u64::from(max_stack.saturating_sub(stack.count)))
        .sum::<u64>();
    let remaining = count.saturating_sub(existing_room);
    let free_slots = u64::try_from(free_resident_slots(record)).unwrap_or(u64::MAX);
    remaining <= free_slots.saturating_mul(u64::from(max_stack))
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
    if !resident_item_fits(record, item_id, count, max_stack) {
        return false;
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
        mc_data::recipes::RecipeKind::Smelting(cooking)
        | mc_data::recipes::RecipeKind::Blasting(cooking)
        | mc_data::recipes::RecipeKind::Smoking(cooking)
        | mc_data::recipes::RecipeKind::CampfireCooking(cooking) => {
            Some(vec![cooking.ingredient.clone()])
        }
        mc_data::recipes::RecipeKind::Stonecutting(stonecutting) => {
            Some(vec![stonecutting.ingredient.clone()])
        }
    }
}

/// The direct worker craft path owns only stateless stations. Furnace, smoker
/// and blast-furnace recipes remain with their live fuel and burn-progress
/// state machines instead of minting an instantaneous no-fuel result here.
fn resident_recipe_station(recipe: &mc_data::recipes::Recipe) -> Option<&'static str> {
    match &recipe.kind {
        mc_data::recipes::RecipeKind::Shaped(_) | mc_data::recipes::RecipeKind::Shapeless(_) => {
            Some("crafting_table")
        }
        mc_data::recipes::RecipeKind::CampfireCooking(_) => Some("campfire"),
        mc_data::recipes::RecipeKind::Stonecutting(_) => Some("stonecutter"),
        mc_data::recipes::RecipeKind::Smelting(_)
        | mc_data::recipes::RecipeKind::Blasting(_)
        | mc_data::recipes::RecipeKind::Smoking(_) => None,
    }
}

/// The server's item component gives the canonical container item back after
/// crafting consumes an item such as a milk bucket.
fn crafting_remainder(
    item_facts: &mc_data::item_components::ItemFactsTable,
    item_id: &str,
) -> Option<String> {
    let item = Identifier::parse(item_id.to_owned()).ok()?;
    item_facts
        .get(&item)
        .and_then(|facts| facts.use_remainder.as_ref())
        .map(|remainder| remainder.as_str().to_owned())
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

/// A receipt-bearing resident step stays in one eight-chunk regional owner
/// lane. The next order call resumes the area at the first untouched target.
fn same_resident_work_region(left: [i32; 3], right: [i32; 3]) -> bool {
    const REGION_BLOCKS: i32 = 8 * 16;
    left[0].div_euclid(REGION_BLOCKS) == right[0].div_euclid(REGION_BLOCKS)
        && left[2].div_euclid(REGION_BLOCKS) == right[2].div_euclid(REGION_BLOCKS)
}

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

/// Bounded evidence that a trunk belongs to a tree rather than a wooden build.
/// The scan follows at most sixteen vertical log cells and requires a canopy
/// block directly above the trunk's top; unloaded terrain fails closed.
fn rooted_tree_with_canopy(
    world: &dyn ResidentWorld,
    dimension: &str,
    cell: [i32; 3],
) -> Result<bool, ScriptWorkPauseReason> {
    const MAX_TREE_TRUNK_HEIGHT: usize = 16;
    let mut root = cell;
    for _ in 0..MAX_TREE_TRUNK_HEIGHT {
        let below = [root[0], root[1] - 1, root[2]];
        let block = world
            .block(dimension, below)
            .ok_or(ScriptWorkPauseReason::Unloaded)?;
        if !is_log_path(&block.path) {
            break;
        }
        root = below;
    }
    let below_root = [root[0], root[1] - 1, root[2]];
    let ground = world
        .block(dimension, below_root)
        .ok_or(ScriptWorkPauseReason::Unloaded)?;
    if !is_ground_path(&ground.path) {
        return Ok(false);
    }

    let mut top = cell;
    for _ in 0..MAX_TREE_TRUNK_HEIGHT {
        let above = [top[0], top[1] + 1, top[2]];
        let block = world
            .block(dimension, above)
            .ok_or(ScriptWorkPauseReason::Unloaded)?;
        if !is_log_path(&block.path) {
            break;
        }
        top = above;
    }
    let canopy = world
        .block(dimension, [top[0], top[1] + 1, top[2]])
        .ok_or(ScriptWorkPauseReason::Unloaded)?;
    Ok(canopy.path.ends_with("_leaves")
        || matches!(
            canopy.path.as_str(),
            "nether_wart_block" | "warped_wart_block"
        ))
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

fn source_missing_input_wait(
    record: &DurableResidentOrderRecord,
    work: &ScriptResidentWorkOrder,
    reason: Option<ScriptWorkPauseReason>,
) -> bool {
    if reason != Some(ScriptWorkPauseReason::MissingInput) {
        return false;
    }
    match work {
        ScriptResidentWorkOrder::Harvest { .. }
        | ScriptResidentWorkOrder::CutTree { .. }
        | ScriptResidentWorkOrder::Mine { .. }
        | ScriptResidentWorkOrder::Fish { .. } => true,
        ScriptResidentWorkOrder::Replant { seed, .. } => gear_has(record, &resource_item(seed)),
        ScriptResidentWorkOrder::TendLivestock { feed, .. } => {
            gear_has(record, &resource_item(feed))
        }
        _ => false,
    }
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
