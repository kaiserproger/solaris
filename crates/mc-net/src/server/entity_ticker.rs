use super::*;

pub(super) struct EntityTickerContext {
    pub(super) prewarmed_entity_pathing_states: std::num::NonZeroUsize,
    pub(super) entity_world_journal_failure: tokio::sync::watch::Receiver<bool>,
    pub(super) entity_shutdown_requested: tokio::sync::oneshot::Receiver<()>,
    pub(super) simulation_owner: play::SimulationOwner,
    pub(super) entity_config: Arc<ServerConfig>,
    pub(super) entity_sessions: Arc<play::SessionRegistry>,
    pub(super) entity_chunk_pipeline_resources: ChunkPipelineResources,
    pub(super) entity_world_read: Option<mc_world::WorldReadView>,
    pub(super) entity_world_mutation: Option<mc_world::WorldMutationView>,
    pub(super) entity_scheduled_ticks: Option<mc_world::ScheduledTickView>,
    pub(super) periodic_save_requests: Option<crate::dirty_flush::DirtyFlushNotifier>,
    pub(super) entity_runtime_control: Option<RuntimeControlHandle>,
    pub(super) entity_runtime_control_signals: Option<RuntimeControlSignalReceiver>,
    pub(super) entity_tick_metrics: RuntimeTickMetricsHandle,
    pub(super) entity_pathing_materials: Option<Arc<BlockMaterialIds>>,
    pub(super) entity_scripts: Option<ScriptEventSink>,
    pub(super) entity_script_zones: Option<PluginZoneAdapter>,
}

fn inline_projectile_facts(
    tick: u64,
    queries: &[play::EntityPhysicsQuery],
    snapshot: Option<Arc<EntityPhysicsSnapshot>>,
    steps: &[play::EntityPhysicsStep],
) -> play::EntityProjectilePhysicsFacts {
    snapshot.map_or_else(Default::default, |snapshot| {
        entity_projectile_physics_facts_from_steps(tick, queries, &snapshot, steps)
    })
}

pub(super) async fn run_entity_ticker(context: EntityTickerContext) {
    let EntityTickerContext {
        prewarmed_entity_pathing_states,
        mut entity_world_journal_failure,
        mut entity_shutdown_requested,
        mut simulation_owner,
        entity_config,
        entity_sessions,
        entity_chunk_pipeline_resources,
        entity_world_read,
        entity_world_mutation,
        entity_scheduled_ticks,
        periodic_save_requests,
        entity_runtime_control,
        mut entity_runtime_control_signals,
        entity_tick_metrics,
        entity_pathing_materials,
        entity_scripts,
        entity_script_zones,
    } = context;

    let _pathing_tables_ready = prewarmed_entity_pathing_states;
    let mut ticker = tokio::time::interval(play::ENTITY_TICK_PERIOD);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let metrics_policy = RuntimeMetricsPolicy::default().normalized();
    let mut metrics_log_gate = RuntimeMetricsLogGate::default();
    let simulation_policy = entity_config.random_tick.normalized();
    let mut natural_spawn_ticker = natural_spawn_ticker::NaturalSpawnTicker::new(simulation_policy);
    let mut tick_metrics = RuntimeTickMetricsWindow::default();
    let (tick_metrics_publisher, mut tick_metrics_observations, tick_metrics_worker) =
        spawn_runtime_tick_metrics_worker(entity_tick_metrics.clone());
    let (memory_pressure_sampler, memory_pressure_worker) =
        if let Some(control) = entity_runtime_control.as_ref() {
            let (sampler, worker) = control.spawn_memory_pressure_sampler();
            (Some(sampler), Some(worker))
        } else {
            (None, None)
        };
    let mut tick = 0_u64;
    let villager_population_ids = villager_population_ids(&entity_config);
    let village_defense_golem_type_id =
        configured_entity_type_id(&entity_config, "minecraft:iron_golem");
    let mut session_empty_generation = entity_sessions.session_empty_generation();
    let mut player_save_generation = entity_sessions.player_save_generation();
    let mut simulation_command_window = SimulationCommandTelemetryWindow::default();
    let mut simulation_command_gate = SimulationCommandGate::default();
    let mut pushed_simulation_lane_attribution = Vec::new();
    let mut entity_physics_job = None;
    let mut entity_update_budget =
        crate::runtime_entity_budget::EntityUpdateBudgetController::default();
    let mut movement_publication_budget =
        crate::runtime_entity_budget::MovementPublicationBudgetController::default();
    let mut entity_budget_last_reliable_drops = 0_u64;
    let mut scheduled_budget_exhausted_since_publish = false;
    let mut inhabited_time = play::InhabitedTimeAccumulator::default();
    loop {
        let command_arrived = tokio::select! {
            biased;
            result = entity_world_journal_failure.changed() => {
                result.expect("session registry owns the world journal failure sender");
                if *entity_world_journal_failure.borrow_and_update() {
                    warn!("world chunk journal failed; requesting controlled shutdown");
                    entity_config.shutdown.request();
                }
                continue;
            }
            _ = &mut entity_shutdown_requested => {
                if let Some(job) = entity_physics_job.take() {
                    apply_entity_physics_job_result(
                        job.await,
                        &simulation_owner,
                        &entity_config,
                        &entity_sessions,
                        &entity_chunk_pipeline_resources,
                        entity_world_read.as_ref(),
                    )
                    .await;
                }
                persist_inhabited_time_tail(
                    &entity_config,
                    entity_world_mutation.as_ref(),
                    &mut inhabited_time,
                )
                .await;
                simulation_owner.shutdown();
                tick_metrics_publisher.try_publish(
                    tick,
                    &tick_metrics,
                    scheduled_budget_exhausted_since_publish,
                );
                info!("simulation drain fenced; entity ticker stopping");
                break;
            }
            generation = wait_for_session_empty_save_request(
                &entity_sessions,
                session_empty_generation,
                periodic_save_requests.as_ref(),
                tick,
            ) => {
                session_empty_generation = generation;
                continue;
            }
            generation = wait_for_player_save_request(
                &entity_sessions,
                player_save_generation,
                periodic_save_requests.as_ref(),
                tick,
            ) => {
                player_save_generation = generation;
                continue;
            }
            result = wait_for_entity_physics_job(&mut entity_physics_job) => {
                let _completed_job = entity_physics_job.take();
                apply_entity_physics_job_result(
                    result,
                    &simulation_owner,
                    &entity_config,
                    &entity_sessions,
                    &entity_chunk_pipeline_resources,
                    entity_world_read.as_ref(),
                )
                .await;
                simulation_owner
                    .tick_primed_tnt(
                        &entity_sessions,
                        entity_config.world.as_ref(),
                        entity_config.block_light.as_deref(),
                        &entity_config.block_facts,
                        play::ExplosionRegistries::from_config(&entity_config),
                        entity_pathing_materials.as_deref(),
                        || {
                            entity_script_zones.as_ref().map(|zones| {
                                zones.protection_snapshot().unwrap_or_else(|error| {
                                    warn!(
                                        ?error,
                                        "zone protection snapshot unavailable; denying explosion block damage"
                                    );
                                    crate::script::ZoneProtectionSnapshot::unavailable()
                                })
                            })
                        },
                    )
                    .await;
                continue;
            }
            observation = tick_metrics_observations.recv(), if !tick_metrics_observations.is_closed() => {
                let Some(observation) = observation else {
                    continue;
                };
                if let Some(control) = entity_runtime_control.as_ref() {
                    let outcome = apply_runtime_control_operation(
                        control,
                        &entity_chunk_pipeline_resources,
                        &entity_sessions,
                        &entity_config.shutdown,
                        RuntimeControlOperation::ObserveWork(runtime_work_input(
                            &observation.percentiles,
                            observation.scheduled_budget_exhausted,
                        )),
                    );
                    if let Some(RuntimeControlOutcome::Work(decision)) = outcome.as_ref()
                        && decision.action == crate::AutoscaleAction::ScaleDown
                    {
                        info!(
                            tick,
                            source_tick = observation.percentiles.source_tick,
                            action = ?decision.action,
                            focus = ?decision.focus,
                            entity_pathing_candidates = decision.budgets.entity_pathing_candidates,
                            random_tick_chunk_budget = decision.budgets.random_tick_chunks,
                            scheduled_tick_budget = decision.budgets.scheduled_ticks,
                            reason = %decision.reason,
                            "runtime work budgets changed"
                        );
                    } else if let Some(RuntimeControlOutcome::Work(decision)) = outcome.as_ref()
                        && decision.action == crate::AutoscaleAction::ScaleUp
                    {
                        debug!(
                            tick,
                            source_tick = observation.percentiles.source_tick,
                            entity_pathing_candidates = decision.budgets.entity_pathing_candidates,
                            random_tick_chunk_budget = decision.budgets.random_tick_chunks,
                            scheduled_tick_budget = decision.budgets.scheduled_ticks,
                            reason = %decision.reason,
                            "runtime work budgets recovering"
                        );
                    }
                }
                continue;
            }
            signal = recv_runtime_control_signal(&mut entity_runtime_control_signals) => {
                let Some(signal) = signal else {
                    entity_runtime_control_signals = None;
                    continue;
                };
                if let Some(control) = entity_runtime_control.as_ref() {
                    observe_runtime_control_signal(
                        control,
                        &entity_chunk_pipeline_resources,
                        &entity_sessions,
                        &entity_config.shutdown,
                        signal,
                    );
                }
                continue;
            }
            // An overdue tick is immediately ready. Commands must win the
            // biased select so overloaded ticks cannot starve player actions.
            ready = simulation_owner.wait_for_command(), if simulation_command_gate.accepts_off_tick_batch() => {
                if !ready {
                    if let Some(job) = entity_physics_job.take() {
                        apply_entity_physics_job_result(
                            job.await,
                            &simulation_owner,
                            &entity_config,
                            &entity_sessions,
                            &entity_chunk_pipeline_resources,
                            entity_world_read.as_ref(),
                        )
                        .await;
                    }
                    persist_inhabited_time_tail(
                        &entity_config,
                        entity_world_mutation.as_ref(),
                        &mut inhabited_time,
                    )
                    .await;
                    simulation_owner.shutdown();
                    tick_metrics_publisher.try_publish(
                        tick,
                        &tick_metrics,
                        scheduled_budget_exhausted_since_publish,
                    );
                    info!("simulation command channel closed; entity ticker stopping");
                    break;
                }
                true
            }
            _ = ticker.tick() => false,
        };
        if command_arrived {
            let started = Instant::now();
            let report = simulation_owner
                .process_ready_commands_with_world_views(
                    &entity_sessions,
                    entity_config.world.as_ref(),
                    play::SimulationWorldAccess {
                        read: entity_world_read.as_ref(),
                        mutation: entity_world_mutation.as_ref(),
                        cpu: Some(&entity_chunk_pipeline_resources),
                        light: entity_config.block_light.as_ref(),
                    },
                    entity_config.block_light.as_deref(),
                    play::SIMULATION_COMMAND_BATCH_LIMIT,
                )
                .await;
            simulation_command_window.record_off_tick(elapsed_us(started), report.processed);
            simulation_command_gate.record_off_tick_batch();
            pushed_simulation_lane_attribution.extend(report.lane_attribution);
            continue;
        }
        simulation_command_gate.record_tick_boundary();
        let tick_started = Instant::now();
        tick = entity_sessions.simulation_tick().saturating_add(1);
        if let Some(scripts) = entity_scripts.as_ref() {
            scripts.enqueue_server_tick(tick);
        }
        let work_budgets = entity_runtime_control
            .as_ref()
            .map(|control| control.snapshot().work_budgets)
            .unwrap_or(RuntimeWorkBudgets {
                random_tick_chunks: simulation_policy.chunk_budget,
                scheduled_ticks: simulation_policy.fluid_tick_budget,
                ..RuntimeWorkBudgets::default()
            });

        let started = Instant::now();
        let mut simulation_commands = simulation_owner
            .process_commands_with_world_views(
                &entity_sessions,
                entity_config.world.as_ref(),
                play::SimulationWorldAccess {
                    read: entity_world_read.as_ref(),
                    mutation: entity_world_mutation.as_ref(),
                    cpu: Some(&entity_chunk_pipeline_resources),
                    light: entity_config.block_light.as_ref(),
                },
                entity_config.block_light.as_deref(),
                play::SIMULATION_COMMAND_BATCH_LIMIT,
            )
            .await;
        let simulation_command_telemetry = simulation_command_window
            .finish_tick(elapsed_us(started), simulation_commands.processed);
        simulation_commands.processed = simulation_command_telemetry.processed;
        pushed_simulation_lane_attribution.append(&mut simulation_commands.lane_attribution);
        simulation_commands.lane_attribution =
            std::mem::take(&mut pushed_simulation_lane_attribution);
        let mut simulation_commands_us = simulation_command_telemetry.elapsed_us;
        let simulation_command_scope = simulation_command_telemetry.scope.as_str();
        let mut simulation_command_cpu_admission_wait_us = simulation_commands
            .lane_attribution
            .iter()
            .map(|attribution| attribution.cpu_admission_wait_us)
            .sum::<u64>();
        let mut simulation_command_post_admission_us = simulation_commands
            .lane_attribution
            .iter()
            .flat_map(|lane| &lane.commands)
            .map(|attribution| attribution.post_admission_command_us)
            .sum::<u64>();
        let started = Instant::now();
        let world_time = simulation_owner.advance_world_time(&entity_sessions, 1);
        tick = entity_sessions.simulation_tick();
        entity_sessions.synchronize_entity_lifecycle_epoch(tick);
        simulation_owner.tick_dying_entities(&entity_sessions, entity_sessions.simulation_tick());
        let world_time_us = elapsed_us(started);
        natural_spawn_ticker.tick(
            &entity_sessions,
            tick,
            entity_world_read.as_ref(),
            entity_pathing_materials.as_deref(),
        );
        let started = Instant::now();
        simulation_owner
            .run_sheep_grazing(
                &entity_config,
                &entity_sessions,
                entity_world_read.as_ref(),
                entity_world_mutation.as_ref(),
                tick,
            )
            .await;
        let sheep_grazing_us = elapsed_us(started);
        let mut animal_breeding_us = 0;
        let physics_was_in_flight = entity_physics_job.is_some();
        simulation_owner.tick_dragon_authority(&entity_sessions, tick);
        let started = Instant::now();
        let queries = if physics_was_in_flight {
            Vec::new()
        } else {
            simulation_owner.collect_entity_physics_queries(
                &entity_sessions,
                &entity_chunk_pipeline_resources,
                tick,
                play::EntitySimulationTickPolicy {
                    entity_updates_per_lane: entity_update_budget.configured_per_lane(),
                    pathing_candidates_per_entity: work_budgets.entity_pathing_candidates,
                    simulation_distance: simulation_policy.simulation_distance,
                },
                simulation_owner.entity_world_context(
                    entity_world_read.as_ref(),
                    entity_pathing_materials.as_deref(),
                    entity_config.blocks.as_ref(),
                    entity_config.items.as_ref(),
                ),
            )
        };
        let entity_goals_us = elapsed_us(started);
        let started = Instant::now();
        simulation_owner.tick_hostile_attacks(
            &entity_sessions,
            tick,
            play::air_state_id(&entity_config.blocks),
        );
        let hostile_attacks_us = elapsed_us(started);
        if tick.is_multiple_of(u64::from(ANIMAL_BREEDING_TICK_INTERVAL_TICKS)) {
            let started = Instant::now();
            simulation_owner
                .tick_animal_breeding(&entity_sessions, ANIMAL_BREEDING_TICK_INTERVAL_TICKS);
            animal_breeding_us = elapsed_us(started);
        }
        if let Some((food_items, villager_type_id, item_type_id)) = villager_population_ids {
            simulation_owner.tick_villager_population(
                &entity_sessions,
                tick,
                food_items,
                villager_type_id,
                item_type_id,
                1,
            );
        }
        if let Some(iron_golem_type_id) = village_defense_golem_type_id {
            simulation_owner.tick_village_defense(
                &entity_sessions,
                tick,
                iron_golem_type_id,
                entity_world_read.as_ref(),
                entity_pathing_materials.as_deref(),
            );
        }
        let entity_query_count = queries.len();
        let (steps, entity_physics_us, entity_dispatch_us) = if physics_was_in_flight {
            (Vec::new(), 0, 0)
        } else {
            let started = Instant::now();
            let inputs =
                prepare_entity_physics_inputs(&entity_config, entity_world_read.as_ref(), &queries);
            if inputs.len() > ENTITY_PHYSICS_INLINE_LIMIT {
                entity_physics_job = Some(spawn_entity_physics_job(
                    tick,
                    queries,
                    entity_chunk_pipeline_resources.clone(),
                    inputs,
                ));
                (Vec::new(), elapsed_us(started), 0)
            } else {
                let physics_snapshot = inputs.first().map(|input| Arc::clone(&input.snapshot));
                let steps =
                    step_entity_physics_inputs(entity_chunk_pipeline_resources.clone(), inputs)
                        .await;
                let entity_physics_us = elapsed_us(started);
                let world_is_current = physics_snapshot.as_ref().is_none_or(|snapshot| {
                    entity_world_read.as_ref().is_some_and(|world_read| {
                        entity_physics_snapshot_is_current(world_read, snapshot)
                    })
                });
                if !world_is_current {
                    debug!(
                        tick,
                        "discarded inline entity physics after world snapshot changed"
                    );
                    (Vec::new(), entity_physics_us, 0)
                } else {
                    let projectile_physics_facts =
                        inline_projectile_facts(tick, &queries, physics_snapshot, &steps);
                    let started = Instant::now();
                    let accepted_steps = simulation_owner.apply_entity_physics_if_current(
                        &entity_sessions,
                        &entity_chunk_pipeline_resources,
                        tick,
                        &queries,
                        &steps,
                        &projectile_physics_facts,
                    );
                    let entity_dispatch_us = elapsed_us(started);
                    let landed_falling_blocks =
                        entity_sessions.landed_falling_blocks(&accepted_steps);
                    if !landed_falling_blocks.is_empty() {
                        simulation_owner
                            .land_falling_blocks(
                                &entity_config,
                                &entity_sessions,
                                entity_world_read.as_ref(),
                                &landed_falling_blocks,
                            )
                            .await;
                    }
                    (steps, entity_physics_us, entity_dispatch_us)
                }
            }
        };
        let entity_step_count = steps.len();
        if entity_physics_job.is_none() {
            simulation_owner
                .tick_primed_tnt(
                    &entity_sessions,
                    entity_config.world.as_ref(),
                    entity_config.block_light.as_deref(),
                    &entity_config.block_facts,
                    play::ExplosionRegistries::from_config(&entity_config),
                    entity_pathing_materials.as_deref(),
                    || {
                        entity_script_zones.as_ref().map(|zones| {
                            zones.protection_snapshot().unwrap_or_else(|error| {
                                warn!(
                                    ?error,
                                    "zone protection snapshot unavailable; denying explosion block damage"
                                );
                                crate::script::ZoneProtectionSnapshot::unavailable()
                            })
                        })
                    },
                )
                .await;
        }

        let started = Instant::now();
        let campfire_tick = simulation_owner
            .run_campfire_cooking_ticks(
                &entity_config,
                &entity_sessions,
                entity_world_read.as_ref(),
                entity_world_mutation.as_ref(),
            )
            .await;
        let campfire_tick_us = elapsed_us(started);

        let started = Instant::now();
        let furnace_updated = simulation_owner
            .run_furnace_ticks(
                &entity_config,
                &entity_sessions,
                entity_world_read.as_ref(),
                entity_world_mutation.as_ref(),
            )
            .await;
        let furnace_tick_us = elapsed_us(started);

        let loaded_chunks = entity_sessions.loaded_chunks_sorted();
        let spawning_chunks = entity_sessions.spawning_chunks_sorted();
        let started = Instant::now();
        let inhabited_updates = inhabited_time.observe_tick(tick, &spawning_chunks);
        let missing = entity_world_mutation
            .as_ref()
            .map_or(inhabited_updates.clone(), |mutation| {
                mutation.increment_chunk_inhabited_times(&inhabited_updates)
            });
        inhabited_time.restore(missing);
        let inhabited_time_us = elapsed_us(started);
        // `entity_save_us` is the synchronous save work executed inside this
        // tick. It is intentionally zero: the request below is non-blocking,
        // its tiny enqueue cost remains visible in total/unattributed tick time,
        // and actual checkpoint I/O is reported by `SaveAllTimings` from the
        // dedicated save worker.
        let entity_save_us = 0;
        if tick.is_multiple_of(simulation_policy.save_interval_ticks)
            && entity_sessions.active_session_count() > 0
        {
            request_full_checkpoint(periodic_save_requests.as_ref(), tick, "periodic interval");
        }

        let started = Instant::now();
        let ambient_protection = entity_script_zones.as_ref().map(|zones| {
            zones.protection_snapshot().unwrap_or_else(|error| {
                warn!(
                    ?error,
                    "zone protection snapshot unavailable; denying ambient block mutation"
                );
                crate::script::ZoneProtectionSnapshot::unavailable()
            })
        });
        let random_tick = simulation_owner
            .run_random_ticks_with_budget(
                &entity_config,
                &entity_sessions,
                play::SimulationWorldAccess {
                    read: entity_world_read.as_ref(),
                    mutation: entity_world_mutation.as_ref(),
                    cpu: Some(&entity_chunk_pipeline_resources),
                    light: entity_config.block_light.as_ref(),
                },
                ambient_protection.as_ref(),
                tick,
                work_budgets.random_tick_chunks,
            )
            .await;
        let random_tick_us = elapsed_us(started);

        let (block_tick, block_tick_us) =
            if entity_scheduled_ticks
                .as_ref()
                .is_some_and(|scheduled_ticks| {
                    loaded_block_tick_due(scheduled_ticks, &loaded_chunks, tick)
                })
            {
                let started = Instant::now();
                let job = spawn_scheduled_block_tick_job(
                    tick,
                    work_budgets.scheduled_ticks,
                    Arc::clone(&entity_config),
                    Arc::clone(&entity_sessions),
                    entity_world_read.clone(),
                    entity_world_mutation.clone(),
                    ambient_protection.map(Arc::new),
                    entity_chunk_pipeline_resources.clone(),
                );
                let (result, mid_tick_commands) = await_scheduled_block_tick_job_with_commands(
                    job,
                    &mut simulation_owner,
                    &entity_config,
                    &entity_sessions,
                    entity_world_read.as_ref(),
                    entity_world_mutation.as_ref(),
                    &entity_chunk_pipeline_resources,
                )
                .await;
                simulation_commands_us =
                    simulation_commands_us.saturating_add(mid_tick_commands.elapsed_us);
                simulation_commands.processed = simulation_commands
                    .processed
                    .saturating_add(mid_tick_commands.report.processed);
                simulation_commands.remaining_depth = mid_tick_commands.report.remaining_depth;
                simulation_command_cpu_admission_wait_us = simulation_command_cpu_admission_wait_us
                    .saturating_add(
                        mid_tick_commands
                            .report
                            .lane_attribution
                            .iter()
                            .map(|attribution| attribution.cpu_admission_wait_us)
                            .sum::<u64>(),
                    );
                simulation_command_post_admission_us = simulation_command_post_admission_us
                    .saturating_add(
                        mid_tick_commands
                            .report
                            .lane_attribution
                            .iter()
                            .flat_map(|lane| &lane.commands)
                            .map(|attribution| attribution.post_admission_command_us)
                            .sum::<u64>(),
                    );
                simulation_commands
                    .lane_attribution
                    .extend(mid_tick_commands.report.lane_attribution);
                let block_tick_us =
                    elapsed_us(started).saturating_sub(mid_tick_commands.elapsed_us);
                let report = match result {
                    Ok(completed) => {
                        debug!(
                            tick = completed.tick,
                            drained = completed.report.drained,
                            applied = completed.report.applied,
                            elapsed_us = completed.elapsed_us,
                            "scheduled block tick job completed"
                        );
                        completed.report
                    }
                    Err(error) if error.is_cancelled() => {
                        debug!("scheduled block tick job cancelled");
                        play::ScheduledBlockTickReport {
                            budget: work_budgets.scheduled_ticks.max(1),
                            ..play::ScheduledBlockTickReport::default()
                        }
                    }
                    Err(error) => {
                        warn!(%error, "scheduled block tick job failed");
                        play::ScheduledBlockTickReport {
                            budget: work_budgets.scheduled_ticks.max(1),
                            ..play::ScheduledBlockTickReport::default()
                        }
                    }
                };
                (report, block_tick_us)
            } else {
                (
                    play::ScheduledBlockTickReport {
                        budget: work_budgets.scheduled_ticks.max(1),
                        ..play::ScheduledBlockTickReport::default()
                    },
                    0,
                )
            };

        let started = Instant::now();
        let fluid_tick = if entity_scheduled_ticks
            .as_ref()
            .is_some_and(|scheduled_ticks| {
                loaded_fluid_tick_due(scheduled_ticks, &loaded_chunks, tick)
            }) {
            simulation_owner
                .run_scheduled_fluid_ticks_with_budget(
                    &entity_config,
                    &entity_sessions,
                    entity_world_read.as_ref(),
                    entity_world_mutation.as_ref(),
                    tick,
                    work_budgets.scheduled_ticks,
                )
                .await
        } else {
            play::ScheduledFluidTickReport {
                budget: work_budgets.scheduled_ticks.max(1),
                ..play::ScheduledFluidTickReport::default()
            }
        };
        let fluid_tick_us = elapsed_us(started);

        let tick_us = elapsed_us(tick_started)
            .saturating_add(simulation_command_telemetry.off_tick_elapsed_us);
        let (_, _, selected_entity_updates, active_entity_population) =
            entity_sessions.entity_update_budget_observation();
        let target_tick_us = entity_runtime_control
            .as_ref()
            .map(|control| {
                control
                    .snapshot()
                    .policy
                    .target_tick_ms
                    .saturating_mul(1_000)
            })
            .unwrap_or(50_000);
        let outbound_pressure = entity_sessions.pressure_snapshot();
        let reliable_drops_increased =
            outbound_pressure.reliable_command_drops > entity_budget_last_reliable_drops;
        entity_budget_last_reliable_drops = outbound_pressure.reliable_command_drops;
        let entity_pressure = crate::runtime_entity_budget::EntityUpdatePressure {
            reliable_drops_increased,
            reliable_retries_in_flight: outbound_pressure.reliable_command_retries_in_flight,
            simulation_queue_depth: simulation_commands.remaining_depth,
        };
        let entity_update_budget_snapshot = entity_update_budget.observe(
            crate::runtime_entity_budget::EntityUpdateBudgetObservation {
                tick_us,
                entity_goals_us,
                selected: selected_entity_updates,
                active_population: active_entity_population,
                lane_count: entity_chunk_pipeline_resources.cpu_limit().max(1),
                target_tick_us,
                pressure: entity_pressure,
            },
        );
        let movement_budget =
            movement_publication_budget.observe(tick_us, target_tick_us, entity_pressure);
        entity_sessions.set_entity_movement_publication_budget(movement_budget);
        let current_tick_sample = RuntimeTickSample {
            tick_us,
            world_time_us,
            sheep_grazing_us,
            animal_breeding_us,
            hostile_attacks_us,
            entity_goals_us,
            entity_physics_us,
            entity_dispatch_us,
            campfire_tick_us,
            inhabited_time_us,
            entity_save_us,
            random_tick_us,
            block_tick_us,
            fluid_tick_us,
        };
        let attributed_tick_us = runtime_attributed_tick_us(
            &current_tick_sample,
            simulation_commands_us,
            furnace_tick_us,
        );
        let unattributed_tick_us = tick_us.saturating_sub(attributed_tick_us);
        tick_metrics.record(current_tick_sample);
        scheduled_budget_exhausted_since_publish |=
            block_tick.budget_exhausted || fluid_tick.budget_exhausted;
        if let Some(control) = entity_runtime_control.as_ref() {
            if let Some(sampler) = memory_pressure_sampler.as_ref() {
                sampler.request();
            }
            observe_runtime_control_tick(
                control,
                &entity_chunk_pipeline_resources,
                &entity_sessions,
                &entity_config.shutdown,
                tick_us,
            );
        }
        if tick.is_multiple_of(metrics_policy.log_interval_ticks) {
            if tick_metrics_publisher.try_publish(
                tick,
                &tick_metrics,
                scheduled_budget_exhausted_since_publish,
            ) {
                scheduled_budget_exhausted_since_publish = false;
            }
            if let Some(percentiles) = entity_tick_metrics.snapshot()
                && tracing::enabled!(tracing::Level::DEBUG)
            {
                debug!(
                    tick,
                    world_time,
                    tick_window_source_tick = percentiles.source_tick,
                    tick_window_submit_us = percentiles.observer_submit_us,
                    tick_window_compute_us = percentiles.observer_compute_us,
                    tick_window_skipped = percentiles.observer_skipped_windows,
                    tick_window_samples = percentiles.tick.samples,
                    tick_window_capacity = tick_metrics.capacity(),
                    tick_p50_us = percentiles.tick.p50_us,
                    tick_p95_us = percentiles.tick.p95_us,
                    tick_p99_us = percentiles.tick.p99_us,
                    tick_max_us = percentiles.tick.max_us,
                    world_time_p50_us = percentiles.world_time.p50_us,
                    world_time_p95_us = percentiles.world_time.p95_us,
                    world_time_p99_us = percentiles.world_time.p99_us,
                    world_time_max_us = percentiles.world_time.max_us,
                    sheep_grazing_p50_us = percentiles.sheep_grazing.p50_us,
                    sheep_grazing_p95_us = percentiles.sheep_grazing.p95_us,
                    sheep_grazing_p99_us = percentiles.sheep_grazing.p99_us,
                    sheep_grazing_max_us = percentiles.sheep_grazing.max_us,
                    animal_breeding_p50_us = percentiles.animal_breeding.p50_us,
                    animal_breeding_p95_us = percentiles.animal_breeding.p95_us,
                    animal_breeding_p99_us = percentiles.animal_breeding.p99_us,
                    animal_breeding_max_us = percentiles.animal_breeding.max_us,
                    hostile_attacks_p50_us = percentiles.hostile_attacks.p50_us,
                    hostile_attacks_p95_us = percentiles.hostile_attacks.p95_us,
                    hostile_attacks_p99_us = percentiles.hostile_attacks.p99_us,
                    hostile_attacks_max_us = percentiles.hostile_attacks.max_us,
                    entity_goals_p50_us = percentiles.entity_goals.p50_us,
                    entity_goals_p95_us = percentiles.entity_goals.p95_us,
                    entity_goals_p99_us = percentiles.entity_goals.p99_us,
                    entity_goals_max_us = percentiles.entity_goals.max_us,
                    entity_physics_p50_us = percentiles.entity_physics.p50_us,
                    entity_physics_p95_us = percentiles.entity_physics.p95_us,
                    entity_physics_p99_us = percentiles.entity_physics.p99_us,
                    entity_physics_max_us = percentiles.entity_physics.max_us,
                    entity_dispatch_p50_us = percentiles.entity_dispatch.p50_us,
                    entity_dispatch_p95_us = percentiles.entity_dispatch.p95_us,
                    entity_dispatch_p99_us = percentiles.entity_dispatch.p99_us,
                    entity_dispatch_max_us = percentiles.entity_dispatch.max_us,
                    campfire_tick_p50_us = percentiles.campfire_tick.p50_us,
                    campfire_tick_p95_us = percentiles.campfire_tick.p95_us,
                    campfire_tick_p99_us = percentiles.campfire_tick.p99_us,
                    campfire_tick_max_us = percentiles.campfire_tick.max_us,
                    inhabited_time_p50_us = percentiles.inhabited_time.p50_us,
                    inhabited_time_p95_us = percentiles.inhabited_time.p95_us,
                    inhabited_time_p99_us = percentiles.inhabited_time.p99_us,
                    inhabited_time_max_us = percentiles.inhabited_time.max_us,
                    entity_save_p50_us = percentiles.entity_save.p50_us,
                    entity_save_p95_us = percentiles.entity_save.p95_us,
                    entity_save_p99_us = percentiles.entity_save.p99_us,
                    entity_save_max_us = percentiles.entity_save.max_us,
                    random_tick_p50_us = percentiles.random_tick.p50_us,
                    random_tick_p95_us = percentiles.random_tick.p95_us,
                    random_tick_p99_us = percentiles.random_tick.p99_us,
                    random_tick_max_us = percentiles.random_tick.max_us,
                    block_tick_p50_us = percentiles.block_tick.p50_us,
                    block_tick_p95_us = percentiles.block_tick.p95_us,
                    block_tick_p99_us = percentiles.block_tick.p99_us,
                    block_tick_max_us = percentiles.block_tick.max_us,
                    fluid_tick_p50_us = percentiles.fluid_tick.p50_us,
                    fluid_tick_p95_us = percentiles.fluid_tick.p95_us,
                    fluid_tick_p99_us = percentiles.fluid_tick.p99_us,
                    fluid_tick_max_us = percentiles.fluid_tick.max_us,
                    "runtime tick percentile window"
                );
            }
        }
        if metrics_log_gate.should_log(tick, tick_us, metrics_policy) {
            let pressure = entity_sessions.pressure_snapshot();
            let lock_pressure = crate::lock_metrics::snapshot();
            if is_slow_tick(tick_us, metrics_policy) {
                warn!(
                    tick,
                    world_time,
                    tick_us,
                    world_time_us,
                    sheep_grazing_us,
                    animal_breeding_us,
                    hostile_attacks_us,
                    entity_goals_us,
                    entity_physics_us,
                    entity_dispatch_us,
                    campfire_tick_us,
                    furnace_tick_us,
                    furnace_updated,
                    unattributed_tick_us,
                    inhabited_time_us,
                    entity_save_us,
                    random_tick_us,
                    block_tick_us,
                    fluid_tick_us,
                    simulation_commands_us,
                    simulation_commands_processed = simulation_commands.processed,
                    simulation_commands_remaining = simulation_commands.remaining_depth,
                    simulation_command_scope,
                    simulation_command_cpu_admission_wait_us,
                    simulation_command_post_admission_us,
                    entity_queries = entity_query_count,
                    entity_steps = entity_step_count,
                    entity_update_budget_per_lane =
                        entity_update_budget_snapshot.configured_per_lane,
                    entity_update_budget_total = entity_update_budget_snapshot.effective_total,
                    entity_update_selected = entity_update_budget_snapshot.selected,
                    entity_update_active_population =
                        entity_update_budget_snapshot.active_population,
                    entity_update_rotation_ticks =
                        entity_update_budget_snapshot.estimated_rotation_ticks,
                    entity_physics_in_flight = entity_physics_job.is_some(),
                    campfire_persisted = campfire_tick.persisted,
                    campfire_completed = campfire_tick.completed,
                    campfire_dropped = campfire_tick.dropped,
                    random_sampled = random_tick.sampled,
                    random_eligible = random_tick.eligible,
                    random_applied = random_tick.applied,
                    block_drained = block_tick.drained,
                    block_applied = block_tick.applied,
                    block_budget = block_tick.budget,
                    block_budget_exhausted = block_tick.budget_exhausted,
                    fluid_drained = fluid_tick.drained,
                    fluid_applied = fluid_tick.applied,
                    fluid_budget = fluid_tick.budget,
                    fluid_budget_exhausted = fluid_tick.budget_exhausted,
                    sessions = pressure.sessions,
                    ticketed_chunks = pressure.ticketed_chunks,
                    prepared_chunks = pressure.prepared_chunks,
                    server_entities = pressure.server_entities,
                    entity_spawn_dispatches = pressure.entity_dispatches.spawn,
                    entity_move_dispatches = pressure.entity_dispatches.move_relative,
                    entity_data_dispatches = pressure.entity_dispatches.data,
                    entity_take_dispatches = pressure.entity_dispatches.take,
                    entity_remove_dispatches = pressure.entity_dispatches.remove,
                    best_effort_animation_drops = pressure.best_effort_animation_drops,
                    reliable_command_drops = pressure.reliable_command_drops,
                    reliable_command_retries = pressure.reliable_command_retries,
                    reliable_command_retries_in_flight =
                        pressure.reliable_command_retries_in_flight,
                    furnace_viewer_sets = pressure.furnace_viewer_sets,
                    chest_viewer_sets = pressure.chest_viewer_sets,
                    world_lock_waits = lock_pressure.world_storage.wait_count,
                    world_lock_wait_us = lock_pressure.world_storage.wait_us,
                    world_lock_max_wait_us = lock_pressure.world_storage.max_wait_us,
                    world_lock_hold_us = lock_pressure.world_storage.hold_us,
                    world_lock_max_hold_us = lock_pressure.world_storage.max_hold_us,
                    session_lock_waits = lock_pressure.session_registry.wait_count,
                    session_lock_wait_us = lock_pressure.session_registry.wait_us,
                    session_lock_max_wait_us = lock_pressure.session_registry.max_wait_us,
                    session_lock_hold_us = lock_pressure.session_registry.hold_us,
                    session_lock_max_hold_us = lock_pressure.session_registry.max_hold_us,
                    container_lock_wait_us = lock_pressure.container_registry.wait_us,
                    container_lock_max_wait_us = lock_pressure.container_registry.max_wait_us,
                    container_lock_hold_us = lock_pressure.container_registry.hold_us,
                    container_lock_max_hold_us = lock_pressure.container_registry.max_hold_us,
                    save_flush_lock_wait_us = lock_pressure.save_all_flush.wait_us,
                    save_flush_lock_hold_us = lock_pressure.save_all_flush.hold_us,
                    chunk_prepare_lock_wait_us = lock_pressure.chunk_prepare.wait_us,
                    chunk_prepare_lock_hold_us = lock_pressure.chunk_prepare.hold_us,
                    player_persistence_lock_wait_us = lock_pressure.player_persistence.wait_us,
                    player_persistence_lock_hold_us = lock_pressure.player_persistence.hold_us,
                    "runtime tick exceeded performance budget"
                );
                let attributed_lane_waits = simulation_commands
                    .lane_attribution
                    .iter()
                    .take(SLOW_SIMULATION_ATTRIBUTION_LIMIT)
                    .map(|lane| lane.cpu_admission_wait_us)
                    .collect::<Vec<_>>();
                let attributed_commands = simulation_commands
                    .lane_attribution
                    .iter()
                    .flat_map(|lane| &lane.commands)
                    .take(SLOW_SIMULATION_ATTRIBUTION_LIMIT)
                    .collect::<Vec<_>>();
                let attributed_command_count = simulation_commands
                    .lane_attribution
                    .iter()
                    .map(|lane| lane.commands.len())
                    .sum::<usize>();
                let omitted_lanes = simulation_commands
                    .lane_attribution
                    .len()
                    .saturating_sub(attributed_lane_waits.len());
                let omitted_commands =
                    attributed_command_count.saturating_sub(attributed_commands.len());
                if !attributed_lane_waits.is_empty() || !attributed_commands.is_empty() {
                    warn!(
                        tick,
                        simulation_command_scope,
                        cpu_admission_wait_us_by_lane = ?attributed_lane_waits,
                        simulation_commands = ?attributed_commands,
                        omitted_lanes,
                        omitted_commands,
                        "slow simulation command attribution"
                    );
                }
            } else {
                debug!(
                    tick,
                    world_time,
                    tick_us,
                    world_time_us,
                    sheep_grazing_us,
                    animal_breeding_us,
                    hostile_attacks_us,
                    entity_goals_us,
                    entity_physics_us,
                    entity_dispatch_us,
                    campfire_tick_us,
                    furnace_tick_us,
                    furnace_updated,
                    unattributed_tick_us,
                    inhabited_time_us,
                    entity_save_us,
                    random_tick_us,
                    block_tick_us,
                    fluid_tick_us,
                    simulation_commands_us,
                    simulation_commands_processed = simulation_commands.processed,
                    simulation_commands_remaining = simulation_commands.remaining_depth,
                    simulation_command_scope,
                    simulation_command_cpu_admission_wait_us,
                    simulation_command_post_admission_us,
                    entity_queries = entity_query_count,
                    entity_steps = entity_step_count,
                    entity_update_budget_per_lane =
                        entity_update_budget_snapshot.configured_per_lane,
                    entity_update_budget_total = entity_update_budget_snapshot.effective_total,
                    entity_update_selected = entity_update_budget_snapshot.selected,
                    entity_update_active_population =
                        entity_update_budget_snapshot.active_population,
                    entity_update_rotation_ticks =
                        entity_update_budget_snapshot.estimated_rotation_ticks,
                    entity_physics_in_flight = entity_physics_job.is_some(),
                    campfire_persisted = campfire_tick.persisted,
                    campfire_completed = campfire_tick.completed,
                    campfire_dropped = campfire_tick.dropped,
                    random_sampled = random_tick.sampled,
                    random_eligible = random_tick.eligible,
                    random_applied = random_tick.applied,
                    block_drained = block_tick.drained,
                    block_applied = block_tick.applied,
                    block_budget = block_tick.budget,
                    block_budget_exhausted = block_tick.budget_exhausted,
                    fluid_drained = fluid_tick.drained,
                    fluid_applied = fluid_tick.applied,
                    fluid_budget = fluid_tick.budget,
                    fluid_budget_exhausted = fluid_tick.budget_exhausted,
                    sessions = pressure.sessions,
                    ticketed_chunks = pressure.ticketed_chunks,
                    prepared_chunks = pressure.prepared_chunks,
                    server_entities = pressure.server_entities,
                    entity_spawn_dispatches = pressure.entity_dispatches.spawn,
                    entity_move_dispatches = pressure.entity_dispatches.move_relative,
                    entity_data_dispatches = pressure.entity_dispatches.data,
                    entity_take_dispatches = pressure.entity_dispatches.take,
                    entity_remove_dispatches = pressure.entity_dispatches.remove,
                    best_effort_animation_drops = pressure.best_effort_animation_drops,
                    reliable_command_drops = pressure.reliable_command_drops,
                    reliable_command_retries = pressure.reliable_command_retries,
                    reliable_command_retries_in_flight =
                        pressure.reliable_command_retries_in_flight,
                    furnace_viewer_sets = pressure.furnace_viewer_sets,
                    chest_viewer_sets = pressure.chest_viewer_sets,
                    world_lock_waits = lock_pressure.world_storage.wait_count,
                    world_lock_wait_us = lock_pressure.world_storage.wait_us,
                    world_lock_max_wait_us = lock_pressure.world_storage.max_wait_us,
                    world_lock_hold_us = lock_pressure.world_storage.hold_us,
                    world_lock_max_hold_us = lock_pressure.world_storage.max_hold_us,
                    session_lock_waits = lock_pressure.session_registry.wait_count,
                    session_lock_wait_us = lock_pressure.session_registry.wait_us,
                    session_lock_max_wait_us = lock_pressure.session_registry.max_wait_us,
                    session_lock_hold_us = lock_pressure.session_registry.hold_us,
                    session_lock_max_hold_us = lock_pressure.session_registry.max_hold_us,
                    container_lock_wait_us = lock_pressure.container_registry.wait_us,
                    container_lock_max_wait_us = lock_pressure.container_registry.max_wait_us,
                    container_lock_hold_us = lock_pressure.container_registry.hold_us,
                    container_lock_max_hold_us = lock_pressure.container_registry.max_hold_us,
                    save_flush_lock_wait_us = lock_pressure.save_all_flush.wait_us,
                    save_flush_lock_hold_us = lock_pressure.save_all_flush.hold_us,
                    chunk_prepare_lock_wait_us = lock_pressure.chunk_prepare.wait_us,
                    chunk_prepare_lock_hold_us = lock_pressure.chunk_prepare.hold_us,
                    player_persistence_lock_wait_us = lock_pressure.player_persistence.wait_us,
                    player_persistence_lock_hold_us = lock_pressure.player_persistence.hold_us,
                    "runtime tick metrics"
                );
            }
        }
    }
    drop(memory_pressure_sampler);
    if let Some(worker) = memory_pressure_worker
        && let Err(error) = worker.await
    {
        warn!(%error, "memory pressure sampler worker failed");
    }
    drop(tick_metrics_publisher);
    drop(tick_metrics_observations);
    if let Err(error) = tick_metrics_worker.await {
        warn!(%error, "runtime tick metrics worker failed");
    }
}
