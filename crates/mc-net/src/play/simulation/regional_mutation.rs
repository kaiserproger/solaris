use super::{
    Arc, BlockEdit, BlockEditBatchOutcome, BlockLightTable, BlockMutationToken, BlockPos,
    BlockStateId, BucketUsePlan, BucketUseTransaction, CAMPFIRE_BLOCK_ENTITY_TYPE_ID,
    CampfireUsePlan, CampfireUseTransaction, ChestBlockEntity, ChestCommitOutcome,
    ChestTransaction, ChestTransactionRequest, CommittedBucketUse, CommittedCampfireUse,
    CommittedSurvivalBreak, CommittedSurvivalPlacement, ContainerDropPlan, ContainerPlayerPlan,
    ContainerXpPlan, FurnaceBlockEntity, FurnaceCommitOutcome, FurnaceTransaction,
    FurnaceTransactionRequest, HashMap, IncrementalLightSources, Ordering, ResidentBlockEdit,
    ResidentBlockPrecondition, ScheduledBlockTick, SessionId, SessionRegistry,
    SharedContainerCommit, SimulationCommand, SimulationCommandAttribution,
    SimulationCommandEnvelope, SimulationLaneAttribution, SimulationOwner, SimulationRequestError,
    SimulationResponse, SimulationTickReport, SimulationWorldAccess, SurvivalBreakPlan,
    SurvivalBreakRequest, SurvivalBreakTransaction, SurvivalPlacementPlan,
    SurvivalPlacementTransaction, Vec3, air_state_id, append_block_edit_outcome,
    applied_edits_need_fluid_ticks, command_single_owner_region, dispatch_regional_block_outcome,
    dispatch_visibility_commands, elapsed_us, falling_block_start_chunks, is_campfire_block,
    is_falling_block_state, plan_falling_block_starts, prepare_survival_block_break_plan,
    publish_regional_light_updates, regional_light_updates, resident_block_edit_outcome,
    resident_block_edit_result_outcome, resident_block_edits, resident_block_preconditions,
    schedule_resident_fluid_ticks_near_applied, snapshot_region, valid_survival_break_plan, warn,
};
use std::collections::BTreeMap;

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
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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
        actor_session: SessionId,
        edits: Vec<ResidentBlockEdit>,
        preconditions: Vec<ResidentBlockPrecondition>,
        scheduled_block_ticks: Vec<ScheduledBlockTick>,
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
        expected_state_id: i32,
        expected: Vec<ChestBlockEntity>,
        updated: Vec<ChestBlockEntity>,
        player: Box<ContainerPlayerPlan>,
    },
    Furnace {
        transaction: Option<FurnaceTransaction>,
        position: BlockPos,
        expected_state_id: i32,
        expected: FurnaceBlockEntity,
        updated: FurnaceBlockEntity,
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

enum RegionalBlockEditJobResult {
    BlockEdits {
        sequence: u64,
        actor_session: SessionId,
        outcome: Box<Option<BlockEditBatchOutcome>>,
        journal_snapshots: Option<Vec<mc_world::ChunkSnapshot>>,
        journal_snapshot_complete: bool,
        light_sources: Option<IncrementalLightSources>,
        light_updates: Vec<crate::play::session::OutboundLightUpdate>,
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
        block_light: Option<&BlockLightTable>,
        journal: Option<&crate::play::world_journal::WorldChunkJournal>,
        run: Vec<SimulationCommandEnvelope>,
    ) -> SimulationTickReport {
        let world_read = access.read.expect("regional block-edit read view");
        let mutation = access.mutation.expect("regional block-edit mutation view");
        let resources = access.cpu.expect("regional block-edit CPU admission");
        let block_light_owned = access.light;
        let lane_count = resources.cpu_limit().max(1);
        let world_tick = sessions.simulation_tick();
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
                } => RegionalMutationJob::BlockEdits {
                    actor_session: *actor_session,
                    edits: resident_block_edits(edits, preconditions, block_light),
                    preconditions: resident_block_preconditions(preconditions),
                    scheduled_block_ticks: scheduled_block_ticks.clone(),
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
                    expected_state_id,
                    actor_session,
                    expected,
                    updated,
                    player,
                } => RegionalMutationJob::Chest {
                    transaction: sessions
                        .prepare_chest_transaction(*actor_session, *primary_position),
                    primary_position: *primary_position,
                    positions: positions.clone(),
                    expected_state_id: *expected_state_id,
                    expected: expected.clone(),
                    updated: updated.clone(),
                    player: player.clone(),
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
                            } => {
                                let (raw_outcome, touched_chunks) =
                                    if let Some(decision_id) = journal_id {
                                        mutation.apply_block_edits_conditionally_journaled(
                                            decision_id,
                                            &edits,
                                            &preconditions,
                                            &scheduled_block_ticks,
                                            block_light.as_deref(),
                                            Some(world_tick.saturating_add(1)),
                                        )
                                    } else {
                                        (
                                            mutation.apply_block_edits_conditionally(
                                                &edits,
                                                &preconditions,
                                                &scheduled_block_ticks,
                                                block_light.as_deref(),
                                                Some(world_tick.saturating_add(1)),
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
                                RegionalBlockEditJobResult::BlockEdits {
                                    sequence: job.sequence,
                                    actor_session,
                                    outcome: Box::new(outcome),
                                    journal_snapshots,
                                    journal_snapshot_complete,
                                    light_sources,
                                    light_updates,
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
                                            ChestTransactionRequest {
                                                primary_position,
                                                positions: &positions,
                                                expected_state_id,
                                                expected: &expected,
                                                updated: &updated,
                                                player: &player,
                                            },
                                        )
                                    },
                                );
                                RegionalBlockEditJobResult::Chest {
                                    sequence: job.sequence,
                                    outcome: Box::new(outcome),
                                    drops: player.drops.clone(),
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

        let world_journal_failed = if let Some(journal) = journal {
            let mut complete = true;
            let mut groups = Vec::with_capacity(run.len());
            for envelope in &run {
                let Some(id) = journal_ids.get(&envelope.sequence).copied() else {
                    complete = false;
                    break;
                };
                let Some(RegionalBlockEditJobOutcome {
                    result:
                        RegionalBlockEditJobResult::BlockEdits {
                            journal_snapshots: Some(snapshots),
                            journal_snapshot_complete,
                            ..
                        },
                    ..
                }) = results.get(&envelope.sequence)
                else {
                    complete = false;
                    break;
                };
                if !journal_snapshot_complete {
                    complete = false;
                    break;
                }
                groups.push((id, snapshots.clone()));
            }
            if !complete {
                warn!("world chunk journal worker snapshot was incomplete");
                sessions.report_world_chunk_journal_failure();
                true
            } else {
                let completions = groups
                    .iter()
                    .map(|(decision_id, snapshots)| {
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
                    journal.record_reserved_snapshot_groups(world_tick, groups)
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
                    light_sources,
                    light_updates,
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
                    envelope.respond(Ok(SimulationResponse::BlockEdits(Ok(Box::new(*outcome)))));
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
                        dispatch_regional_block_outcome(sessions, actor_session, &committed.block);
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
                        dispatch_regional_block_outcome(sessions, actor_session, &committed.block);

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
                        dispatch_regional_block_outcome(sessions, actor_session, &committed.block);
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

        SimulationTickReport {
            processed,
            remaining_depth: self.metrics.depth.load(Ordering::Relaxed),
            lane_attribution,
        }
    }
}
