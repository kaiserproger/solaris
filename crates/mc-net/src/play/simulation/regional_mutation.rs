use mc_world::{ChunkPos, JournalStampResult, ResidentBlockEdit, ResidentBlockPrecondition};

use super::super::block_edit_commit::{
    resident_block_edit_result_outcome, resident_block_edits, resident_block_preconditions,
};
use super::{
    Arc, BlockEdit, BlockEditBatchOutcome, BlockMutationToken, BlockPos, BlockStateId,
    BucketUsePlan, BucketUseTransaction, CAMPFIRE_BLOCK_ENTITY_TYPE_ID, CampfireUsePlan,
    CampfireUseTransaction, ChestBlockEntity, ChestCommitOutcome, ChestTransaction,
    ChestTransactionRequest, CommittedBucketUse, CommittedCampfireUse, CommittedSurvivalBreak,
    CommittedSurvivalPlacement, ContainerDropPlan, ContainerPlayerPlan, ContainerXpPlan,
    FurnaceBlockEntity, FurnaceCommitOutcome, FurnaceTransaction, FurnaceTransactionRequest,
    HashMap, IncrementalLightSources, Ordering, ScheduledBlockTick, ServerOwnedChestCommit,
    SessionId, SessionRegistry, SharedContainerCommit, SimulationCommand,
    SimulationCommandAttribution, SimulationCommandEnvelope, SimulationLaneAttribution,
    SimulationOwner, SimulationRequestError, SimulationResponse, SimulationTickReport,
    SimulationWorldAccess, SurvivalBreakPlan, SurvivalBreakRequest, SurvivalBreakTransaction,
    SurvivalPlacementPlan, SurvivalPlacementTransaction, Vec3, VisibilityDispatch,
    WarehouseTransferOutcome, air_state_id, append_block_edit_outcome,
    applied_edits_need_fluid_ticks, command_single_owner_region, dispatch_regional_block_outcome,
    dispatch_visibility_commands, elapsed_us, falling_block_start_chunks, is_campfire_block,
    is_falling_block_state, plan_falling_block_starts, prepare_survival_block_break_plan,
    publish_regional_light_updates, regional_light_updates, resident_block_edit_outcome,
    schedule_resident_fluid_ticks_near_applied, snapshot_region, valid_survival_break_plan, warn,
};
use std::collections::BTreeMap;

#[cfg(test)]
use super::OutboundCommand;
#[cfg(test)]
use super::RegionKey;

#[cfg(test)]
#[derive(Debug, Clone)]
pub(in crate::play) struct RegionalBlockEditProbe {
    entered: std::sync::mpsc::Sender<RegionKey>,
    release: Arc<std::sync::Mutex<std::sync::mpsc::Receiver<()>>>,
}

#[cfg(test)]
impl RegionalBlockEditProbe {
    pub(in crate::play) fn enter(&self, region: RegionKey) {
        self.entered.send(region).expect("regional worker entry");
        self.release
            .lock()
            .expect("test lock poisoned")
            .recv()
            .expect("regional worker release");
    }
}

struct RegionalBlockEditJob {
    sequence: u64,
    kind: &'static str,
    journal_id: Option<u64>,
    #[cfg(test)]
    region: RegionKey,
    command: RegionalMutationJob,
}

enum RegionalMutationJob {
    BlockEdits {
        actor_session: Option<SessionId>,
        edits: Vec<ResidentBlockEdit>,
        preconditions: Vec<ResidentBlockPrecondition>,
        scheduled_block_ticks: Vec<ScheduledBlockTick>,
        leaf_trigger: bool,
        zone_fence: Option<crate::script::ZoneProtectionFence>,
        hook_approval: Option<mc_script::precommit::Approval>,
        plugin_receipt: Option<Vec<u8>>,
    },
    SurvivalPlacement {
        actor_session: SessionId,
        transaction: Option<SurvivalPlacementTransaction>,
        plan: SurvivalPlacementPlan,
    },
    SurvivalBreak {
        actor_session: SessionId,
        transaction: Option<SurvivalBreakTransaction>,
        request: SurvivalBreakRequest,
        planning_snapshot: mc_world::WorldReadSnapshot,
    },
    BucketUse {
        actor_session: SessionId,
        transaction: Option<BucketUseTransaction>,
        plan: BucketUsePlan,
    },
    Chest {
        transaction: Option<ChestTransaction>,
        primary_position: BlockPos,
        positions: Vec<BlockPos>,
        expected_tokens: Option<Vec<mc_world::BlockMutationToken>>,
        expected_state_id: i32,
        expected: Vec<ChestBlockEntity>,
        updated: Vec<ChestBlockEntity>,
        /// The player participant, or `None` for a server-owned deposit whose
        /// second participant rides the plugin receipt.
        player: Option<Box<ContainerPlayerPlan>>,
        /// Present for a server-owned deposit: the encoded plugin receipt the
        /// run journals beside the container's after-image.
        plugin_receipt: Option<Vec<u8>>,
    },
    Furnace {
        transaction: Option<FurnaceTransaction>,
        position: BlockPos,
        expected_state_id: i32,
        expected: FurnaceBlockEntity,
        updated: Box<FurnaceBlockEntity>,
        player: Box<ContainerPlayerPlan>,
    },
    OpaqueBlockEntity {
        position: BlockPos,
        expected_state: BlockStateId,
        expected_token: BlockMutationToken,
        bytes: Vec<u8>,
    },
    CampfireUse {
        actor_session: SessionId,
        transaction: Option<CampfireUseTransaction>,
        plan: Box<CampfireUsePlan>,
    },
}

/// What one server-owned deposit contributes to the run's single group append.
enum RegionalWarehouseDecision {
    /// The container committed and its after-image is stamped for the id this
    /// envelope reserved, so the run journals the container and the plugin
    /// receipt together and then releases the stamp's flush fence.
    Stamped {
        images: Vec<mc_world::ChunkSnapshot>,
        receipt: Vec<u8>,
    },
    /// The composite refused before any mutation: the reserved id is closed
    /// with no participant so the run's append stays contiguous.
    Refused,
    /// The container committed but its chunk could not be stamped for this id,
    /// so the decision cannot be appended at all: the run fails closed rather
    /// than publishing a receipt recoverable without its container.
    Unjournaled,
}

enum RegionalBlockEditJobResult {
    BlockEdits {
        sequence: u64,
        actor_session: Option<SessionId>,
        outcome: Box<Option<BlockEditBatchOutcome>>,
        failure: Option<SimulationRequestError>,
        journal_snapshots: Option<Vec<mc_world::ChunkSnapshot>>,
        journal_snapshot_complete: bool,
        light_sources: Option<IncrementalLightSources>,
        light_updates: Vec<crate::play::session::OutboundLightUpdate>,
        plugin_receipt: Option<Vec<u8>>,
    },
    SurvivalPlacement {
        sequence: u64,
        actor_session: SessionId,
        committed: Box<Result<Option<CommittedSurvivalPlacement>, SimulationRequestError>>,
        block_facts: Arc<mc_data::block_facts::BlockFactsTable>,
        needs_fluid_ticks: bool,
        light_sources: Option<IncrementalLightSources>,
        light_updates: Vec<crate::play::session::OutboundLightUpdate>,
    },
    SurvivalBreak {
        sequence: u64,
        actor_session: SessionId,
        committed: Box<Result<Option<CommittedSurvivalBreak>, SimulationRequestError>>,
        plan: Option<Box<SurvivalBreakPlan>>,
        falling_spawns: Vec<(i32, Vec3, BlockStateId)>,
        block_facts: Option<Arc<mc_data::block_facts::BlockFactsTable>>,
        needs_fluid_ticks: bool,
        light_sources: Option<IncrementalLightSources>,
        light_updates: Vec<crate::play::session::OutboundLightUpdate>,
    },
    BucketUse {
        sequence: u64,
        actor_session: SessionId,
        committed: Box<Result<Option<CommittedBucketUse>, SimulationRequestError>>,
        block_facts: Arc<mc_data::block_facts::BlockFactsTable>,
        schedule_fluid_ticks: bool,
        light_sources: Option<IncrementalLightSources>,
        light_updates: Vec<crate::play::session::OutboundLightUpdate>,
    },
    Chest {
        sequence: u64,
        outcome: Box<Result<ChestCommitOutcome, SimulationRequestError>>,
        drops: Vec<ContainerDropPlan>,
    },
    /// One server-owned deposit: the run pairs its decision with the reserved
    /// id this envelope already holds.
    WarehouseTransfer {
        sequence: u64,
        outcome: Box<Result<WarehouseTransferOutcome, SimulationRequestError>>,
        decision: Option<RegionalWarehouseDecision>,
        dispatches: Vec<VisibilityDispatch>,
    },
    Furnace {
        sequence: u64,
        outcome: Box<Result<FurnaceCommitOutcome, SimulationRequestError>>,
        drops: Vec<ContainerDropPlan>,
        xp_orb: Option<ContainerXpPlan>,
    },
    OpaqueBlockEntity {
        sequence: u64,
        outcome: Result<bool, SimulationRequestError>,
    },
    CampfireUse {
        sequence: u64,
        actor_session: SessionId,
        committed: Box<Result<Option<CommittedCampfireUse>, SimulationRequestError>>,
        position: BlockPos,
        client_nbt: mc_nbt::Tag,
    },
}

impl RegionalBlockEditJobResult {
    fn sequence(&self) -> u64 {
        match self {
            Self::BlockEdits { sequence, .. }
            | Self::SurvivalPlacement { sequence, .. }
            | Self::SurvivalBreak { sequence, .. }
            | Self::BucketUse { sequence, .. }
            | Self::Chest { sequence, .. }
            | Self::WarehouseTransfer { sequence, .. }
            | Self::Furnace { sequence, .. }
            | Self::OpaqueBlockEntity { sequence, .. }
            | Self::CampfireUse { sequence, .. } => *sequence,
        }
    }
}

struct RegionalBlockEditJobOutcome {
    result: RegionalBlockEditJobResult,
    attribution: SimulationCommandAttribution,
}

struct RegionalBlockEditLaneOutcome {
    lane: usize,
    cpu_admission_wait_us: u64,
    commands: Vec<RegionalBlockEditJobOutcome>,
}

impl SimulationOwner {
    #[cfg(test)]
    pub(in crate::play) fn install_regional_block_edit_probe(
        &mut self,
        entered: std::sync::mpsc::Sender<RegionKey>,
        release: std::sync::mpsc::Receiver<()>,
    ) {
        self.regional_block_edit_probe = Some(RegionalBlockEditProbe {
            entered,
            release: Arc::new(std::sync::Mutex::new(release)),
        });
    }

    pub(super) async fn process_regional_block_edit_run(
        &mut self,
        sessions: &SessionRegistry,
        access: SimulationWorldAccess<'_>,
        journal: Option<&crate::play::world_journal::WorldChunkJournal>,
        run: Vec<SimulationCommandEnvelope>,
    ) -> SimulationTickReport {
        let world_read = access.read.expect("regional block-edit read view");
        let mutation = access.mutation.expect("regional block-edit mutation view");
        let resources = access.cpu.expect("regional block-edit CPU admission");
        let block_light_owned = access.light;
        let lane_count = resources.cpu_capacity().max(1);
        let world_tick = sessions.simulation_tick();
        // Regional phases must not span simulation turns: the region tick
        // precedes command processing and cannot enter while this turn owns
        // a prepared patient. Fence before assigning a world decision id.
        let mut admitted = Vec::with_capacity(run.len());
        let mut refused = 0;
        for envelope in run {
            if let SimulationCommand::CommitChest {
                treatment: Some(participant),
                ..
            } = &envelope.command
            {
                let (expected, accepted, operation_revision, heal_milli) = {
                    let guard = participant.lock().expect("planned treatment lock");
                    let treatment = guard.as_ref().expect("planned treatment participant");
                    (
                        treatment.expected.clone(),
                        treatment.accepted.clone(),
                        treatment.operation_revision,
                        treatment.heal_milli,
                    )
                };
                match sessions
                    .prepare_resident_treatment(expected, operation_revision, heal_milli)
                    .await
                {
                    Ok(Some((mutation, next))) if next == accepted => {
                        participant
                            .lock()
                            .expect("planned treatment lock")
                            .as_mut()
                            .expect("planned treatment participant")
                            .mutation = Some(mutation);
                    }
                    Ok(Some(_))
                    | Ok(None)
                    | Err(mc_entity::RegionOwnerLaneError::InvalidMutation)
                    | Err(mc_entity::RegionOwnerLaneError::Busy) => {
                        self.metrics.processed.fetch_add(1, Ordering::Relaxed);
                        refused += 1;
                        envelope.respond(Ok(SimulationResponse::WarehouseTransfer(Ok(
                            WarehouseTransferOutcome::StaleResident,
                        ))));
                        continue;
                    }
                    Err(error) => {
                        warn!(?error, "resident treatment regional preparation failed");
                        sessions.report_world_chunk_journal_failure();
                        self.metrics
                            .rejected_world_mutation
                            .fetch_add(1, Ordering::Relaxed);
                        refused += 1;
                        envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                        continue;
                    }
                }
            }
            admitted.push(envelope);
        }
        let run = admitted;
        if run.is_empty() {
            return SimulationTickReport {
                processed: refused,
                remaining_depth: self.metrics.depth.load(Ordering::Relaxed),
                ..SimulationTickReport::default()
            };
        }
        let journal_ids = if let Some(journal) = journal {
            let journal = journal.clone();
            let command_count = run.len();
            let reservation =
                tokio::task::spawn_blocking(move || journal.reserve_decision_ids(command_count))
                    .await;
            match reservation {
                Ok(Ok(ids)) => run
                    .iter()
                    .map(|envelope| envelope.sequence)
                    .zip(ids)
                    .collect::<HashMap<_, _>>(),
                Ok(Err(error)) => {
                    warn!(%error, "world chunk journal decision reservation failed");
                    sessions.report_world_chunk_journal_failure();
                    for envelope in run {
                        envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                    }
                    return SimulationTickReport {
                        processed: 0,
                        remaining_depth: self.metrics.depth.load(Ordering::Relaxed),
                        ..SimulationTickReport::default()
                    };
                }
                Err(error) => {
                    warn!(?error, "world chunk journal reservation worker failed");
                    sessions.report_world_chunk_journal_failure();
                    for envelope in run {
                        envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                    }
                    return SimulationTickReport {
                        processed: 0,
                        remaining_depth: self.metrics.depth.load(Ordering::Relaxed),
                        ..SimulationTickReport::default()
                    };
                }
            }
        } else {
            HashMap::new()
        };
        let mut lanes = BTreeMap::<usize, Vec<RegionalBlockEditJob>>::new();
        for envelope in &run {
            let region =
                command_single_owner_region(&envelope.command).expect("regional mutation owner");
            let command = match &envelope.command {
                SimulationCommand::ApplyBlockEdits {
                    actor_session,
                    edits,
                    preconditions,
                    scheduled_block_ticks,
                    leaf_trigger,
                    zone_fence,
                    hook_approval,
                    plugin_receipt,
                    ..
                } => RegionalMutationJob::BlockEdits {
                    actor_session: *actor_session,
                    edits: resident_block_edits(edits),
                    preconditions: resident_block_preconditions(preconditions),
                    scheduled_block_ticks: scheduled_block_ticks.clone(),
                    leaf_trigger: *leaf_trigger,
                    zone_fence: zone_fence.clone(),
                    hook_approval: hook_approval.clone(),
                    plugin_receipt: plugin_receipt.clone(),
                },
                SimulationCommand::CommitSurvivalPlacement(command) => {
                    RegionalMutationJob::SurvivalPlacement {
                        actor_session: command.actor_session,
                        transaction: sessions
                            .prepare_survival_placement_transaction(command.actor_session),
                        plan: command.plan.clone(),
                    }
                }
                SimulationCommand::CommitSurvivalBreak(command) => {
                    RegionalMutationJob::SurvivalBreak {
                        actor_session: command.actor_session,
                        transaction: sessions
                            .prepare_survival_break_transaction(command.actor_session),
                        request: command.request.clone(),
                        planning_snapshot: snapshot_region(world_read, region),
                    }
                }
                SimulationCommand::CommitBucketUse(command) => RegionalMutationJob::BucketUse {
                    actor_session: command.actor_session,
                    transaction: sessions.prepare_bucket_use_transaction(command.actor_session),
                    plan: command.plan.clone(),
                },
                SimulationCommand::CommitChest {
                    primary_position,
                    positions,
                    expected_tokens,
                    expected_state_id,
                    actor_session,
                    expected,
                    updated,
                    player,
                    plugin_receipt,
                    ..
                } => RegionalMutationJob::Chest {
                    transaction: sessions
                        .prepare_chest_transaction(*actor_session, *primary_position),
                    primary_position: *primary_position,
                    positions: positions.clone(),
                    expected_tokens: expected_tokens.clone(),
                    expected_state_id: *expected_state_id,
                    expected: expected.clone(),
                    updated: updated.clone(),
                    player: player.clone(),
                    plugin_receipt: plugin_receipt.clone(),
                },
                SimulationCommand::CommitFurnace {
                    position,
                    expected_state_id,
                    actor_session,
                    expected,
                    updated,
                    player,
                } => RegionalMutationJob::Furnace {
                    transaction: sessions.prepare_furnace_transaction(*actor_session, *position),
                    position: *position,
                    expected_state_id: *expected_state_id,
                    expected: expected.clone(),
                    updated: updated.clone(),
                    player: player.clone(),
                },
                SimulationCommand::CommitOpaqueBlockEntity {
                    position,
                    expected_state,
                    expected_token,
                    bytes,
                } => RegionalMutationJob::OpaqueBlockEntity {
                    position: *position,
                    expected_state: *expected_state,
                    expected_token: *expected_token,
                    bytes: bytes.clone(),
                },
                SimulationCommand::CommitCampfireUse(command) => RegionalMutationJob::CampfireUse {
                    actor_session: command.actor_session,
                    transaction: sessions.prepare_campfire_use_transaction(command.actor_session),
                    plan: Box::new(command.plan.clone()),
                },
                _ => unreachable!("regional mutation run was preflighted"),
            };
            let lane = ((region.x as u32).wrapping_mul(31) ^ region.z as u32) as usize % lane_count;
            lanes.entry(lane).or_default().push(RegionalBlockEditJob {
                sequence: envelope.sequence,
                kind: envelope.command.kind(),
                journal_id: journal_ids.get(&envelope.sequence).copied(),
                #[cfg(test)]
                region,
                command,
            });
        }

        let mut admitted_lanes = Vec::with_capacity(lanes.len());
        for (lane, jobs) in lanes {
            let admission_started = std::time::Instant::now();
            let permit = match resources.acquire_cpu().await {
                Ok(permit) => permit,
                Err(_) => {
                    for envelope in run {
                        envelope.respond(Err(SimulationRequestError::OwnerStopped));
                    }
                    return SimulationTickReport {
                        processed: 0,
                        remaining_depth: self.metrics.depth.load(Ordering::Relaxed),
                        ..SimulationTickReport::default()
                    };
                }
            };
            admitted_lanes.push((lane, jobs, permit, elapsed_us(admission_started)));
        }

        let mut workers = tokio::task::JoinSet::new();
        for (lane, jobs, permit, cpu_admission_wait_us) in admitted_lanes {
            let mutation = mutation.clone();
            let world_read = world_read.clone();
            let block_light = block_light_owned.cloned();
            #[cfg(test)]
            let probe = self.regional_block_edit_probe.clone();
            workers.spawn_blocking(move || {
                let _permit = permit;
                let commands = jobs
                    .into_iter()
                    .map(|job| {
                        #[cfg(test)]
                        if let Some(probe) = probe.as_ref() {
                            probe.enter(job.region);
                        }
                        let started = std::time::Instant::now();
                        let kind = job.kind;
                        let journal_id = job.journal_id;
                        let result = match job.command {
                            RegionalMutationJob::BlockEdits {
                                actor_session,
                                edits,
                                preconditions,
                                scheduled_block_ticks,
                                leaf_trigger,
                                zone_fence,
                                hook_approval,
                                plugin_receipt,
                            } => {
                                let failure = if zone_fence
                                    .as_ref()
                                    .is_some_and(|fence| !fence.is_current())
                                {
                                    Some(SimulationRequestError::Precommit(
                                        mc_script::precommit::HookFailure::PermissionDenied,
                                    ))
                                } else {
                                    super::precommit::refuse_build_approval(&hook_approval)
                                        .err()
                                        .map(SimulationRequestError::Precommit)
                                };
                                let (
                                    outcome,
                                    journal_snapshots,
                                    journal_snapshot_complete,
                                    light_sources,
                                    light_updates,
                                ) = if failure.is_some() {
                                    (None, journal_id.map(|_| Vec::new()), true, None, Vec::new())
                                } else {
                                    let (raw_outcome, touched_chunks) =
                                        if let Some(decision_id) = journal_id {
                                            mutation.apply_block_edits_conditionally_journaled(
                                                decision_id,
                                                &edits,
                                                &preconditions,
                                                &scheduled_block_ticks,
                                                block_light.as_deref(),
                                                leaf_trigger
                                                    .then_some(world_tick.saturating_add(1)),
                                            )
                                        } else {
                                            (
                                                mutation.apply_block_edits_conditionally(
                                                    &edits,
                                                    &preconditions,
                                                    &scheduled_block_ticks,
                                                    block_light.as_deref(),
                                                    leaf_trigger
                                                        .then_some(world_tick.saturating_add(1)),
                                                ),
                                                Vec::new(),
                                            )
                                        };
                                    let outcome = resident_block_edit_result_outcome(raw_outcome);
                                    let journal_snapshots = journal_id.map(|_| {
                                        let snapshot = world_read.snapshot_chunks(&touched_chunks);
                                        touched_chunks
                                            .iter()
                                            .filter_map(|position| snapshot.chunk(*position))
                                            .collect::<Vec<_>>()
                                    });
                                    let journal_snapshot_complete =
                                        journal_snapshots.as_ref().is_none_or(|snapshots| {
                                            snapshots.len() == touched_chunks.len()
                                        });
                                    let (light_sources, light_updates) = regional_light_updates(
                                        &world_read,
                                        block_light.as_deref(),
                                        outcome.as_ref(),
                                    );
                                    (
                                        outcome,
                                        journal_snapshots,
                                        journal_snapshot_complete,
                                        light_sources,
                                        light_updates,
                                    )
                                };
                                RegionalBlockEditJobResult::BlockEdits {
                                    sequence: job.sequence,
                                    actor_session,
                                    outcome: Box::new(outcome),
                                    failure,
                                    journal_snapshots,
                                    journal_snapshot_complete,
                                    light_sources,
                                    light_updates,
                                    plugin_receipt,
                                }
                            }
                            RegionalMutationJob::SurvivalPlacement {
                                actor_session,
                                transaction,
                                plan,
                            } => {
                                let committed = transaction.map_or(Ok(None), |transaction| {
                                    transaction.commit(
                                        &mutation,
                                        block_light.as_deref(),
                                        world_tick,
                                        &plan,
                                    )
                                });
                                let outcome = committed
                                    .as_ref()
                                    .ok()
                                    .and_then(Option::as_ref)
                                    .map(|committed| &committed.block);
                                let needs_fluid_ticks = outcome.is_some_and(|outcome| {
                                    applied_edits_need_fluid_ticks(
                                        &world_read,
                                        &plan.block_facts,
                                        &outcome.applied,
                                    )
                                });
                                let (light_sources, light_updates) = regional_light_updates(
                                    &world_read,
                                    block_light.as_deref(),
                                    outcome,
                                );
                                RegionalBlockEditJobResult::SurvivalPlacement {
                                    sequence: job.sequence,
                                    actor_session,
                                    committed: Box::new(committed),
                                    block_facts: Arc::clone(&plan.block_facts),
                                    needs_fluid_ticks,
                                    light_sources,
                                    light_updates,
                                }
                            }
                            RegionalMutationJob::SurvivalBreak {
                                actor_session,
                                transaction,
                                request,
                                planning_snapshot,
                            } => {
                                let plan = match request {
                                    SurvivalBreakRequest::Prepared(plan) => Some(plan),
                                    SurvivalBreakRequest::Block(request) => {
                                        prepare_survival_block_break_plan(
                                            &planning_snapshot,
                                            &request,
                                        )
                                    }
                                    SurvivalBreakRequest::PrecommitFailure(_) => {
                                        unreachable!("precommit refusals use the canonical owner path")
                                    }
                                };
                                let mut committed = match plan.as_ref() {
                                    None => Ok(None),
                                    Some(plan) if !valid_survival_break_plan(plan) => {
                                        Err(SimulationRequestError::InvalidCommand)
                                    }
                                    Some(plan) => transaction.map_or(Ok(None), |transaction| {
                                        transaction.commit(
                                            &mutation,
                                            block_light.as_deref(),
                                            world_tick,
                                            plan,
                                        )
                                    }),
                                };
                                let mut falling_spawns = Vec::new();
                                if let (Some(plan), Ok(Some(committed))) =
                                    (plan.as_ref(), committed.as_mut())
                                    && let Some(entity_type_id) = plan.falling_block_entity_type_id
                                {
                                    let air = air_state_id(&plan.blocks);
                                    let falling_chunks =
                                        falling_block_start_chunks(&committed.block.applied);
                                    let post_commit_snapshot =
                                        world_read.snapshot_chunks(&falling_chunks);
                                    let start_plan = plan_falling_block_starts(
                                        &plan.blocks,
                                        &plan.block_facts,
                                        &post_commit_snapshot,
                                        &committed.block.applied,
                                        air,
                                    );
                                    let removal_edits = start_plan
                                        .starts
                                        .iter()
                                        .map(|start| BlockEdit {
                                            pos: start.pos,
                                            new_state: air,
                                        })
                                        .collect::<Vec<_>>();
                                    if let Some(falling) = resident_block_edit_outcome(
                                        &mutation,
                                        block_light.as_deref(),
                                        world_tick,
                                        &removal_edits,
                                        &start_plan.preconditions,
                                        &[],
                                    ) {
                                        for edit in &falling.applied {
                                            if is_falling_block_state(&plan.blocks, edit.previous) {
                                                falling_spawns.push((
                                                    entity_type_id,
                                                    Vec3::new(
                                                        f64::from(edit.pos.x) + 0.5,
                                                        f64::from(edit.pos.y),
                                                        f64::from(edit.pos.z) + 0.5,
                                                    ),
                                                    edit.previous,
                                                ));
                                            }
                                        }
                                        append_block_edit_outcome(&mut committed.block, falling);
                                    }
                                }
                                let outcome = committed
                                    .as_ref()
                                    .ok()
                                    .and_then(Option::as_ref)
                                    .map(|committed| &committed.block);
                                let block_facts =
                                    plan.as_ref().map(|plan| Arc::clone(&plan.block_facts));
                                let needs_fluid_ticks = outcome.is_some_and(|outcome| {
                                    block_facts.as_ref().is_some_and(|block_facts| {
                                        applied_edits_need_fluid_ticks(
                                            &world_read,
                                            block_facts,
                                            &outcome.applied,
                                        )
                                    })
                                });
                                let (light_sources, light_updates) = regional_light_updates(
                                    &world_read,
                                    block_light.as_deref(),
                                    outcome,
                                );
                                RegionalBlockEditJobResult::SurvivalBreak {
                                    sequence: job.sequence,
                                    actor_session,
                                    committed: Box::new(committed),
                                    plan: plan.map(Box::new),
                                    falling_spawns,
                                    block_facts,
                                    needs_fluid_ticks,
                                    light_sources,
                                    light_updates,
                                }
                            }
                            RegionalMutationJob::BucketUse {
                                actor_session,
                                transaction,
                                plan,
                            } => {
                                let committed = transaction.map_or(Ok(None), |transaction| {
                                    transaction.commit(
                                        &mutation,
                                        block_light.as_deref(),
                                        world_tick,
                                        &plan,
                                    )
                                });
                                let outcome = committed
                                    .as_ref()
                                    .ok()
                                    .and_then(Option::as_ref)
                                    .map(|committed| &committed.block);
                                let (light_sources, light_updates) = regional_light_updates(
                                    &world_read,
                                    block_light.as_deref(),
                                    outcome,
                                );
                                RegionalBlockEditJobResult::BucketUse {
                                    sequence: job.sequence,
                                    actor_session,
                                    committed: Box::new(committed),
                                    block_facts: Arc::clone(&plan.block_facts),
                                    schedule_fluid_ticks: plan.schedule_fluid_ticks,
                                    light_sources,
                                    light_updates,
                                }
                            }
                            RegionalMutationJob::Chest {
                                transaction,
                                primary_position,
                                positions,
                                expected_tokens,
                                expected_state_id,
                                expected,
                                updated,
                                player,
                                plugin_receipt,
                            } => {
                                if let Some(receipt) = plugin_receipt {
                                    let committed = transaction.map_or(
                                        Err(SimulationRequestError::StaleSession),
                                        |transaction| {
                                            transaction.commit_server_owned(
                                                &mutation,
                                                ChestTransactionRequest {
                                                    primary_position,
                                                    positions: &positions,
                                                    expected_tokens: None,
                                                    expected_state_id,
                                                    expected: &expected,
                                                    updated: &updated,
                                                    player: player.as_deref(),
                                                },
                                            )
                                        },
                                    );
                                    let (outcome, decision, dispatches) = match (committed, journal_id)
                                    {
                                        (
                                            Ok(ServerOwnedChestCommit::Committed(dispatches)),
                                            Some(decision_id),
                                        ) => {
                                            let container_chunk = ChunkPos {
                                                x: primary_position.x.div_euclid(16),
                                                z: primary_position.z.div_euclid(16),
                                            };
                                            match mutation.stamp_chunks_for_world_journal(
                                                decision_id,
                                                std::slice::from_ref(&container_chunk),
                                            ) {
                                                JournalStampResult::Stamped(images) => (
                                                    Ok(WarehouseTransferOutcome::Committed {
                                                        decision_id,
                                                    }),
                                                    Some(RegionalWarehouseDecision::Stamped {
                                                        images,
                                                        receipt,
                                                    }),
                                                    dispatches,
                                                ),
                                                JournalStampResult::NewerDecision(newer) => {
                                                    warn!(
                                                        decision_id,
                                                        newer,
                                                        "server-owned container stamp lost its decision"
                                                    );
                                                    (
                                                        Err(SimulationRequestError::WorldMutationFailed),
                                                        Some(RegionalWarehouseDecision::Unjournaled),
                                                        Vec::new(),
                                                    )
                                                }
                                                JournalStampResult::Missing => {
                                                    warn!(
                                                        decision_id,
                                                        "server-owned container chunk is not resident"
                                                    );
                                                    (
                                                        Err(SimulationRequestError::WorldMutationFailed),
                                                        Some(RegionalWarehouseDecision::Unjournaled),
                                                        Vec::new(),
                                                    )
                                                }
                                            }
                                        }
                                        (Ok(ServerOwnedChestCommit::Committed(_)), None) => (
                                            Err(SimulationRequestError::WorldMutationFailed),
                                            Some(RegionalWarehouseDecision::Unjournaled),
                                            Vec::new(),
                                        ),
                                        (Ok(ServerOwnedChestCommit::StalePlayer), _) => (
                                            Ok(WarehouseTransferOutcome::StalePlayer),
                                            Some(RegionalWarehouseDecision::Refused),
                                            Vec::new(),
                                        ),
                                        (Ok(ServerOwnedChestCommit::StaleContainer), _) => (
                                            Ok(WarehouseTransferOutcome::StaleContainer),
                                            Some(RegionalWarehouseDecision::Refused),
                                            Vec::new(),
                                        ),
                                        (Ok(ServerOwnedChestCommit::MissingContainer), _) => (
                                            Ok(WarehouseTransferOutcome::MissingContainer),
                                            Some(RegionalWarehouseDecision::Refused),
                                            Vec::new(),
                                        ),
                                        (Err(error), _) => (
                                            Err(error),
                                            Some(RegionalWarehouseDecision::Refused),
                                            Vec::new(),
                                        ),
                                    };
                                    RegionalBlockEditJobResult::WarehouseTransfer {
                                        sequence: job.sequence,
                                        outcome: Box::new(outcome),
                                        decision,
                                        dispatches,
                                    }
                                } else {
                                    let outcome = match (transaction, player.as_deref()) {
                                        (Some(transaction), Some(player)) => {
                                            transaction.commit(
                                                &mutation,
                                                ChestTransactionRequest {
                                                    primary_position,
                                                    positions: &positions,
                                                    expected_tokens: expected_tokens.as_deref(),
                                                    expected_state_id,
                                                    expected: &expected,
                                                    updated: &updated,
                                                    player: Some(player),
                                                },
                                            )
                                        }
                                        (None, _) => Err(SimulationRequestError::StaleSession),
                                        // The command validator refuses a menu
                                        // commit without its player plan.
                                        (Some(_), None) => {
                                            Err(SimulationRequestError::InvalidCommand)
                                        }
                                    };
                                    RegionalBlockEditJobResult::Chest {
                                        sequence: job.sequence,
                                        outcome: Box::new(outcome),
                                        drops: player.map(|player| player.drops.clone()).unwrap_or_default(),
                                    }
                                }
                            }
                            RegionalMutationJob::Furnace {
                                transaction,
                                position,
                                expected_state_id,
                                expected,
                                updated,
                                player,
                            } => {
                                let outcome = transaction.map_or(
                                    Err(SimulationRequestError::StaleSession),
                                    |transaction| {
                                        transaction.commit(
                                            &mutation,
                                            FurnaceTransactionRequest {
                                                position,
                                                expected_state_id,
                                                expected: &expected,
                                                updated: &updated,
                                                player: &player,
                                            },
                                        )
                                    },
                                );
                                RegionalBlockEditJobResult::Furnace {
                                    sequence: job.sequence,
                                    outcome: Box::new(outcome),
                                    drops: player.drops.clone(),
                                    xp_orb: player.xp_orb,
                                }
                            }
                            RegionalMutationJob::OpaqueBlockEntity {
                                position,
                                expected_state,
                                expected_token,
                                bytes,
                            } => {
                                let outcome = match mutation
                                    .commit_opaque_block_entity_conditionally(
                                        position,
                                        expected_state,
                                        expected_token,
                                        bytes,
                                    ) {
                                    mc_world::ResidentOpaqueBlockEntityCommitResult::Applied => {
                                        Ok(true)
                                    }
                                    mc_world::ResidentOpaqueBlockEntityCommitResult::Stale => {
                                        Ok(false)
                                    }
                                    mc_world::ResidentOpaqueBlockEntityCommitResult::Missing => {
                                        Err(SimulationRequestError::WorldUnavailable)
                                    }
                                };
                                RegionalBlockEditJobResult::OpaqueBlockEntity {
                                    sequence: job.sequence,
                                    outcome,
                                }
                            }
                            RegionalMutationJob::CampfireUse {
                                actor_session,
                                transaction,
                                plan,
                            } => {
                                let committed = transaction.map_or(
                                    Err(SimulationRequestError::StaleSession),
                                    |transaction| transaction.commit(&mutation, &plan),
                                );
                                RegionalBlockEditJobResult::CampfireUse {
                                    sequence: job.sequence,
                                    actor_session,
                                    committed: Box::new(committed),
                                    position: plan.position,
                                    client_nbt: plan.client_nbt,
                                }
                            }
                        };
                        RegionalBlockEditJobOutcome {
                            attribution: SimulationCommandAttribution {
                                kind,
                                post_admission_command_us: elapsed_us(started),
                            },
                            result,
                        }
                    })
                    .collect::<Vec<_>>();
                RegionalBlockEditLaneOutcome {
                    lane,
                    cpu_admission_wait_us,
                    commands,
                }
            });
        }

        let mut results = HashMap::with_capacity(run.len());
        let mut lane_attribution = Vec::new();
        while let Some(joined) = workers.join_next().await {
            match joined {
                Ok(lane_outcome) => {
                    lane_attribution.push((
                        lane_outcome.lane,
                        SimulationLaneAttribution {
                            cpu_admission_wait_us: lane_outcome.cpu_admission_wait_us,
                            commands: lane_outcome
                                .commands
                                .iter()
                                .map(|outcome| outcome.attribution)
                                .collect(),
                        },
                    ));
                    results.extend(
                        lane_outcome
                            .commands
                            .into_iter()
                            .map(|outcome| (outcome.result.sequence(), outcome)),
                    );
                }
                Err(error) => {
                    warn!(?error, "regional block-edit worker failed");
                }
            }
        }
        lane_attribution.sort_unstable_by_key(|(lane, _)| *lane);
        let lane_attribution = lane_attribution
            .into_iter()
            .map(|(_, attribution)| attribution)
            .collect::<Vec<_>>();

        // Arm the publication probe before the run appends: the observation is
        // taken inside the publication path itself, so the evidence is the
        // append state when the container's slots leave the run, wherever that
        // statement sits.
        #[cfg(test)]
        arm_warehouse_publication_probe(journal, &journal_ids, &results);

        let world_journal_failed = if let Some(journal) = journal {
            let mut complete = true;
            let mut groups = Vec::with_capacity(run.len());
            for envelope in &run {
                let Some(id) = journal_ids.get(&envelope.sequence).copied() else {
                    complete = false;
                    break;
                };
                let Some(RegionalBlockEditJobOutcome { result, .. }) =
                    results.get(&envelope.sequence)
                else {
                    complete = false;
                    break;
                };
                match result {
                    RegionalBlockEditJobResult::BlockEdits {
                        journal_snapshots: Some(snapshots),
                        journal_snapshot_complete,
                        outcome,
                        plugin_receipt,
                        ..
                    } => {
                        if !journal_snapshot_complete {
                            complete = false;
                            break;
                        }
                        groups.push((
                            id,
                            snapshots.clone(),
                            match outcome.as_ref().as_ref() {
                                Some(_) => plugin_receipt.clone(),
                                None => None,
                            },
                        ));
                    }
                    // A refused server-owned deposit closes its reserved id
                    // with no participant, exactly like a cancelled block-drop
                    // decision, so the run's append stays contiguous.
                    RegionalBlockEditJobResult::WarehouseTransfer {
                        decision: Some(RegionalWarehouseDecision::Refused),
                        ..
                    } => groups.push((id, Vec::new(), None)),
                    RegionalBlockEditJobResult::WarehouseTransfer {
                        decision:
                            Some(RegionalWarehouseDecision::Stamped {
                                images, receipt, ..
                            }),
                        ..
                    } => groups.push((id, images.clone(), Some(receipt.clone()))),
                    _ => {
                        complete = false;
                        break;
                    }
                }
            }
            if !complete {
                warn!("world chunk journal worker snapshot was incomplete");
                sessions.report_world_chunk_journal_failure();
                true
            } else {
                let completions = groups
                    .iter()
                    .map(|(decision_id, snapshots, _)| {
                        (
                            *decision_id,
                            snapshots
                                .iter()
                                .map(|snapshot| snapshot.pos)
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect::<Vec<_>>();
                let journal = journal.clone();
                match tokio::task::spawn_blocking(move || {
                    journal.record_reserved_decisions(world_tick, groups)
                })
                .await
                {
                    Ok(Ok(())) => {
                        for (decision_id, positions) in completions {
                            mutation.clear_journal_pending_conditionally(decision_id, &positions);
                        }
                        false
                    }
                    Ok(Err(error)) => {
                        warn!(
                            outcome_unknown = error.outcome_unknown(),
                            %error,
                            "world chunk journal group append failed"
                        );
                        sessions.report_world_chunk_journal_failure();
                        true
                    }
                    Err(error) => {
                        warn!(?error, "world chunk journal append worker failed");
                        sessions.report_world_chunk_journal_failure();
                        true
                    }
                }
            }
        } else {
            false
        };

        let processed = run.len();
        for envelope in run {
            let Some(outcome) = results.remove(&envelope.sequence) else {
                if let SimulationCommand::CommitChest {
                    treatment: Some(participant),
                    ..
                } = &envelope.command
                    && let Some(treatment) =
                        participant.lock().expect("prepared treatment lock").take()
                {
                    if let Some(mutation) = treatment.mutation {
                        mutation.leave_fenced_for_recovery();
                    }
                    sessions.report_world_chunk_journal_failure();
                }
                envelope.respond(Err(if world_journal_failed {
                    SimulationRequestError::WorldMutationFailed
                } else {
                    SimulationRequestError::OwnerStopped
                }));
                continue;
            };
            let result = outcome.result;
            self.metrics.processed.fetch_add(1, Ordering::Relaxed);
            match &result {
                RegionalBlockEditJobResult::Chest { .. }
                | RegionalBlockEditJobResult::WarehouseTransfer { .. }
                | RegionalBlockEditJobResult::Furnace { .. } => {
                    self.metrics
                        .container_commits_processed
                        .fetch_add(1, Ordering::Relaxed);
                }
                RegionalBlockEditJobResult::OpaqueBlockEntity { .. }
                | RegionalBlockEditJobResult::CampfireUse { .. } => {
                    self.metrics
                        .block_entity_commits_processed
                        .fetch_add(1, Ordering::Relaxed);
                }
                RegionalBlockEditJobResult::BlockEdits { .. }
                | RegionalBlockEditJobResult::SurvivalPlacement { .. }
                | RegionalBlockEditJobResult::SurvivalBreak { .. }
                | RegionalBlockEditJobResult::BucketUse { .. } => {
                    self.metrics
                        .block_edits_processed
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            match result {
                RegionalBlockEditJobResult::BlockEdits {
                    actor_session,
                    mut outcome,
                    failure,
                    light_sources,
                    light_updates,
                    plugin_receipt,
                    ..
                } => {
                    if world_journal_failed
                        && outcome
                            .as_ref()
                            .as_ref()
                            .is_some_and(|outcome| !outcome.applied.is_empty())
                    {
                        self.metrics
                            .rejected_world_mutation
                            .fetch_add(1, Ordering::Relaxed);
                        envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                        continue;
                    }
                    if let Some(error) = failure {
                        envelope.respond(Err(error));
                        continue;
                    }
                    let committed = outcome.is_some();
                    if let Some(outcome) = outcome.as_mut() {
                        publish_regional_light_updates(
                            sessions,
                            mutation,
                            block_light_owned,
                            light_sources.as_ref(),
                            light_updates,
                            outcome,
                        );
                        dispatch_regional_block_outcome(sessions, actor_session, outcome);
                    }
                    if plugin_receipt.is_some() {
                        let decision = committed.then(|| {
                            journal_ids
                                .get(&envelope.sequence)
                                .copied()
                                .expect("journaled structure portion has a decision")
                        });
                        envelope.respond(Ok(SimulationResponse::SettlementPortion(Ok(decision))));
                    } else {
                        envelope
                            .respond(Ok(SimulationResponse::BlockEdits(Ok(Box::new(*outcome)))));
                    }
                }
                RegionalBlockEditJobResult::SurvivalPlacement {
                    actor_session,
                    mut committed,
                    block_facts,
                    needs_fluid_ticks,
                    light_sources,
                    light_updates,
                    ..
                } => {
                    if let Ok(Some(committed)) = committed.as_mut() {
                        if needs_fluid_ticks {
                            schedule_resident_fluid_ticks_near_applied(
                                world_read,
                                mutation,
                                &block_facts,
                                world_tick,
                                &committed.block.applied,
                            );
                        }
                        publish_regional_light_updates(
                            sessions,
                            mutation,
                            block_light_owned,
                            light_sources.as_ref(),
                            light_updates,
                            &mut committed.block,
                        );
                        dispatch_regional_block_outcome(
                            sessions,
                            Some(actor_session),
                            &committed.block,
                        );
                    }
                    envelope.respond(Ok(SimulationResponse::SurvivalPlacement(
                        (*committed).map(|committed| committed.map(Box::new)),
                    )));
                }
                RegionalBlockEditJobResult::SurvivalBreak {
                    actor_session,
                    mut committed,
                    plan,
                    falling_spawns,
                    block_facts,
                    needs_fluid_ticks,
                    light_sources,
                    light_updates,
                    ..
                } => {
                    if let (Ok(Some(committed)), Some(plan)) = (committed.as_mut(), plan.as_deref())
                    {
                        for edit in &committed.block.applied {
                            if is_campfire_block(&plan.blocks, edit.previous)
                                && !is_campfire_block(&plan.blocks, edit.new_state)
                                && sessions.clear_campfire_cooking(edit.pos)
                            {
                                committed.block.cleared_campfires.push(edit.pos);
                            }
                        }
                        if needs_fluid_ticks && let Some(block_facts) = block_facts.as_deref() {
                            schedule_resident_fluid_ticks_near_applied(
                                world_read,
                                mutation,
                                block_facts,
                                world_tick,
                                &committed.block.applied,
                            );
                        }
                        publish_regional_light_updates(
                            sessions,
                            mutation,
                            block_light_owned,
                            light_sources.as_ref(),
                            light_updates,
                            &mut committed.block,
                        );
                        dispatch_regional_block_outcome(
                            sessions,
                            Some(actor_session),
                            &committed.block,
                        );

                        let mut dispatches = std::mem::take(&mut committed.dispatches);
                        for drop in &plan.drops {
                            dispatches.extend(sessions.spawn_item_drop_owned(
                                &self.authority,
                                drop.entity_type_id,
                                drop.position,
                                drop.stack.clone(),
                            ));
                        }
                        for (entity_type_id, position, state) in falling_spawns {
                            dispatches.extend(sessions.spawn_falling_block_owned(
                                &self.authority,
                                entity_type_id,
                                position,
                                state,
                            ));
                        }
                        dispatch_visibility_commands(dispatches);
                    }
                    envelope.respond(Ok(SimulationResponse::SurvivalBreak(
                        (*committed).map(|committed| committed.map(Box::new)),
                    )));
                }
                RegionalBlockEditJobResult::BucketUse {
                    actor_session,
                    mut committed,
                    block_facts,
                    schedule_fluid_ticks,
                    light_sources,
                    light_updates,
                    ..
                } => {
                    if let Ok(Some(committed)) = committed.as_mut() {
                        if schedule_fluid_ticks {
                            schedule_resident_fluid_ticks_near_applied(
                                world_read,
                                mutation,
                                &block_facts,
                                world_tick,
                                &committed.block.applied,
                            );
                        }
                        publish_regional_light_updates(
                            sessions,
                            mutation,
                            block_light_owned,
                            light_sources.as_ref(),
                            light_updates,
                            &mut committed.block,
                        );
                        dispatch_regional_block_outcome(
                            sessions,
                            Some(actor_session),
                            &committed.block,
                        );
                    }
                    envelope.respond(Ok(SimulationResponse::BucketUse(
                        (*committed).map(|committed| committed.map(Box::new)),
                    )));
                }
                RegionalBlockEditJobResult::Chest {
                    mut outcome, drops, ..
                } => {
                    if let Ok(SharedContainerCommit::Committed { dispatches, .. }) =
                        outcome.as_mut()
                    {
                        for drop in drops {
                            dispatches.extend(sessions.spawn_item_drop_owned(
                                &self.authority,
                                drop.entity_type_id,
                                drop.position,
                                drop.stack,
                            ));
                        }
                        dispatch_visibility_commands(std::mem::take(dispatches));
                    }
                    envelope.respond(Ok(SimulationResponse::ChestCommit(
                        (*outcome).map(Box::new),
                    )));
                }
                RegionalBlockEditJobResult::WarehouseTransfer {
                    outcome,
                    decision,
                    dispatches,
                    ..
                } => {
                    let treatment = match &envelope.command {
                        SimulationCommand::CommitChest {
                            treatment: Some(participant),
                            ..
                        } => participant.lock().expect("prepared treatment lock").take(),
                        _ => None,
                    };
                    if world_journal_failed {
                        // The append may have reached durable storage before a
                        // journal error. Keep the regional phase fenced until
                        // recovery rather than letting the patient die unpaid.
                        if let Some(mutation) = treatment.and_then(|treatment| treatment.mutation) {
                            mutation.leave_fenced_for_recovery();
                        }
                        self.metrics
                            .rejected_world_mutation
                            .fetch_add(1, Ordering::Relaxed);
                        envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                        continue;
                    }
                    debug_assert!(
                        !matches!(decision, Some(RegionalWarehouseDecision::Unjournaled)),
                        "an unjournaled container commit fails the run's append"
                    );
                    if matches!(
                        outcome.as_ref(),
                        Ok(WarehouseTransferOutcome::Committed { .. })
                    ) {
                        if let Some(treatment) = treatment {
                            let accepted = treatment.accepted;
                            let journal =
                                (*journal.expect("paid treatment requires world journal")).clone();
                            let decision_id = journal_ids[&envelope.sequence];
                            let committed = tokio::task::spawn_blocking(move || {
                                treatment
                                    .mutation
                                    .expect("fenced treatment")
                                    .commit()
                                    .is_ok()
                                    && journal.writer.flush().is_ok()
                                    && journal.confirm_treatment_committed(decision_id).is_ok()
                            })
                            .await;
                            if !matches!(committed, Ok(true)) {
                                sessions.report_world_chunk_journal_failure();
                                envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                                continue;
                            }
                            sessions.publish_resident_treatment(&accepted);
                        }
                        dispatch_visibility_commands(dispatches);
                    }
                    envelope.respond(Ok(SimulationResponse::WarehouseTransfer(*outcome)));
                }
                RegionalBlockEditJobResult::Furnace {
                    mut outcome,
                    drops,
                    xp_orb,
                    ..
                } => {
                    if let Ok(SharedContainerCommit::Committed { dispatches, .. }) =
                        outcome.as_mut()
                    {
                        for drop in drops {
                            dispatches.extend(sessions.spawn_item_drop_owned(
                                &self.authority,
                                drop.entity_type_id,
                                drop.position,
                                drop.stack,
                            ));
                        }
                        if let Some(xp_orb) = xp_orb {
                            dispatches.extend(sessions.spawn_xp_orb_owned(
                                &self.authority,
                                xp_orb.entity_type_id,
                                xp_orb.position,
                                xp_orb.value,
                            ));
                        }
                        dispatch_visibility_commands(std::mem::take(dispatches));
                    }
                    envelope.respond(Ok(SimulationResponse::FurnaceCommit(
                        (*outcome).map(Box::new),
                    )));
                }
                RegionalBlockEditJobResult::OpaqueBlockEntity { outcome, .. } => {
                    envelope.respond(Ok(SimulationResponse::OpaqueBlockEntity(outcome)));
                }
                RegionalBlockEditJobResult::CampfireUse {
                    actor_session,
                    committed,
                    position,
                    client_nbt,
                    ..
                } => {
                    if committed.as_ref().as_ref().is_ok_and(Option::is_some) {
                        dispatch_visibility_commands(sessions.block_entity_data_dispatches(
                            position,
                            Some(actor_session),
                            CAMPFIRE_BLOCK_ENTITY_TYPE_ID,
                            client_nbt,
                        ));
                    }
                    envelope.respond(Ok(SimulationResponse::CampfireUse(
                        (*committed).map(|committed| committed.map(Box::new)),
                    )));
                }
            }
        }

        #[cfg(test)]
        crate::play::session::publication_probe::disarm();

        SimulationTickReport {
            processed: processed + refused,
            remaining_depth: self.metrics.depth.load(Ordering::Relaxed),
            lane_attribution,
        }
    }
}

/// Test-only arming for the append-before-publication rule.
///
/// The observation itself is taken inside `dispatch_visibility_commands`, at the
/// point the container's `ChestSlots` command leaves the run, so the journal
/// records the append state at publication rather than at some adjacent
/// statement. Arming every server-owned warehouse publication the run holds
/// before it appends keeps that evidence true wherever the publication sits.
#[cfg(test)]
fn arm_warehouse_publication_probe(
    journal: Option<&crate::play::world_journal::WorldChunkJournal>,
    journal_ids: &HashMap<u64, u64>,
    results: &HashMap<u64, RegionalBlockEditJobOutcome>,
) {
    let Some(journal) = journal else {
        return;
    };
    for (sequence, job_outcome) in results {
        let RegionalBlockEditJobResult::WarehouseTransfer {
            outcome,
            dispatches,
            ..
        } = &job_outcome.result
        else {
            continue;
        };
        if outcome.is_err() {
            continue;
        }
        let Some(position) = dispatches
            .iter()
            .find_map(|dispatch| match &dispatch.command {
                OutboundCommand::ChestSlots { position, .. } => Some(*position),
                _ => None,
            })
        else {
            continue;
        };
        let Some(decision_id) = journal_ids.get(sequence).copied() else {
            continue;
        };
        crate::play::session::publication_probe::arm(journal, decision_id, position);
    }
}
