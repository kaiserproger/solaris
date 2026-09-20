use super::*;

pub(super) fn request_full_checkpoint(
    requests: Option<&crate::dirty_flush::DirtyFlushNotifier>,
    tick: u64,
    trigger: &'static str,
) {
    let Some(requests) = requests else {
        return;
    };
    debug!(tick, trigger, "full checkpoint requested");
    requests.request_full_checkpoint();
}

pub(super) async fn persist_inhabited_time_tail(
    config: &ServerConfig,
    mutation: Option<&mc_world::WorldMutationView>,
    accumulator: &mut play::InhabitedTimeAccumulator,
) {
    let updates = accumulator.drain();
    let missing = mutation.map_or_else(
        || updates.clone(),
        |mutation| mutation.increment_chunk_inhabited_times(&updates),
    );
    if missing.is_empty() {
        return;
    }
    let Some(world) = config.world.as_ref() else {
        warn!(
            chunks = missing.len(),
            "cannot persist inhabited time without world storage"
        );
        return;
    };
    let mut storage = crate::lock_metrics::timed_guard(
        crate::lock_metrics::LockMetricKind::WorldStorage,
        "persist inhabited time tail",
        Instant::now(),
        world.lock().await,
    );
    let mut loaded = Vec::with_capacity(missing.len());
    for update @ (position, _) in missing {
        match storage.get_chunk_without_generation(position) {
            Ok(Some(_)) => loaded.push(update),
            Ok(None) => warn!(?position, "inhabited-time chunk vanished before shutdown"),
            Err(error) => warn!(?position, %error, "failed to load inhabited-time chunk"),
        }
    }
    let still_missing = storage
        .mutation_view()
        .increment_chunk_inhabited_times(&loaded);
    if !still_missing.is_empty() {
        warn!(
            chunks = still_missing.len(),
            "inhabited-time chunks vanished during shutdown publication"
        );
    }
}

pub(super) async fn wait_for_session_empty_save_request(
    sessions: &play::SessionRegistry,
    observed: u64,
    requests: Option<&crate::dirty_flush::DirtyFlushNotifier>,
    tick: u64,
) -> u64 {
    sessions.wait_for_session_empty(observed).await;
    request_full_checkpoint(requests, tick, "last session unregistered");
    sessions.session_empty_generation()
}

pub(super) async fn wait_for_player_save_request(
    sessions: &play::SessionRegistry,
    observed: u64,
    requests: Option<&crate::dirty_flush::DirtyFlushNotifier>,
    tick: u64,
) -> u64 {
    sessions.wait_for_player_save_request(observed).await;
    request_full_checkpoint(requests, tick, "player disconnected");
    sessions.player_save_generation()
}

pub(crate) async fn save_periodic_checkpoint(
    config: &ServerConfig,
    sessions: &play::SessionRegistry,
    simulation: &play::SimulationHandle,
    shutdown: &ShutdownHandle,
) -> Option<SaveAllReport> {
    let total_started = Instant::now();
    let queue_started = Instant::now();
    let coordinator = config.shutdown.save_coordinator();
    let _save_guard = tokio::select! {
        biased;
        () = shutdown.notified() => return None,
        guard = coordinator.lock() => guard,
    };
    let coordinator_us = elapsed_us(queue_started);
    let barrier_started = Instant::now();
    let barrier = tokio::select! {
        biased;
        () = shutdown.notified() => return None,
        result = simulation.save_barrier(config.world.is_some()) => result,
    };
    let snapshot = match barrier {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let elapsed = elapsed_us(total_started);
            let report = SaveAllReport {
                players_saved: 0,
                entities_saved: 0,
                chunks_flushed: 0,
                world_metadata_saved: false,
                timings: SaveAllTimings {
                    queued_us: coordinator_us.saturating_add(elapsed_us(barrier_started)),
                    total_us: elapsed,
                    ..SaveAllTimings::default()
                },
                errors: vec![format!("simulation barrier failed: {error:?}")],
            };
            sessions.retain_save_report(&report);
            return Some(report);
        }
    };
    let barrier_us = elapsed_us(barrier_started);
    Some(
        crate::resource_profile::measure_future(
            crate::resource_profile::CpuStage::Saving,
            save_all_with_context_snapshot_locked(
                "periodic checkpoint",
                config,
                sessions,
                Some(snapshot),
                false,
                coordinator_us.saturating_add(barrier_us),
                total_started,
            ),
        )
        .await,
    )
}

#[derive(Debug)]
pub(super) struct DirtyOnlyFlushReport {
    pub(super) planned_chunks: usize,
    pub(super) flushed_chunks: usize,
    pub(super) remaining_dirty: usize,
    pub(super) immediately_flushable: bool,
}

/// Pressure-only fast path for the measured full-checkpoint storm: persist one
/// bounded chunk batch without taking a simulation save barrier or touching
/// players, entities, metadata, or WAL checkpoints. Generation checks in
/// `commit_dirty_flush` fence newer edits; periodic, disconnect, shutdown, and
/// explicit full checkpoints remain the fallback durability path.
pub(super) async fn flush_dirty_chunks_only(
    config: &ServerConfig,
    simulation_tick: u64,
) -> Result<DirtyOnlyFlushReport, String> {
    let Some(world) = config.world.as_ref() else {
        return Ok(DirtyOnlyFlushReport {
            planned_chunks: 0,
            flushed_chunks: 0,
            remaining_dirty: 0,
            immediately_flushable: false,
        });
    };
    let coordinator = config.shutdown.save_coordinator();
    let _save_guard = coordinator.lock().await;
    let mut stale_retries = 0usize;
    loop {
        let (plan, dirty_before) = {
            let mut storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::SaveAllFlush,
                "dirty-only flush plan",
                Instant::now(),
                world.lock().await,
            );
            if storage.world_root().is_none() {
                return Ok(DirtyOnlyFlushReport {
                    planned_chunks: 0,
                    flushed_chunks: 0,
                    remaining_dirty: storage.dirty_count(),
                    immediately_flushable: false,
                });
            }
            let dirty_before = storage.dirty_count();
            let plan = storage
                .plan_dirty_flush_at_tick_bounded(simulation_tick, DIRTY_ONLY_FLUSH_MAX_CHUNKS)
                .map_err(|error| format!("dirty-only flush plan failed: {error}"))?;
            (plan, dirty_before)
        };
        let planned_chunks = plan.chunk_count();
        if plan.is_empty() {
            return Ok(DirtyOnlyFlushReport {
                planned_chunks: 0,
                flushed_chunks: 0,
                remaining_dirty: dirty_before,
                immediately_flushable: false,
            });
        }
        let commit = match crate::dirty_flush::write_dirty_flush_blocking_typed(plan).await {
            Ok(commit) => commit,
            Err(error)
                if error.is_stale_region()
                    && stale_retries < DIRTY_ONLY_FLUSH_STALE_REGION_RETRIES =>
            {
                stale_retries += 1;
                continue;
            }
            Err(error) => return Err(format!("dirty-only flush write failed: {error}")),
        };
        let install = {
            let mut storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::SaveAllFlush,
                "dirty-only flush install",
                Instant::now(),
                world.lock().await,
            );
            match storage.install_dirty_flush(commit) {
                Ok(install) => install,
                Err(mc_world::WorldError::StaleRegion(_))
                    if stale_retries < DIRTY_ONLY_FLUSH_STALE_REGION_RETRIES =>
                {
                    stale_retries += 1;
                    continue;
                }
                Err(error) => return Err(format!("dirty-only flush install failed: {error}")),
            }
        };
        let synced = crate::dirty_flush::sync_dirty_flush_install_blocking_typed(install)
            .await
            .map_err(|error| format!("dirty-only flush sync failed: {error}"))?;
        let mut storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::SaveAllFlush,
            "dirty-only flush finalize",
            Instant::now(),
            world.lock().await,
        );
        let flushed_chunks = storage.finalize_dirty_flush(synced).cleaned_chunks();
        return Ok(DirtyOnlyFlushReport {
            planned_chunks,
            flushed_chunks,
            remaining_dirty: storage.dirty_count(),
            immediately_flushable: storage.has_flushable_dirty_chunks(),
        });
    }
}

pub(super) fn log_dirty_only_flush(
    context: &'static str,
    result: Result<DirtyOnlyFlushReport, String>,
) -> crate::dirty_flush::DirtyFlushCompletion {
    match result {
        Ok(report) => {
            debug!(
                planned = report.planned_chunks,
                flushed = report.flushed_chunks,
                remaining_dirty = report.remaining_dirty,
                immediately_flushable = report.immediately_flushable,
                %context,
                "bounded dirty-only flush completed"
            );
            return if report.immediately_flushable {
                crate::dirty_flush::DirtyFlushCompletion::MoreDirty
            } else if report.remaining_dirty == 0 {
                crate::dirty_flush::DirtyFlushCompletion::Complete
            } else {
                crate::dirty_flush::DirtyFlushCompletion::AwaitingProducer
            };
        }
        Err(error) => {
            warn!(%error, %context, "bounded dirty-only flush failed");
        }
    }
    crate::dirty_flush::DirtyFlushCompletion::Failed
}

pub(super) async fn enqueue_startup_checkpoint(
    config: &ServerConfig,
    requests: &crate::dirty_flush::DirtyFlushNotifier,
) {
    let Some(dirty_chunks) = startup_dirty_flush_dirty_count(config).await else {
        return;
    };
    info!(dirty = dirty_chunks, "startup checkpoint scheduled");
    requests.request_full_checkpoint();
}

pub(super) async fn startup_dirty_flush_dirty_count(config: &ServerConfig) -> Option<usize> {
    if config.shutdown.is_requested() {
        return None;
    }
    startup_dirty_flush_remaining_dirty_count(config).await
}

pub(super) async fn startup_dirty_flush_remaining_dirty_count(
    config: &ServerConfig,
) -> Option<usize> {
    let world = config.world.as_ref()?;
    let storage = crate::lock_metrics::timed_guard(
        crate::lock_metrics::LockMetricKind::WorldStorage,
        "startup checkpoint dirty count",
        Instant::now(),
        world.lock().await,
    );
    storage.world_root()?;
    let dirty_chunks = storage.stats().dirty_chunks;
    (dirty_chunks > 0).then_some(dirty_chunks)
}

#[cfg(test)]
pub(super) async fn save_all(
    config: &ServerConfig,
    sessions: &play::SessionRegistry,
) -> SaveAllReport {
    save_all_with_context("save-all", config, sessions).await
}

pub(super) async fn save_all_after_drain_with_context(
    context: &'static str,
    config: &ServerConfig,
    sessions: &play::SessionRegistry,
) -> SaveAllReport {
    save_all_with_context_snapshot(context, config, sessions, None, true).await
}

pub(crate) async fn save_all_after_simulation_barrier(
    context: &'static str,
    config: &ServerConfig,
    sessions: &play::SessionRegistry,
    simulation: &play::SimulationHandle,
) -> SaveAllReport {
    let total_started = Instant::now();
    let queue_started = Instant::now();
    let coordinator = config.shutdown.save_coordinator();
    let _save_guard = coordinator.lock().await;
    let coordinator_us = elapsed_us(queue_started);
    let barrier_started = Instant::now();
    let mut journal_failure = sessions.subscribe_world_chunk_journal_failure();
    let snapshot = loop {
        let dirty_tail_generation = config.shutdown.dirty_tail_generation();
        let snapshot = match simulation.save_barrier(config.world.is_some()).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return save_barrier_error_report(
                    total_started,
                    coordinator_us.saturating_add(elapsed_us(barrier_started)),
                    format!("simulation barrier failed: {error:?}"),
                );
            }
        };
        let captures_all_dirty_chunks = snapshot
            .world_flush_plan
            .as_ref()
            .is_none_or(mc_world::DirtyFlushPlan::captures_all_dirty_chunks);
        if captures_all_dirty_chunks {
            break snapshot;
        }
        if *journal_failure.borrow() {
            return save_barrier_error_report(
                total_started,
                coordinator_us.saturating_add(elapsed_us(barrier_started)),
                "world chunk journal failed while completing save barrier".to_string(),
            );
        }

        tokio::select! {
            () = config
                .shutdown
                .wait_for_dirty_tail_progress(dirty_tail_generation) => {}
            changed = journal_failure.changed() => {
                if changed.is_err() || *journal_failure.borrow() {
                    return save_barrier_error_report(
                        total_started,
                        coordinator_us.saturating_add(elapsed_us(barrier_started)),
                        "world chunk journal failed while completing save barrier".to_string(),
                    );
                }
            }
        }
    };
    let barrier_us = elapsed_us(barrier_started);
    crate::resource_profile::measure_future(
        crate::resource_profile::CpuStage::Saving,
        save_all_with_context_snapshot_locked(
            context,
            config,
            sessions,
            Some(snapshot),
            false,
            coordinator_us.saturating_add(barrier_us),
            total_started,
        ),
    )
    .await
}

pub(super) fn save_barrier_error_report(
    total_started: Instant,
    queued_us: u64,
    error: String,
) -> SaveAllReport {
    SaveAllReport {
        players_saved: 0,
        entities_saved: 0,
        chunks_flushed: 0,
        world_metadata_saved: false,
        timings: SaveAllTimings {
            queued_us,
            total_us: elapsed_us(total_started),
            ..SaveAllTimings::default()
        },
        errors: vec![error],
    }
}

#[cfg(test)]
pub(super) async fn save_all_with_context(
    context: &'static str,
    config: &ServerConfig,
    sessions: &play::SessionRegistry,
) -> SaveAllReport {
    save_all_with_context_snapshot(context, config, sessions, None, false).await
}

pub(super) async fn save_all_with_context_snapshot(
    context: &'static str,
    config: &ServerConfig,
    sessions: &play::SessionRegistry,
    snapshot: Option<play::SimulationSaveSnapshot>,
    require_clean_dirty_flush: bool,
) -> SaveAllReport {
    let total_started = Instant::now();
    let queue_started = Instant::now();
    let coordinator = config.shutdown.save_coordinator();
    let _save_guard = coordinator.lock().await;
    crate::resource_profile::measure_future(
        crate::resource_profile::CpuStage::Saving,
        save_all_with_context_snapshot_locked(
            context,
            config,
            sessions,
            snapshot,
            require_clean_dirty_flush,
            elapsed_us(queue_started),
            total_started,
        ),
    )
    .await
}

pub(super) async fn save_all_with_context_snapshot_locked(
    context: &'static str,
    config: &ServerConfig,
    sessions: &play::SessionRegistry,
    snapshot: Option<play::SimulationSaveSnapshot>,
    require_clean_dirty_flush: bool,
    queued_us: u64,
    total_started: Instant,
) -> SaveAllReport {
    let report = save_all_with_context_snapshot_locked_impl(
        context,
        config,
        sessions,
        snapshot,
        require_clean_dirty_flush,
        queued_us,
        total_started,
    )
    .await;
    sessions.retain_save_report(&report);
    report
}

pub(super) async fn save_all_with_context_snapshot_locked_impl(
    context: &'static str,
    config: &ServerConfig,
    sessions: &play::SessionRegistry,
    snapshot: Option<play::SimulationSaveSnapshot>,
    require_clean_dirty_flush: bool,
    queued_us: u64,
    total_started: Instant,
) -> SaveAllReport {
    let barrier_snapshot = snapshot.is_some();
    let mut report = SaveAllReport {
        players_saved: 0,
        entities_saved: 0,
        chunks_flushed: 0,
        world_metadata_saved: false,
        timings: SaveAllTimings {
            queued_us,
            ..SaveAllTimings::default()
        },
        errors: Vec::new(),
    };
    let Some(world) = config.world.as_ref() else {
        report.timings.total_us = elapsed_us(total_started);
        return report;
    };
    let root = {
        let storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::WorldStorage,
            "save-all world root",
            Instant::now(),
            world.lock().await,
        );
        storage.world_root().map(std::path::Path::to_path_buf)
    };
    let Some(root) = root else {
        report.timings.total_us = elapsed_us(total_started);
        return report;
    };
    let (
        simulation_tick,
        players,
        entities,
        entity_journal_phases,
        world_chunk_journal_watermark,
        world_time,
        daylight_cycle_enabled,
        weather,
        players_sleeping_percentage,
        keep_inventory,
        mut world_flush_plan,
    ) = match snapshot {
        Some(snapshot) => (
            snapshot.simulation_tick,
            snapshot.players,
            snapshot.entities,
            snapshot.entity_journal_phases,
            snapshot.world_chunk_journal_watermark,
            snapshot.world_time,
            snapshot.daylight_cycle_enabled,
            snapshot.weather,
            snapshot.players_sleeping_percentage,
            snapshot.keep_inventory,
            snapshot.world_flush_plan,
        ),
        None => {
            let (entities, entity_journal_phases) = sessions.persisted_entity_save_snapshot();
            (
                sessions.simulation_tick(),
                sessions.persisted_player_states(),
                entities,
                entity_journal_phases,
                sessions.world_chunk_journal_watermark(),
                sessions.world_time(),
                sessions.daylight_cycle_enabled(),
                sessions.weather(),
                sessions.players_sleeping_percentage(),
                sessions.keep_inventory(),
                None,
            )
        }
    };

    if let Some(journal) = sessions.world_chunk_journal() {
        match tokio::task::spawn_blocking(move || journal.writer.flush()).await {
            Ok(Ok(())) => {}
            result => {
                sessions.report_world_chunk_journal_failure();
                report
                    .errors
                    .push(format!("journal durability barrier failed: {result:?}"));
                report.timings.total_us = elapsed_us(total_started);
                return report;
            }
        }
    }

    let mut world_flush_clean = false;
    let mut attempt = 0usize;
    loop {
        attempt = attempt.saturating_add(1);
        let started = Instant::now();
        let mut storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::SaveAllFlush,
            "save-all dirty flush plan",
            Instant::now(),
            world.lock().await,
        );
        let storage_before = storage.stats();
        let flush_plan = if let Some(plan) = world_flush_plan.take() {
            plan
        } else {
            match storage.plan_dirty_flush_at_tick(simulation_tick) {
                Ok(plan) => plan,
                Err(err) => {
                    report
                        .errors
                        .push(format!("dirty chunks: flush plan failed: {err}"));
                    break;
                }
            }
        };
        let flushable_before = storage.has_flushable_dirty_chunks();
        drop(storage);
        report.timings.flush_plan_us = report
            .timings
            .flush_plan_us
            .saturating_add(elapsed_us(started));

        let planned_chunks = flush_plan.chunk_count();
        let flush_started = Instant::now();
        let (remaining_dirty, has_flushable_dirty) = if flush_plan.is_empty() {
            info!(
                attempt,
                flushed = 0usize,
                planned = 0usize,
                flush_us = elapsed_us(flush_started),
                chunk_cache_len = storage_before.chunk_cache_len,
                chunk_cache_capacity = storage_before.chunk_cache_capacity,
                region_cache_len = storage_before.region_cache_len,
                region_cache_capacity = storage_before.region_cache_capacity,
                dirty_before = storage_before.dirty_chunks,
                dirty_after = storage_before.dirty_chunks,
                %context,
                "world storage save pressure"
            );
            (storage_before.dirty_chunks, flushable_before)
        } else {
            let started = Instant::now();
            let commit = match crate::dirty_flush::write_dirty_flush_blocking(flush_plan).await {
                Ok(commit) => commit,
                Err(err) => {
                    report
                        .errors
                        .push(format!("dirty chunks: flush write failed: {err}"));
                    report.timings.flush_write_us = report
                        .timings
                        .flush_write_us
                        .saturating_add(elapsed_us(started));
                    break;
                }
            };
            report.timings.flush_write_us = report
                .timings
                .flush_write_us
                .saturating_add(elapsed_us(started));

            let started = Instant::now();
            let install = {
                let mut storage = crate::lock_metrics::timed_guard(
                    crate::lock_metrics::LockMetricKind::SaveAllFlush,
                    "save-all dirty flush install",
                    Instant::now(),
                    world.lock().await,
                );
                if barrier_snapshot {
                    storage.install_dirty_flush_snapshot(commit)
                } else {
                    storage.install_dirty_flush(commit)
                }
            };
            let install = match install {
                Ok(install) => install,
                Err(err) => {
                    report
                        .errors
                        .push(format!("dirty chunks: flush install failed: {err}"));
                    report.timings.flush_commit_us = report
                        .timings
                        .flush_commit_us
                        .saturating_add(elapsed_us(started));
                    break;
                }
            };
            let synced =
                match crate::dirty_flush::sync_dirty_flush_install_blocking_typed(install).await {
                    Ok(synced) => synced,
                    Err(err) => {
                        report
                            .errors
                            .push(format!("dirty chunks: flush sync failed: {err}"));
                        report.timings.flush_commit_us = report
                            .timings
                            .flush_commit_us
                            .saturating_add(elapsed_us(started));
                        break;
                    }
                };
            let mut storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::SaveAllFlush,
                "save-all dirty flush finalize",
                Instant::now(),
                world.lock().await,
            );
            let finalized = storage.finalize_dirty_flush(synced);
            let flushed = if barrier_snapshot {
                finalized.installed_chunks()
            } else {
                finalized.cleaned_chunks()
            };
            report.chunks_flushed = report.chunks_flushed.saturating_add(flushed);
            let storage_after = storage.stats();
            let has_flushable_dirty = storage.has_flushable_dirty_chunks();
            info!(
                attempt,
                flushed,
                planned = planned_chunks,
                flush_us = elapsed_us(flush_started),
                chunk_cache_len = storage_after.chunk_cache_len,
                chunk_cache_capacity = storage_after.chunk_cache_capacity,
                region_cache_len = storage_after.region_cache_len,
                region_cache_capacity = storage_after.region_cache_capacity,
                dirty_before = storage_before.dirty_chunks,
                dirty_after = storage_after.dirty_chunks,
                %context,
                "world storage save pressure"
            );
            report.timings.flush_commit_us = report
                .timings
                .flush_commit_us
                .saturating_add(elapsed_us(started));
            (storage_after.dirty_chunks, has_flushable_dirty)
        };

        if remaining_dirty == 0 {
            world_flush_clean = true;
            break;
        }
        if !require_clean_dirty_flush {
            break;
        }
        if !has_flushable_dirty {
            report.errors.push(format!(
                "dirty chunks: final flush found {remaining_dirty} journal-pending chunks after producer drain"
            ));
            break;
        }
        info!(
            attempt,
            dirty = remaining_dirty,
            %context,
            "final dirty flush retrying changed chunks"
        );
    }

    if world_flush_clean && let Some(watermark) = world_chunk_journal_watermark {
        let checkpoint = sessions.world_chunk_journal().map(|journal| {
            tokio::task::spawn_blocking(move || journal.checkpoint_through(watermark))
        });
        if let Some(checkpoint) = checkpoint {
            match checkpoint.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    sessions.report_world_chunk_journal_failure();
                    report
                        .errors
                        .push(format!("world chunk journal checkpoint failed: {error}"));
                }
                Err(error) => {
                    sessions.report_world_chunk_journal_failure();
                    report.errors.push(format!(
                        "world chunk journal checkpoint worker failed: {error}"
                    ));
                }
            }
        }
    }

    let started = Instant::now();
    let (players_saved, acknowledged_players, player_errors) =
        save_player_states_blocking(root.clone(), Arc::clone(&config.items), players).await;
    report.players_saved = players_saved;
    report.errors.extend(player_errors);
    sessions.acknowledge_saved_player_states(&acknowledged_players);
    report.timings.players_us = elapsed_us(started);

    let started = Instant::now();
    let entity_count = entities.records.len();
    match save_entities_blocking(root.clone(), Arc::clone(&config.items), entities).await {
        Ok(()) => {
            report.entities_saved = entity_count;
            if let Err(error) = sessions.clear_recovered_entity_commits(&entity_journal_phases) {
                report
                    .errors
                    .push(format!("entity journal checkpoint failed: {error:?}"));
            }
        }
        Err(err) => report.errors.push(format!("entities: save failed: {err}")),
    }
    report.timings.entities_us = elapsed_us(started);

    let started = Instant::now();
    let metadata = play::persistence::WorldPersistedMetadata {
        world_time,
        daylight_cycle_enabled,
        weather,
        players_sleeping_percentage,
        keep_inventory,
        world_identity: play::persistence::world_identity(&root),
    };
    match save_world_metadata_blocking(root.clone(), metadata).await {
        Ok(()) => report.world_metadata_saved = true,
        Err(err) => report
            .errors
            .push(format!("world metadata: save failed: {err}")),
    }
    report.timings.metadata_us = elapsed_us(started);
    report.timings.total_us = elapsed_us(total_started);

    report
}

pub(super) async fn save_player_states_blocking(
    root: std::path::PathBuf,
    items: Arc<ItemRegistry>,
    players: Vec<(
        uuid::Uuid,
        play::persistence::PlayerPersistedState,
        Option<u64>,
    )>,
) -> (usize, Vec<(uuid::Uuid, u64)>, Vec<String>) {
    match tokio::task::spawn_blocking(move || {
        let mut saved = 0usize;
        let mut acknowledged = Vec::new();
        let mut errors = Vec::new();
        for (uuid, player, disconnected_generation) in players {
            match play::persistence::save_player_state(&root, uuid, &items, &player) {
                Ok(()) => {
                    saved += 1;
                    if let Some(generation) = disconnected_generation {
                        acknowledged.push((uuid, generation));
                    }
                }
                Err(err) => errors.push(format!("player {uuid}: save failed: {err}")),
            }
        }
        (saved, acknowledged, errors)
    })
    .await
    {
        Ok(result) => result,
        Err(err) => (
            0,
            Vec::new(),
            vec![format!("players: save worker failed: {err}")],
        ),
    }
}

pub(super) async fn save_entities_blocking(
    root: std::path::PathBuf,
    items: Arc<ItemRegistry>,
    entities: play::persistence::PersistedEntityCheckpoint,
) -> Result<(), String> {
    crate::blocking::spawn_result_blocking(move || {
        play::persistence::save_persisted_entity_records(&root, &items, &entities)
    })
    .await
}

pub(super) async fn save_world_metadata_blocking(
    root: std::path::PathBuf,
    metadata: play::persistence::WorldPersistedMetadata,
) -> Result<(), String> {
    crate::blocking::spawn_result_blocking(move || {
        play::persistence::save_world_metadata(&root, &metadata)
    })
    .await
}
