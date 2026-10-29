//! WAL journal entries, native writer and checkpoint commit.
//! Moved verbatim from `core.rs`.

use super::*;

/// One checkpoint frozen on the tick thread for the journal thread.
pub(super) struct CheckpointJob {
    pub(super) tick: u64,
    pub(super) globals: std::collections::BTreeMap<String, mc_world::native::NativeDomainRecord>,
    pub(super) dimensions: std::collections::BTreeMap<
        crate::dimensions::Dimension,
        crate::dimensions::checkpoint::Capture,
    >,
    pub(super) regions: crate::world::ChunkCapture,
    pub(super) players: Vec<CheckpointPlayerData>,
    pub(super) entities: std::collections::BTreeMap<(i32, i32), mc_world::anvil::EntityChunk>,
    pub(super) items: std::sync::Arc<mc_data::items::ItemRegistry>,
    pub(super) world_spawn: [i32; 3],
    pub(super) world_spawn_rotation: [f32; 2],
    pub(super) scoreboard: crate::scoreboard::Scoreboard,
    pub(super) command_storage: crate::command_storage::Storage,
    pub(super) saved_maps: std::collections::BTreeMap<i32, Arc<crate::maps::SavedMap>>,
    pub(super) bossbars: crate::bossbar::BossBars,
    pub(super) raids: crate::raids::Raids,
    pub(super) wandering_trader: crate::mob_lanes::TraderSpawner,
    pub(super) scheduled_functions: Vec<(u64, String)>,
    pub(super) datapack_selection: crate::datapacks::Selection,
    pub(super) generated_entity_claims: std::collections::BTreeSet<String>,
    pub(super) tickets: std::collections::BTreeMap<String, ([i32; 2], (u64, i32))>,
    pub(super) block_ticks: crate::block_ticks::BlockTicksImage,
    pub(super) fluid_ticks: crate::fluids::FluidTicksImage,
    pub(super) inhabited_time: Vec<crate::journal::InhabitedTimeImage>,
    pub(super) debt: Vec<crate::journal::SimDebtImage>,
    pub(super) climate: crate::weather::ClimateImage,
    pub(super) border: crate::command_border::Border,
    pub(super) difficulty: crate::survival::Difficulty,
    pub(super) gamerules: std::collections::BTreeMap<String, String>,
    /// Per-record settlement rows (`settlement/<id>`) plus the shared
    /// `settlement_sequence` row. No aggregate registry blob.
    pub(super) settlements:
        std::collections::BTreeMap<String, mc_world::native::NativeDomainRecord>,
    /// Block-log deltas since the previous capture and retention floors.
    pub(super) block_log: crate::block_log::Persist,
    /// Chunks written at which revision, or `None` when the checkpoint
    /// failed (no manifest; the journal retains everything).
    pub(super) done: std::sync::mpsc::Sender<CheckpointAck>,
    pub(super) reply: Option<std::sync::mpsc::Sender<bool>>,
}

/// Freeze checkpoint `tick` (dirty chunks by `Arc`, player/entity/block-
/// entity records by value) and hand it to the journal thread, which
/// writes it in write-ahead order (see [`write_checkpoint`]). Returns
/// whether a job was sent; queue admission blocks only while the bounded
/// journal queue is full, and never waits for the disk write itself.
pub(super) fn start_checkpoint(
    state: &mut TickState,
    work_tx: &std::sync::mpsc::SyncSender<JournalWork>,
    done: &std::sync::mpsc::Sender<CheckpointAck>,
    tick: u64,
) -> bool {
    start_checkpoint_with_reply(state, work_tx, done, tick, None)
}

pub(super) fn start_checkpoint_with_reply(
    state: &mut TickState,
    work_tx: &std::sync::mpsc::SyncSender<JournalWork>,
    done: &std::sync::mpsc::Sender<CheckpointAck>,
    tick: u64,
    reply: Option<std::sync::mpsc::Sender<bool>>,
) -> bool {
    let capture_start = std::time::Instant::now();
    let (players, entities, block_entities) = checkpoint_data_for(state, tick);
    let data_time = capture_start.elapsed();
    crate::campfire::checkpoint(state);
    crate::bees::checkpoint(state);
    crate::jukebox::checkpoint(state);
    crate::brewing::checkpoint(state);
    crate::sculk::checkpoint(state);
    crate::crafter::checkpoint(state);
    crate::shelf::checkpoint(state);
    crate::shulker::checkpoint(state);
    crate::redstone::containers::checkpoint(state);
    crate::end_portals::checkpoint(state);
    let owners_time = capture_start.elapsed();
    let absent_block_ticks = state.block_ticks.checkpoint(&mut state.world, tick);
    let absent_fluid_ticks = state.fluid_ticks.checkpoint(&mut state.world, tick);
    let scheduler_time = capture_start.elapsed();
    let mut regions = state.world.capture_dirty(&block_entities);
    let chunks_time = capture_start.elapsed();
    state
        .world
        .capture_scheduled_block_ticks(&mut regions, absent_block_ticks);
    state
        .world
        .capture_scheduled_fluid_ticks(&mut regions, absent_fluid_ticks);
    let absent_time = capture_start.elapsed();
    let dimensions = crate::dimensions::checkpoint::capture(state, tick);
    let dimensions_time = capture_start.elapsed();
    if regions.load_error().is_some()
        || dimensions
            .values()
            .any(|capture| capture.regions.load_error().is_some())
    {
        if let Some(reply) = reply {
            let _ = reply.send(false);
        }
        return false;
    }
    let job = CheckpointJob {
        tick,
        globals: state
            .native_globals
            .iter()
            .filter(|(name, _)| name.as_str() != "plugins")
            .map(|(name, record)| (name.clone(), record.clone()))
            .collect(),
        dimensions,
        regions,
        players,
        entities,
        items: std::sync::Arc::clone(&state.registry.items),
        world_spawn: state.world.world_spawn(),
        world_spawn_rotation: state.world.world_spawn_rotation(),
        scoreboard: state.scoreboard.clone(),
        command_storage: state.command_storage.clone(),
        saved_maps: state.maps.records.clone(),
        bossbars: state.bossbars.clone(),
        raids: state.raids.clone(),
        wandering_trader: state.trader_spawner,
        scheduled_functions: state.datapacks.image(),
        datapack_selection: state.datapacks.selection().clone(),
        generated_entity_claims: state.generated_entity_claims.clone(),
        tickets: checkpoint_tickets(state, tick),
        block_ticks: state.block_ticks.image(),
        fluid_ticks: state.fluid_ticks.image(),
        inhabited_time: state.world.inhabited_checkpoint(tick),
        debt: state.world.debt_checkpoint(tick),
        climate: crate::weather::ClimateImage::capture(state),
        border: state.border,
        difficulty: state.difficulty,
        gamerules: state
            .gamerules
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        settlements: match crate::settlements::persistence::checkpoint_rows(state, tick) {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(tick, %error, "settlement checkpoint skipped; journal retained");
                if let Some(reply) = reply {
                    let _ = reply.send(false);
                }
                return false;
            }
        },
        block_log: crate::block_log::checkpoint(state),
        done: done.clone(),
        reply,
    };
    if std::env::var("MC_CORE_PHASE_LOG").is_ok() {
        eprintln!(
            "checkpoint capture ms: tick={} data={:.2} owners={:.2} scheduler={:.2} chunks={:.2} absent={:.2} dimensions={:.2} job={:.2}",
            tick,
            data_time.as_secs_f64() * 1000.0,
            (owners_time - data_time).as_secs_f64() * 1000.0,
            (scheduler_time - owners_time).as_secs_f64() * 1000.0,
            (chunks_time - scheduler_time).as_secs_f64() * 1000.0,
            (absent_time - chunks_time).as_secs_f64() * 1000.0,
            (dimensions_time - absent_time).as_secs_f64() * 1000.0,
            (capture_start.elapsed() - dimensions_time).as_secs_f64() * 1000.0,
        );
    }
    work_tx.send(JournalWork::Checkpoint(Box::new(job))).is_ok()
}

/// Settlement rows the WAL deleted since the previous checkpoint: present in
/// the durable globals image but absent from this capture. Removing them here
/// keeps the tombstone and the row deletion in one atomic transaction.
pub(super) fn list_stale_settlement_rows(
    globals: &std::collections::BTreeMap<String, mc_world::native::NativeDomainRecord>,
    captured: &std::collections::BTreeMap<String, mc_world::native::NativeDomainRecord>,
) -> Vec<String> {
    globals
        .keys()
        .filter(|key| {
            key.starts_with("settlement/")
                && !key.starts_with("settlement_sequence")
                && !captured.contains_key(key.as_str())
        })
        .cloned()
        .collect()
}

/// Freeze durable ticket post-images on the same tick thread as all world domains.
pub(crate) fn checkpoint_tickets(
    state: &TickState,
    tick: u64,
) -> std::collections::BTreeMap<String, ([i32; 2], (u64, i32))> {
    state
        .tickets
        .iter()
        .filter(|(owner, _)| !owner.starts_with("solaris:portal:"))
        .map(|(owner, (centre, radius))| (owner.clone(), ([centre.x, centre.z], (tick, *radius))))
        .collect()
}

pub(super) fn player_journal_entry(
    uuid: String,
    record: &crate::journal::ReplayPlayer,
    previous: Option<&crate::journal::ReplayPlayer>,
    tick: u64,
) -> JournalEntry {
    JournalEntry::Player {
        uuid,
        opaque_player_fields: record.opaque_player_fields.clone(),
        statistics: Box::new(record.statistics.clone()),
        advancement_progress: previous
            .is_none_or(|old| !Arc::ptr_eq(&old.advancement_progress, &record.advancement_progress))
            .then(|| Arc::clone(&record.advancement_progress)),
        unlocked_recipes: previous
            .is_none_or(|old| !Arc::ptr_eq(&old.unlocked_recipes, &record.unlocked_recipes))
            .then(|| Arc::clone(&record.unlocked_recipes)),
        dimension: record.dimension,
        pos: record.pos,
        yaw: record.yaw,
        pitch: record.pitch,
        health: record.health,
        food: record.food,
        saturation: record.saturation,
        exhaustion: record.exhaustion,
        air: record.air,
        experience_level: record.experience_level,
        experience_progress: record.experience_progress,
        experience_total: record.experience_total,
        game_mode: record.game_mode,
        selected: record.selected,
        inventory: record.inventory.clone(),
        carried: record.carried.clone(),
        grid: record.grid.clone(),
        respawn: record.respawn,
        respawn_dimension: record.respawn_dimension,
        respawn_forced: record.respawn_forced,
        respawn_yaw: record.respawn_yaw,
        respawn_pitch: record.respawn_pitch,
        active_effects: record.active_effects.clone(),
        current_impulse_impact_pos: record.current_impulse_impact_pos,
        fall_flying: record.fall_flying,
        enchantment_seed: record.enchantment_seed,
        enchantment_offer_seed: record.enchantment_offer_seed,
        warden_warning: record.warden_warning,
        burn_ticks: record.burn_ticks,
        portal: record.portal,
        tick,
    }
}

/// Build journal entries for one committed tick (absolute post-images).
pub(crate) fn journal_entries_for_tick(
    state: &mut TickState,
    output: &TickOutput,
) -> Vec<JournalEntry> {
    // Classify item births/crossings before entity_images advances the
    // existing WAL baselines (restored items already have a baseline).
    crate::abstract_sim::cancel_external_writes(state);
    let mut entries = Vec::new();
    crate::plugin::journal(state, &mut entries);
    // Settlement images always ride an explicit Dimension envelope, every
    // dimension including Overworld. Child ticks extend the raw images and the
    // existing child wrap below supplies their envelope.
    let mut settlement_entries = Vec::new();
    crate::settlements::persistence::journal_entries(state, output.tick, &mut settlement_entries);
    if state.dimension == crate::dimensions::Dimension::Overworld {
        if !settlement_entries.is_empty() {
            entries.push(JournalEntry::Dimension {
                dimension: state.dimension,
                entries: settlement_entries,
            });
        }
    } else {
        entries.extend(settlement_entries);
    }
    state.block_ticks.journal_entries(&mut entries);
    state.fluid_ticks.journal(&mut entries);
    state.world.journal_inhabited_time(&mut entries);
    crate::end_fight::journal(state, &mut entries);
    crate::raids::journal(state, &mut entries);
    crate::mob_lanes::journal(state, &mut entries);
    state.datapacks.journal(&mut entries);
    if state.dimension == crate::dimensions::Dimension::Overworld
        && std::mem::take(&mut state.gamerules_dirty)
    {
        entries.push(JournalEntry::GameRules {
            values: state
                .gamerules
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        });
    }
    let climate = crate::weather::ClimateImage::capture(state);
    if state.dimension == crate::dimensions::Dimension::Overworld
        && state.journaled_climate != Some(climate)
    {
        entries.push(JournalEntry::Climate { value: climate });
        state.journaled_climate = Some(climate);
    }
    if state.journaled_border != Some(state.border) {
        entries.push(JournalEntry::Border {
            value: state.border,
        });
        state.journaled_border = Some(state.border);
    }
    if state.dimension == crate::dimensions::Dimension::Overworld
        && state.journaled_difficulty != Some(state.difficulty)
    {
        entries.push(JournalEntry::Difficulty {
            value: state.difficulty,
        });
        state.journaled_difficulty = Some(state.difficulty);
    }
    for claim in std::mem::take(&mut state.new_generated_entity_claims) {
        entries.push(JournalEntry::GeneratedEntityClaim { claim });
    }
    if state.dimension == crate::dimensions::Dimension::Overworld
        && std::mem::take(&mut state.scoreboard_dirty)
    {
        entries.push(JournalEntry::Scoreboard {
            value: state.scoreboard.clone(),
        });
    }
    if state.dimension == crate::dimensions::Dimension::Overworld
        && std::mem::take(&mut state.command_storage_dirty)
    {
        entries.push(JournalEntry::CommandStorage {
            value: crate::command_storage::encode(&state.command_storage),
        });
    }
    if state.dimension == crate::dimensions::Dimension::Overworld
        && std::mem::take(&mut state.bossbars_dirty)
    {
        entries.push(JournalEntry::Bossbars {
            value: state.bossbars.to_bytes(),
        });
    }
    if state.dimension == crate::dimensions::Dimension::Overworld {
        for id in std::mem::take(&mut state.maps.dirty) {
            entries.push(JournalEntry::SavedMap {
                id,
                value: Arc::clone(&state.maps.records[&id]),
            });
        }
    }
    // Removals precede final storage images: a replacement at the same cell
    // must clear its old family without deleting the newly installed entity.
    let mut removed = std::mem::take(&mut state.block_entity_removed);
    removed.extend(
        state
            .world
            .take_block_entity_removals()
            .into_iter()
            .map(|pos| (pos.x, pos.y, pos.z)),
    );
    removed.sort_unstable();
    removed.dedup();
    for (x, y, z) in removed {
        state.journaled_block_entities.remove(&(x, y, z));
        entries.push(JournalEntry::RemoveBlockEntity {
            pos: [x, y, z],
            tick: output.tick,
        });
    }
    // Block-entity post-images (absolute) for containers whose custody was
    // touched since the last batch and whose contents differ from their last
    // journaled image (equality stays the arbiter).
    for cell in state.block_entities.take_journal_touched() {
        let Some(live) = state.block_entities.get(&cell) else {
            state.journaled_block_entities.remove(&cell);
            continue;
        };
        let image = (live.slots.clone(), live.furnace_image());
        if state.journaled_block_entities.get(&cell) == Some(&image) {
            continue;
        }
        entries.push(JournalEntry::BlockEntity {
            pos: [cell.0, cell.1, cell.2],
            slots: image.0.clone(),
            furnace: image.1.clone(),
            tick: output.tick,
        });
        state.journaled_block_entities.insert(cell, image);
    }
    for change in &output.changes.blocks {
        entries.push(JournalEntry::Block {
            pos: block_pos_array(change.position),
            state: change.state,
        });
    }
    let mut opaque = std::mem::take(&mut state.opaque_changes);
    // Keep the latest owner image for a cell. Timer-only owners deliberately
    // leave the chunk's wire/checkpoint image stale until capture.
    opaque.reverse();
    opaque.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
    opaque.dedup_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
    for (pos, staged) in opaque {
        let live = state
            .world
            .chunk(crate::boundary::ChunkPos {
                x: pos.x.div_euclid(16),
                z: pos.z.div_euclid(16),
            })
            .and_then(|chunk| chunk.block_entities.get(&pos));
        // Presence fences removed storage, not timer values: replacing a live
        // owner image with older cached NBT would roll retained timers back.
        let nbt = match (staged, live) {
            (Some(bytes), Some(_)) => Some(bytes),
            (_, live) => live.cloned(),
        };
        entries.push(JournalEntry::OpaqueBlockEntity {
            pos: block_pos_array(pos),
            nbt,
        });
    }
    crate::block_log::journal(state, &mut entries);
    // Player post-images whose durable fields (inventory, selected, vitals
    // incl. xp, game mode, position) differ from the last journaled one.
    // The open menu's grid/cursor fold back via close semantics at journal
    // time: journal the state as if closed.
    state
        .journaled_players
        .retain(|id, _| state.players.contains_key(id));
    let ids: Vec<EntityId> = state.players.keys().copied().collect();
    for id in ids {
        let Some(mut record) = player_record(state, id, 0, false) else {
            continue;
        };
        let statistics = &state.players[&id].statistics;
        if let Some(previous) = state.journaled_players.get_mut(&id)
            && record.eq_except_statistics(previous)
        {
            let changes = statistics.changes_since(&previous.statistics);
            let timers = (statistics.timers() != previous.statistics.timers())
                .then_some(statistics.timers());
            if !changes.is_empty() || timers.is_some() {
                previous.statistics.apply_changes(&changes);
                if let Some(timers) = timers {
                    previous.statistics.apply_timers(timers);
                }
                entries.push(JournalEntry::Statistics {
                    uuid: state
                        .entities
                        .slot_of_any(id)
                        .map(|slot| state.entities.row(slot).uuid.to_string())
                        .unwrap_or_default(),
                    changes,
                    timers,
                    tick: output.tick,
                });
            }
            continue;
        }
        record.statistics = statistics.clone();
        let previous = state.journaled_players.get(&id);
        let uuid = state
            .entities
            .slot_of_any(id)
            .map(|slot| state.entities.row(slot).uuid.to_string())
            .unwrap_or_default();
        entries.push(player_journal_entry(uuid, &record, previous, output.tick));
        state.journaled_players.insert(id, record);
    }
    // Leave freezes timers and closes menu custody before removing the live
    // player. Its final postimage must enter this WAL batch, not wait for a
    // checkpoint while the previous connected anchor keeps accruing time.
    for uuid in std::mem::take(&mut state.left_players) {
        if let Some(record) = state.offline_players.get(&uuid)
            && record.tick == output.tick
        {
            entries.push(player_journal_entry(
                uuid.to_string(),
                record,
                None,
                output.tick,
            ));
        }
    }
    crate::journal_touched::entity_images(state, output, &mut entries);
    for id in &output.changes.entities_removed {
        // Wire removals include intermediate dimension departures. A same-tick
        // return has live custody here again and must keep its final image.
        if state.entities.slot_of(*id).is_some() {
            continue;
        }
        entries.push(JournalEntry::RemoveEntity {
            id: entity_id_int(*id),
            tick: output.tick,
        });
    }
    crate::chunk_tickets::journal(state, output.tick, &mut entries);
    for (dimension, world) in &mut state.other_dimensions {
        let child_output = output
            .snapshot
            .dimensions
            .get(dimension)
            .expect("every resident dimension publishes in the same tick");
        entries.push(JournalEntry::Dimension {
            dimension: *dimension,
            entries: journal_entries_for_tick(world, child_output),
        });
    }
    state.world.journal_debt(&mut entries);
    entries
}

/// Work for the native writer thread, the single redb writer. FIFO, so saves
/// of generated chunks stay ordered with checkpoints.
pub(super) enum NativeWork {
    /// Commit a checkpoint whose WAL coverage the journal thread already gated
    /// (`Err`: the gate failed, so nothing is written and the job reports failure).
    /// The plugin mirror is the journal thread's FIFO image frozen at `job.tick`.
    Checkpoint {
        job: Box<CheckpointJob>,
        plugins: Result<Box<crate::plugin::PluginState>, CoreError>,
    },
    SaveGenerated(crate::world::ChunkCapture),
    /// Copy the last committed checkpoint and the WAL after it (hot backup).
    Backup {
        job: BackupJob,
        through: u64,
    },
    /// Explicit orderly termination, queued after the final journal flush.
    Shutdown,
}

/// Journal thread: owns WAL appends and their fsyncs, and gates each
/// checkpoint's WAL coverage. Checkpoint jobs arrive FIFO behind the appends of
/// every tick they cover, so the gate only has to confirm coverage (see
/// [`write_checkpoint`]). The multi-second redb commit runs on a second thread
/// ([`native_writer_loop`]) so a later tick's `Append` is never parked behind
/// it. The function returns only after that thread has drained, so a shutdown
/// still completes the final checkpoint.
pub(super) fn journal_loop(
    world_dir: &std::path::Path,
    work_rx: std::sync::mpsc::Receiver<JournalWork>,
    mut writer: JournalWriter,
    generated_busy: &std::sync::atomic::AtomicBool,
    budget: &JournalBudget,
    initial_manifest: CheckpointManifest,
    mut plugins: crate::plugin::PluginState,
) {
    // Tick of the last committed checkpoint, published by the native writer
    // before it acknowledges. In normal running a gate only follows the previous
    // checkpoint's ACK (`checkpoint_in_flight`), so it reads that tick. A
    // shutdown may start its final checkpoint with one still in flight; the gate
    // then reads an older tick. The writer's verified startup interval and
    // contiguous append coverage keep that gate independent of pruned files.
    let committed_tick = std::sync::atomic::AtomicU64::new(initial_manifest.tick);
    // Set by the native writer after a panic; the journal side then halts exactly
    // like an append failure.
    let native_failed = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        // Owned by this closure: returning drops the receiver at once (before the
        // scope joins the native writer), so the tick's next `send` fails instead of
        // blocking behind an in-flight commit.
        let work_rx = work_rx;
        // Returning on any path closes the budget, waking a tick thread parked
        // in `reserve` so it halts like a failed `send`.
        let _close = crate::journal_budget::CloseOnDrop(budget);
        let (native_tx, native_rx) = std::sync::mpsc::channel::<NativeWork>();
        std::thread::Builder::new()
            .name("mc-core-native".to_owned())
            .spawn_scoped(scope, || {
                native_writer_loop(
                    world_dir,
                    native_rx,
                    generated_busy,
                    &committed_tick,
                    &native_failed,
                    initial_manifest,
                );
            })
            .expect("spawn native checkpoint writer");
        // Fail-closed journal: once an append fails, the WAL has a hole no
        // later fsync can fill. Return (dropping our receiver) so the tick
        // loop's next `work_tx.send` fails — the existing channel carries the
        // failure to the tick owner, which halts (no more commits, no more
        // publishes, Disconnect to all). No retry worker: the hole is
        // unfillable.
        while let Ok(work) = work_rx.recv() {
            if native_failed.load(std::sync::atomic::Ordering::Acquire) {
                tracing::error!("native checkpoint writer died; world halting (fail closed)");
                return;
            }
            match work {
                JournalWork::Append {
                    tick_lo,
                    tick_hi,
                    entries,
                    plugin_ack,
                    reserved,
                } => {
                    if let Err(error) = writer.append(tick_lo, tick_hi, &entries) {
                        tracing::error!(tick_lo, tick_hi, error = %stop::Chain(&error), "journal append failed; world halting (fail closed)");
                        return;
                    }
                    for entry in &entries {
                        if let JournalEntry::Plugins { value } = entry {
                            plugins.apply_delta(value);
                        }
                    }
                    if let Some(ack) = plugin_ack {
                        let _ = ack.send(tick_hi);
                    }
                    #[cfg(test)]
                    if let Some((root, observer)) = JOURNAL_APPEND_OBSERVER
                        .lock()
                        .expect("journal append observer")
                        .as_ref()
                        && root == world_dir
                    {
                        if let Err(error) = writer.flush() {
                            tracing::error!(tick_hi, error = %stop::Chain(&error), "journal flush failed; world halting (fail closed)");
                            return;
                        }
                        observer(tick_hi, &entries);
                    }
                    budget.release(reserved);
                }
                JournalWork::Checkpoint(job) => {
                    let gate = writer.gate(
                        committed_tick.load(std::sync::atomic::Ordering::Acquire),
                        job.tick,
                    );
                    let plugins = gate.map(|()| Box::new(plugins.clone()));
                    if native_tx
                        .send(NativeWork::Checkpoint { job, plugins })
                        .is_err()
                    {
                        tracing::error!(
                            cause = "native writer channel closed",
                            "journal checkpoint handoff failed; world halting"
                        );
                        return;
                    }
                }
                JournalWork::Backup(job) => match writer.flush() {
                    // Everything appended so far is durable; the native writer
                    // copies it behind every earlier checkpoint commit.
                    Ok(()) => {
                        let through = writer.covered_through().unwrap_or(0);
                        if native_tx.send(NativeWork::Backup { job, through }).is_err() {
                            tracing::error!(
                                cause = "native writer channel closed",
                                "journal backup handoff failed; world halting"
                            );
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = job.reply.send(Err(error));
                    }
                },
                JournalWork::SaveGenerated(chunks) => {
                    if native_tx.send(NativeWork::SaveGenerated(chunks)).is_err() {
                        tracing::error!(
                            cause = "native writer channel closed",
                            "journal generated-chunk handoff failed; world halting"
                        );
                        return;
                    }
                }
                JournalWork::Shutdown => {
                    if let Err(error) = writer.flush() {
                        tracing::error!(error = %stop::Chain(&error), "journal final flush failed");
                        return;
                    }
                    if native_tx.send(NativeWork::Shutdown).is_err() {
                        tracing::error!(
                            cause = "native writer channel closed",
                            "journal shutdown handoff failed"
                        );
                        return;
                    }
                    tracing::info!(cause = "shutdown requested", "core journal owner stopped");
                    return;
                }
            }
        }
        tracing::error!(
            cause = "tick work channel closed",
            "core journal owner stopped"
        );
    });
}

/// Native writer thread: the single redb writer for checkpoints and generated
/// saves. In normal running at most one checkpoint and one generated save are
/// outstanding (`checkpoint_in_flight`, `generated_busy`); shutdown may add a
/// final checkpoint and more generated saves, which this FIFO drains before the
/// journal scope exits. Only a successful Immediate commit permits pruning
/// batches wholly covered by that newly durable checkpoint.
///
/// A panic in one job fails closed: the job reports failure on `done`/`reply`,
/// `native_failed` makes the journal side halt, and this thread stops.
pub(super) fn native_writer_loop(
    world_dir: &std::path::Path,
    work_rx: std::sync::mpsc::Receiver<NativeWork>,
    generated_busy: &std::sync::atomic::AtomicBool,
    committed_tick: &std::sync::atomic::AtomicU64,
    native_failed: &std::sync::atomic::AtomicBool,
    initial_manifest: CheckpointManifest,
) {
    // The current complete manifest is the recovery/pruning authority.
    let mut prev_manifest = initial_manifest;
    let mut block_log = crate::block_log::Backlog::default();
    while let Ok(work) = work_rx.recv() {
        let notify = match &work {
            NativeWork::Checkpoint { job, .. } => Some((job.done.clone(), job.reply.clone())),
            NativeWork::SaveGenerated(_) | NativeWork::Backup { .. } | NativeWork::Shutdown => None,
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_native_work(
                work,
                world_dir,
                generated_busy,
                committed_tick,
                &mut prev_manifest,
                &mut block_log,
            )
        }));
        if matches!(outcome, Ok(std::ops::ControlFlow::Break(()))) {
            tracing::info!(
                cause = "shutdown requested",
                "native checkpoint writer stopped"
            );
            return;
        }
        if let Err(payload) = outcome {
            tracing::error!(
                panic = stop::panic_message(&*payload),
                "native checkpoint writer panicked; world halting (fail closed)"
            );
            if let Some((done, reply)) = notify {
                let _ = done.send(None);
                if let Some(reply) = reply {
                    let _ = reply.send(false);
                }
            }
            native_failed.store(true, std::sync::atomic::Ordering::Release);
            return;
        }
    }
    tracing::error!(
        cause = "journal work channel closed",
        "native checkpoint writer stopped"
    );
}

pub(super) fn run_native_work(
    work: NativeWork,
    world_dir: &std::path::Path,
    generated_busy: &std::sync::atomic::AtomicBool,
    committed_tick: &std::sync::atomic::AtomicU64,
    prev_manifest: &mut CheckpointManifest,
    block_log: &mut crate::block_log::Backlog,
) -> std::ops::ControlFlow<()> {
    match work {
        NativeWork::Shutdown => return std::ops::ControlFlow::Break(()),
        NativeWork::Checkpoint { mut job, plugins } => {
            let started = std::time::Instant::now();
            // Block-log rows join the table before the commit that may prune
            // their WAL; a failed write keeps them queued for the next job.
            block_log.absorb(std::mem::take(&mut job.block_log));
            let written = plugins.and_then(|plugins| {
                let store = job
                    .regions
                    .store()
                    .ok_or_else(|| CoreError::Storage("checkpoint has no native store".into()))?;
                block_log.write(store, job.regions.registry(), &job.items)?;
                commit_checkpoint(world_dir, &job, &plugins, prev_manifest)
            });
            let committed = written.is_ok();
            match &written {
                Ok((_, _, io)) => {
                    committed_tick.store(prev_manifest.tick, std::sync::atomic::Ordering::Release);
                    tracing::info!(
                        tick = job.tick,
                        total_ms = started.elapsed().as_millis() as u64,
                        chunk_ms = io.chunk_micros / 1000,
                        entity_ms = io.entity_micros / 1000,
                        db_commit_ms = io.commit_micros / 1000,
                        entities_visited = io.entities_visited,
                        rewritten_entities = io.rewritten_entities,
                        entity_payload_bytes = io.entity_payload_bytes,
                        rewritten_chunks = io.rewritten_chunks,
                        chunk_payload_bytes = io.chunk_payload_bytes,
                        sections_encoded = io.sections_encoded,
                        section_records_written = io.section_records_written,
                        "checkpoint committed"
                    );
                }
                Err(error) => {
                    tracing::warn!(tick = job.tick, %error, "checkpoint skipped; journal retained");
                }
            }
            let CheckpointJob {
                block_ticks,
                fluid_ticks,
                dimensions,
                done,
                reply,
                ..
            } = *job;
            let ack = written.ok().map(|(written, mut child_written, _)| {
                let dimensions = dimensions
                    .into_iter()
                    .map(|(dimension, capture)| {
                        (
                            dimension,
                            crate::dimensions::checkpoint::Ack {
                                written: child_written
                                    .remove(&dimension)
                                    .expect("completed dimension write"),
                                block_ticks: capture.block_ticks,
                                fluid_ticks: capture.fluid_ticks,
                            },
                        )
                    })
                    .collect();
                (written, block_ticks, fluid_ticks, dimensions)
            });
            let _ = done.send(ack);
            if let Some(reply) = reply {
                let _ = reply.send(committed);
            }
            if committed {
                prune_batches_through(&world_dir.join("journal"), prev_manifest.tick);
            }
        }
        NativeWork::SaveGenerated(chunks) => {
            // Best effort: a failed save only means generating again.
            if let Err(error) = chunks.fill_generated() {
                tracing::warn!(%error, "generated chunks not saved");
            }
            generated_busy.store(false, std::sync::atomic::Ordering::Release);
        }
        NativeWork::Backup { job, through } => {
            let started = std::time::Instant::now();
            let result =
                crate::hot_backup::copy_world(world_dir, &job.out, prev_manifest.tick, through);
            match &result {
                Ok(backup) => tracing::info!(
                    checkpoint_tick = backup.checkpoint_tick,
                    wal_tick = backup.wal_tick,
                    files = backup.files.len(),
                    total_ms = started.elapsed().as_millis() as u64,
                    "hot backup copied"
                ),
                Err(error) => tracing::warn!(%error, "hot backup failed"),
            }
            let _ = job.reply.send(result);
        }
    }
    std::ops::ControlFlow::Continue(())
}

/// Gate the WAL, commit every dimension/domain in one Immediate native txn,
/// without acknowledging or pruning the captured revisions here.
pub(super) fn write_checkpoint(
    world_dir: &std::path::Path,
    job: &CheckpointJob,
    plugins: &crate::plugin::PluginState,
    writer: &mut JournalWriter,
    prev_manifest: &mut CheckpointManifest,
) -> Result<CheckpointWrite, CoreError> {
    writer.gate(prev_manifest.tick, job.tick)?;
    commit_checkpoint(world_dir, job, plugins, prev_manifest)
}

/// Native commit of a checkpoint whose WAL coverage is already gated durable.
pub(super) fn commit_checkpoint(
    _world_dir: &std::path::Path,
    job: &CheckpointJob,
    plugins: &crate::plugin::PluginState,
    prev_manifest: &mut CheckpointManifest,
) -> Result<CheckpointWrite, CoreError> {
    let written = job.regions.written();
    #[cfg(test)]
    {
        let delay = DELAY_CHECKPOINT_COMMIT
            .lock()
            .as_ref()
            .filter(|(dir, _)| *dir == _world_dir)
            .map(|(_, delay)| *delay);
        if let Some(delay) = delay {
            std::thread::sleep(delay);
        }
    }
    #[cfg(test)]
    if PANIC_CHECKPOINT_COMMIT
        .lock()
        .take_if(|dir| dir.as_path() == _world_dir)
        .is_some()
    {
        panic!("injected native writer panic");
    }
    #[cfg(test)]
    if FAIL_CHECKPOINT_AFTER_REGIONS
        .lock()
        .ok()
        .and_then(|mut armed| armed.take_if(|dir| *dir == _world_dir))
        .is_some()
    {
        return Err(CoreError::Storage("injected checkpoint failure".to_owned()));
    }
    let dimensions = crate::dimensions::checkpoint::images(&job.dimensions, job.tick);
    let dimension_written = job
        .dimensions
        .iter()
        .map(|(&dimension, capture)| (dimension, capture.regions.written()))
        .collect();
    let mut merged = prev_manifest.players.clone();
    for player in job
        .players
        .iter()
        .chain(job.dimensions.values().flat_map(|world| &world.players))
    {
        merged.insert(player.uuid.clone(), job.tick);
    }
    let manifest = CheckpointManifest {
        tick: job.tick,
        domain_versions: crate::native_checkpoint::domain_versions(),
        dimensions,
        players: merged,
        world_spawn: Some(job.world_spawn),
        world_spawn_rotation: job.world_spawn_rotation,
        generated_entity_claims: job.generated_entity_claims.clone(),
        tickets: job.tickets.clone(),
        block_ticks: job.block_ticks.clone(),
        fluid_ticks: job.fluid_ticks.clone(),
        inhabited_time: job.inhabited_time.clone(),
        debt: job.debt.clone(),
        climate: Some(job.climate),
        border: job.border,
        difficulty: job.difficulty,
        gamerules: job.gamerules.clone(),
        end_fight: None,
        raids: Some(job.raids.clone()),
        wandering_trader: Some(job.wandering_trader),
        scheduled_functions: job.scheduled_functions.clone(),
        datapack_selection: Some(job.datapack_selection.clone()),
    };
    crate::native_checkpoint::validate_manifest(&manifest)?;
    let mut globals = job.globals.clone();
    // Per-record settlement rows ride the same atomic native transaction.
    // Tombstones are row deletions: drop checkpoint rows the WAL removed.
    for (key, row) in &job.settlements {
        globals.insert(key.clone(), row.clone());
    }
    for key in list_stale_settlement_rows(&globals, &job.settlements) {
        globals.remove(&key);
    }
    // This runs only on the journal thread (or startup before tick ownership).
    // FIFO processing freezes the mirror at exactly job.tick: later appends
    // cannot be applied until this complete native transaction returns.
    globals.insert(
        "plugins".into(),
        crate::native_checkpoint::record(
            job.tick,
            serde_json::to_vec(plugins).map_err(|e| CoreError::Storage(e.to_string()))?,
        ),
    );
    globals.insert(
        "checkpoint".into(),
        crate::native_checkpoint::record(
            job.tick,
            serde_json::to_vec(&manifest).map_err(|e| CoreError::Storage(e.to_string()))?,
        ),
    );
    globals.insert(
        "scoreboard".into(),
        crate::native_checkpoint::tag_record(job.tick, &job.scoreboard.to_nbt())?,
    );
    globals.insert(
        "command_storage".into(),
        crate::native_checkpoint::tag_record(
            job.tick,
            &crate::command_storage::to_tag(&job.command_storage),
        )?,
    );
    globals.insert(
        "bossbars".into(),
        crate::native_checkpoint::tag_record(job.tick, &job.bossbars.to_nbt())?,
    );
    crate::maps::checkpoint(&job.saved_maps, job.tick, &mut globals)?;
    crate::native_checkpoint::refresh_metadata(&mut globals, &manifest)?;
    let extras = globals
        .get("conversion_extras")
        .ok_or_else(|| CoreError::Storage("missing native conversion extras".into()))?;
    crate::native_checkpoint::decode_tag(extras, "conversion_extras")?;
    for record in globals.values_mut() {
        record.captured_tick = job.tick;
    }
    let players = crate::native_checkpoint::encode_players(
        job.players
            .iter()
            .chain(job.dimensions.values().flat_map(|capture| &capture.players)),
        &job.items,
        job.tick,
    )?;
    let entities = std::iter::once((crate::dimensions::Dimension::Overworld, &job.entities))
        .chain(
            job.dimensions
                .iter()
                .map(|(&dimension, capture)| (dimension, &capture.entities)),
        )
        .flat_map(|(dimension, chunks)| {
            crate::native_checkpoint::entity_snapshots(dimension, chunks, &job.items, job.tick)
        })
        .map(|entity| entity.map_err(|e| mc_world::native::NativeError::Source(e.to_string())));
    let store = job
        .regions
        .store()
        .ok_or_else(|| CoreError::Storage("checkpoint has no native store".into()))?;
    let io = store
        .commit_checkpoint_stream(
            mc_world::native::NativeCheckpoint {
                tick: job.tick,
                previous_tick: prev_manifest.tick,
                chunks: &[],
                entities: &[],
                players: &players,
                globals: &globals,
            },
            |write| {
                for capture in std::iter::once(&job.regions)
                    .chain(job.dimensions.values().map(|capture| &capture.regions))
                {
                    capture
                        .visit_native(|snapshot| {
                            write(snapshot).map_err(|error| CoreError::Storage(error.to_string()))
                        })
                        .map_err(|error| {
                            mc_world::native::NativeError::Source(error.to_string())
                        })?;
                }
                Ok(())
            },
            entities,
            job.regions.registry(),
            &job.items,
        )
        .map_err(|e| CoreError::Storage(e.to_string()))?;
    #[cfg(test)]
    if FAIL_CHECKPOINT_AFTER_NATIVE_COMMIT
        .lock()
        .ok()
        .and_then(|mut armed| armed.take_if(|dir| *dir == _world_dir))
        .is_some()
    {
        return Err(CoreError::Storage(
            "injected crash after native commit before acknowledgement".into(),
        ));
    }
    *prev_manifest = manifest;
    Ok((written, dimension_written, io))
}
