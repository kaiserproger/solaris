use super::*;

pub(crate) fn startup_spawn_view_distance(config: &mc_server::ServerConfig) -> i32 {
    if config.autoscale.enabled {
        config
            .autoscale
            .to_policy(&config.server, &config.chunk_pipeline)
            .min_view_distance
    } else {
        config.server.view_distance
    }
    .clamp(mc_net::MIN_VIEW_DISTANCE, mc_net::MAX_VIEW_DISTANCE)
}

pub(crate) fn runtime_cache_view_distance(config: &mc_server::ServerConfig) -> i32 {
    if config.autoscale.enabled {
        config
            .autoscale
            .to_policy(&config.server, &config.chunk_pipeline)
            .max_view_distance
            .max(config.server.view_distance)
    } else {
        config.server.view_distance
    }
    .clamp(mc_net::MIN_VIEW_DISTANCE, mc_net::MAX_VIEW_DISTANCE)
}

pub(crate) fn chunk_cache_size_for_view_distance(view_distance: i32) -> usize {
    let view_distance = view_distance.max(0) as usize;
    let radius = if view_distance == 0 {
        1
    } else {
        view_distance + 2
    };
    let width = radius * 2 + 1;
    width * width
}

/// The startup bake's worker count: the configured bound wins, and `0` means the
/// process CPU count. The bake must not raise a bound the operator set - that is
/// exactly the number a constrained host uses to keep one server off every core.
pub(crate) fn startup_chunk_worker_threads(configured_workers: usize) -> usize {
    if configured_workers > 0 {
        return configured_workers;
    }
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
        .max(1)
}

pub(crate) fn startup_light_bake_worker_threads(chunk_workers: usize) -> usize {
    chunk_workers
        .saturating_mul(2)
        .clamp(1, STARTUP_LIGHT_BAKE_WORKER_CAP)
}

pub(crate) fn generate_spawn_window(
    storage: &mut mc_world::WorldStorage,
    generator: Arc<dyn mc_world::ChunkGenerator>,
    view_distance: i32,
    worker_threads: usize,
    light_bake_worker_threads: usize,
    block_light: Option<&mc_data::block_light::BlockLightTable>,
) -> Result<usize> {
    let view_distance = view_distance.max(0);
    let positions = spawn_window_positions_at(storage.spawn().chunk(), view_distance);
    let generated =
        generate_chunk_positions(storage, generator, positions, worker_threads, "spawn")?;
    if let Some(block_light) = block_light {
        bake_spawn_window_light(
            storage,
            block_light,
            view_distance,
            light_bake_worker_threads,
        )?;
    }
    Ok(generated)
}

/// Chunk positions covering an inclusive block rectangle, normalised so either
/// corner may come first.
pub(crate) fn region_positions(
    from: (i32, i32),
    to: (i32, i32),
) -> Result<Vec<mc_world::ChunkPos>> {
    let (x0, x1) = (from.0.min(to.0), from.0.max(to.0));
    let (z0, z1) = (from.1.min(to.1), from.1.max(to.1));
    let (cx0, cx1) = (x0.div_euclid(16), x1.div_euclid(16));
    let (cz0, cz1) = (z0.div_euclid(16), z1.div_euclid(16));
    let width = i64::from(cx1) - i64::from(cx0) + 1;
    let depth = i64::from(cz1) - i64::from(cz0) + 1;
    let count = width * depth;
    if count > MAX_PREGENERATE_CHUNKS as i64 {
        bail!(
            "requested region covers {count} chunks, above the {MAX_PREGENERATE_CHUNKS} chunk pre-generation cap"
        );
    }
    let mut positions = Vec::with_capacity(count as usize);
    for z in cz0..=cz1 {
        for x in cx0..=cx1 {
            positions.push(mc_world::ChunkPos { x, z });
        }
    }
    Ok(positions)
}

/// `pregenerate` writes Solaris terrain, so it must not target a world that
/// serve keeps read-only: an unversioned vanilla Anvil import.
pub(crate) fn ensure_pregenerate_target(world_source: WorldSource) -> Result<()> {
    if world_source == WorldSource::ExistingVanilla {
        bail!(
            "refusing to pre-generate into an unversioned vanilla import, which serve treats as read-only"
        );
    }
    Ok(())
}

/// Split requested positions into the ones that still need generating and the
/// count already resident or stored on disk. Already stored chunks keep their
/// blocks, so repeating a request preserves played terrain and in-game edits.
pub(crate) fn pending_region_positions(
    storage: &mut mc_world::WorldStorage,
    positions: Vec<mc_world::ChunkPos>,
) -> Result<(Vec<mc_world::ChunkPos>, usize)> {
    let mut pending = Vec::with_capacity(positions.len());
    let mut skipped = 0usize;
    for pos in positions {
        if storage.chunk_is_stored(pos)? {
            skipped += 1;
        } else {
            pending.push(pos);
        }
    }
    Ok((pending, skipped))
}

/// Generate and store every requested chunk with the configured worker count.
///
/// `label` names the batch in logs and error contexts ("spawn" for the startup
/// window, "region" for an operator-requested rectangle).
pub(crate) fn generate_chunk_positions(
    storage: &mut mc_world::WorldStorage,
    generator: Arc<dyn mc_world::ChunkGenerator>,
    positions: Vec<mc_world::ChunkPos>,
    worker_threads: usize,
    label: &str,
) -> Result<usize> {
    let total = positions.len();
    if total == 0 {
        return Ok(0);
    }

    let workers = worker_threads.max(1).min(total);
    tracing::info!(
        chunks = total,
        workers,
        label,
        "world pre-generation started",
    );

    let positions = Arc::new(positions);
    let next = Arc::new(AtomicUsize::new(0));
    let queue_batches = workers.clamp(1, STARTUP_GENERATION_QUEUE_BATCHES);
    let (tx, rx) = std::sync::mpsc::sync_channel(queue_batches);
    let batch_size = 8usize.min(total);
    let started = Instant::now();
    let log_every = (total / 20).max(64);
    let generated = std::thread::scope(|scope| -> Result<usize> {
        let mut handles = Vec::with_capacity(workers);
        for worker_index in 0..workers {
            let positions = Arc::clone(&positions);
            let next = Arc::clone(&next);
            let tx = tx.clone();
            let generator = Arc::clone(&generator);
            let handle = std::thread::Builder::new()
                .name(format!("solaris-spawn-gen-{worker_index}"))
                .spawn_scoped(scope, move || {
                    let mut batch = Vec::with_capacity(batch_size);
                    loop {
                        let idx = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&pos) = positions.get(idx) else {
                            break;
                        };
                        let chunk = generator.generate(pos);
                        batch.push((pos, chunk));
                        if batch.len() >= batch_size && tx.send(std::mem::take(&mut batch)).is_err()
                        {
                            break;
                        }
                    }
                    if !batch.is_empty() {
                        let _ = tx.send(batch);
                    }
                })
                .with_context(|| format!("spawning worldgen worker {worker_index}"))?;
            handles.push(handle);
        }
        drop(tx);

        let mut generated = 0usize;
        let mut last_log = Instant::now();
        let mut consumer_error = None;
        'receive: while let Ok(batch) = rx.recv() {
            for (pos, chunk) in batch {
                if storage.stats().dirty_chunk_cache_saturated
                    && let Err(error) = storage.flush_dirty().with_context(|| {
                        format!(
                            "flushing dirty chunks before pre-generating {label} chunk ({}, {})",
                            pos.x, pos.z
                        )
                    })
                {
                    consumer_error = Some(error);
                    break 'receive;
                }
                if let Err(error) = storage
                    .insert_generated_chunk(pos, chunk)
                    .with_context(|| format!("pre-generating {label} chunk ({}, {})", pos.x, pos.z))
                {
                    consumer_error = Some(error);
                    break 'receive;
                }
                generated += 1;
            }
            if generated == total
                || generated.is_multiple_of(log_every)
                || last_log.elapsed() >= Duration::from_secs(2)
            {
                let percent = (generated * 90 / total).min(90);
                tracing::info!("Preparing world... {percent}%");
                tracing::info!(
                    generated,
                    total,
                    label,
                    elapsed_ms = started.elapsed().as_millis(),
                    "world pre-generation progress",
                );
                last_log = Instant::now();
            }
        }
        drop(rx);

        let worker_panicked = handles
            .into_iter()
            .fold(false, |panicked, handle| handle.join().is_err() || panicked);
        if let Some(error) = consumer_error {
            return Err(error);
        }
        if worker_panicked {
            bail!("{label} pre-generation worker panicked");
        }
        if generated != total {
            bail!("{label} pre-generation incomplete: generated {generated} of {total} chunks");
        }
        Ok(generated)
    })?;

    let elapsed = started.elapsed();
    let chunks_per_second = generated as f64 / elapsed.as_secs_f64().max(0.001);
    tracing::info!(
        generated,
        total,
        label,
        elapsed_ms = elapsed.as_millis(),
        chunks_per_second,
        "world pre-generation finished",
    );
    Ok(generated)
}

pub(crate) fn warm_spawn_window(
    storage: &mut mc_world::WorldStorage,
    view_distance: i32,
) -> Result<usize> {
    let mut warmed = 0usize;
    for pos in spawn_window_positions_at(storage.spawn().chunk(), view_distance) {
        if storage
            .get_chunk(pos)
            .with_context(|| format!("warming spawn chunk ({}, {})", pos.x, pos.z))?
            .is_some()
        {
            warmed += 1;
        }
    }
    Ok(warmed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExistingSpawnWindowPrep {
    pub(crate) warmed: usize,
    pub(crate) baked: usize,
    pub(crate) dirty: usize,
}

pub(crate) fn prepare_existing_spawn_window(
    storage: &mut mc_world::WorldStorage,
    block_light: &mc_data::block_light::BlockLightTable,
    view_distance: i32,
    worker_threads: usize,
) -> Result<ExistingSpawnWindowPrep> {
    let warmed = warm_spawn_window(storage, view_distance)?;
    let baked =
        bake_missing_spawn_window_light(storage, block_light, view_distance, worker_threads)?;
    let dirty = storage.dirty_count();
    if dirty > 0 {
        tracing::info!("Preparing world... 95% (warmed spawn window resident)");
    }
    Ok(ExistingSpawnWindowPrep {
        warmed,
        baked,
        dirty,
    })
}

pub(crate) fn bake_spawn_window_light(
    storage: &mut mc_world::WorldStorage,
    block_light: &mc_data::block_light::BlockLightTable,
    view_distance: i32,
    worker_threads: usize,
) -> Result<usize> {
    let positions = spawn_view_positions_at(storage.spawn().chunk(), view_distance);
    bake_spawn_window_light_for_positions(
        storage,
        block_light,
        view_distance,
        worker_threads,
        positions,
    )
}

pub(crate) fn bake_missing_spawn_window_light(
    storage: &mut mc_world::WorldStorage,
    block_light: &mc_data::block_light::BlockLightTable,
    view_distance: i32,
    worker_threads: usize,
) -> Result<usize> {
    let mut missing = Vec::new();
    for pos in spawn_view_positions_at(storage.spawn().chunk(), view_distance) {
        let Some(chunk) = storage.cached_chunk_snapshot(pos) else {
            bail!(
                "missing warmed spawn chunk ({}, {}) while checking baked light",
                pos.x,
                pos.z
            );
        };
        if mc_world::light::ChunkLight::from_chunk(&chunk).is_none() {
            missing.push(pos);
        }
    }
    bake_spawn_window_light_for_positions(
        storage,
        block_light,
        view_distance,
        worker_threads,
        missing,
    )
}

pub(crate) fn bake_spawn_window_light_for_positions(
    storage: &mut mc_world::WorldStorage,
    block_light: &mc_data::block_light::BlockLightTable,
    view_distance: i32,
    worker_threads: usize,
    positions: Vec<mc_world::ChunkPos>,
) -> Result<usize> {
    let total = positions.len();
    if total == 0 {
        return Ok(0);
    }
    let mut snapshots: HashMap<mc_world::ChunkPos, Arc<mc_world::Chunk>> = HashMap::new();
    for pos in spawn_window_positions_at(storage.spawn().chunk(), view_distance) {
        let Some(chunk) = storage.cached_chunk_snapshot(pos) else {
            bail!(
                "missing generated chunk ({}, {}) while baking spawn light",
                pos.x,
                pos.z
            );
        };
        snapshots.insert(pos, chunk);
    }

    let workers = worker_threads.max(1).min(total);
    let started = Instant::now();
    tracing::info!(
        chunks = total,
        workers,
        view_distance,
        "spawn-window light bake started",
    );

    let positions = Arc::new(positions);
    let snapshots = Arc::new(snapshots);
    let next = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = std::sync::mpsc::channel::<
        Result<Vec<(mc_world::ChunkPos, mc_world::light::ChunkLight)>>,
    >();
    let batch_size = 8usize.min(total);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let positions = Arc::clone(&positions);
            let snapshots = Arc::clone(&snapshots);
            let next = Arc::clone(&next);
            let tx = tx.clone();
            scope.spawn(move || {
                let mut workspace = mc_world::light::LightWorkspace::new();
                let mut batch = Vec::with_capacity(batch_size);
                loop {
                    let idx = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&pos) = positions.get(idx) else {
                        break;
                    };
                    let mut refs: [[Option<&mc_world::Chunk>; 3]; 3] = [[None; 3]; 3];
                    for dz in -1i32..=1 {
                        for dx in -1i32..=1 {
                            let neighbour = mc_world::ChunkPos {
                                x: pos.x + dx,
                                z: pos.z + dz,
                            };
                            refs[(dz + 1) as usize][(dx + 1) as usize] =
                                snapshots.get(&neighbour).map(|chunk| chunk.as_ref());
                        }
                    }
                    if refs[1][1].is_none() {
                        let _ = tx.send(Err(anyhow::anyhow!(
                            "missing centre chunk ({}, {}) while baking spawn light",
                            pos.x,
                            pos.z
                        )));
                        return;
                    }
                    let light =
                        mc_world::light::compute_chunk_light_in(&mut workspace, refs, block_light);
                    batch.push((pos, light));
                    if batch.len() >= batch_size && tx.send(Ok(std::mem::take(&mut batch))).is_err()
                    {
                        break;
                    }
                }
                if !batch.is_empty() {
                    let _ = tx.send(Ok(batch));
                }
            });
        }
        drop(tx);
        for batch in rx {
            for (pos, light) in batch? {
                if !storage.set_baked_light(pos, &light)? {
                    bail!(
                        "missing generated chunk ({}, {}) while storing baked spawn light",
                        pos.x,
                        pos.z
                    );
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    })?;

    let elapsed = started.elapsed();
    tracing::info!(
        baked = total,
        elapsed_ms = elapsed.as_millis(),
        chunks_per_second = total as f64 / elapsed.as_secs_f64().max(0.001),
        workers,
        view_distance,
        "spawn-window light bake finished",
    );
    Ok(total)
}

#[cfg(test)]
pub(crate) fn spawn_view_positions(view_distance: i32) -> Vec<mc_world::ChunkPos> {
    spawn_view_positions_at(mc_world::ChunkPos { x: 0, z: 0 }, view_distance)
}

pub(crate) fn spawn_view_positions_at(
    center: mc_world::ChunkPos,
    view_distance: i32,
) -> Vec<mc_world::ChunkPos> {
    let radius = view_distance.max(0);
    let width = radius as usize * 2 + 1;
    let mut positions = Vec::with_capacity(width * width);
    for z in -radius..=radius {
        for x in -radius..=radius {
            positions.push(mc_world::ChunkPos {
                x: center.x + x,
                z: center.z + z,
            });
        }
    }
    positions
}

#[cfg(test)]
pub(crate) fn spawn_window_positions(view_distance: i32) -> Vec<mc_world::ChunkPos> {
    spawn_window_positions_at(mc_world::ChunkPos { x: 0, z: 0 }, view_distance)
}

pub(crate) fn spawn_window_positions_at(
    center: mc_world::ChunkPos,
    view_distance: i32,
) -> Vec<mc_world::ChunkPos> {
    let radius = view_distance.max(0) + 1;
    let width = radius as usize * 2 + 1;
    let mut positions = Vec::with_capacity(width * width);
    for z in -radius..=radius {
        for x in -radius..=radius {
            positions.push(mc_world::ChunkPos {
                x: center.x + x,
                z: center.z + z,
            });
        }
    }
    positions
}

pub(crate) fn count_region_files(world_dir: &Path) -> usize {
    let mut total = 0;
    for candidate in [
        world_dir.join("dimensions/minecraft/overworld/region"),
        world_dir.join("region"),
    ] {
        if let Ok(entries) = std::fs::read_dir(&candidate) {
            for e in entries.flatten() {
                if e.path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("mca"))
                {
                    total += 1;
                }
            }
        }
    }
    total
}

pub(crate) fn ensure_world_region_root(world_dir: &Path) -> Result<()> {
    let modern = world_dir
        .join("dimensions")
        .join("minecraft")
        .join("overworld")
        .join("region");
    let legacy = world_dir.join("region");
    if modern.is_dir() || legacy.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(&legacy)
        .with_context(|| format!("creating empty world region directory {}", legacy.display()))
}
