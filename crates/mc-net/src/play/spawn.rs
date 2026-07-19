use super::*;

/// Pack `(x, y, z)` into vanilla's `BlockPos` `i64` representation.
pub(super) fn pack_block_pos(x: i32, y: i32, z: i32) -> i64 {
    (((x as i64) & 0x3FF_FFFF) << 38) | (((z as i64) & 0x3FF_FFFF) << 12) | ((y as i64) & 0xFFF)
}

/// Pick the dimension that the player will spawn into. Solaris is currently
/// an overworld-only server, so prefer `minecraft:overworld` when present and
/// keep the old first-entry fallback for test stubs and degraded data.
pub(super) fn spawn_dimension(data: &VanillaData) -> Option<(i32, &Identifier, &[Identifier])> {
    let registry = data.registry("dimension_type")?;
    if registry.entries.is_empty() {
        return None;
    }
    let index = registry
        .entries
        .iter()
        .position(|entry| entry.as_str() == "minecraft:overworld")
        .unwrap_or(0);
    let id = i32::try_from(index).ok()?;
    Some((id, &registry.entries[index], registry.entries.as_slice()))
}

pub(super) fn spawn_position(
    config: &ServerConfig,
    world_read: Option<&mc_world::WorldReadView>,
) -> (f64, f64, f64) {
    let y = adaptive_spawn_y(config, world_read).unwrap_or(DEFAULT_SPAWN_Y);
    (SPAWN_X, y, SPAWN_Z)
}

fn adaptive_spawn_y(
    config: &ServerConfig,
    world_read: Option<&mc_world::WorldReadView>,
) -> Option<f64> {
    let chunk = world_read?
        .snapshot_chunks(&[ChunkPos { x: 0, z: 0 }])
        .chunk(ChunkPos { x: 0, z: 0 })?;
    if let Some(top) = chunk.highest_opaque_y(0, 0) {
        return Some((top + 2) as f64);
    }
    let mut chunk = chunk.as_ref().clone();
    spawn_y_from_chunk(&mut chunk, config.block_light.as_deref())
}

pub(crate) async fn prepare_spawn_chunk(
    config: &ServerConfig,
    resources: ChunkPipelineResources,
) -> Result<(), String> {
    let Some(world) = config.world.as_ref() else {
        return Ok(());
    };
    let position = ChunkPos { x: 0, z: 0 };
    let (plan, generator) = {
        let storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::WorldStorage,
            "spawn chunk plan",
            Instant::now(),
            world.lock().await,
        );
        (
            storage.plan_chunk_snapshot_without_generation(position),
            storage.generator(),
        )
    };

    let chunk = match plan {
        mc_world::ChunkSnapshotPlan::Cached(_) => return Ok(()),
        mc_world::ChunkSnapshotPlan::Load(plan) => {
            let permit = resources
                .acquire_io()
                .await
                .map_err(|_| "chunk IO worker pool closed".to_string())?;
            let loaded = crate::blocking::spawn_result_blocking(move || {
                let _permit = permit;
                plan.load()
            })
            .await?;
            match loaded {
                Some(chunk) => chunk,
                None => {
                    let Some(generator) = generator else {
                        return Ok(());
                    };
                    let permit = resources
                        .acquire_cpu()
                        .await
                        .map_err(|_| "chunk CPU worker pool closed".to_string())?;
                    tokio::task::spawn_blocking(move || {
                        let _permit = permit;
                        generator.generate(position)
                    })
                    .await
                    .map_err(|error| error.to_string())?
                }
            }
        }
    };

    let mut storage = crate::lock_metrics::timed_guard(
        crate::lock_metrics::LockMetricKind::WorldStorage,
        "spawn chunk commit",
        Instant::now(),
        world.lock().await,
    );
    storage
        .commit_chunk_snapshot(position, chunk)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub(super) fn spawn_y_from_chunk(
    chunk: &mut Chunk,
    table: Option<&BlockLightTable>,
) -> Option<f64> {
    if let Some(top) = chunk.highest_opaque_y(0, 0) {
        return Some((top + 2) as f64);
    }
    let table = table?;
    chunk.rebuild_highest_opaque(table);
    let top = chunk.highest_opaque_y(0, 0)?;
    Some((top + 2) as f64)
}

/// `(chunk_x, chunk_z)` for the constant spawn point. Implemented as a
/// fn rather than inlined so the math is unit-testable and so M3.e can
/// share the formula when it computes the view-distance ring.
#[cfg(test)]
pub(super) fn spawn_chunk_pos() -> (i32, i32) {
    chunk_pos_from_coords(SPAWN_X, SPAWN_Z)
}

pub(super) fn chunk_pos_from_coords(x: f64, z: f64) -> (i32, i32) {
    (
        (x.floor() as i32).div_euclid(16),
        (z.floor() as i32).div_euclid(16),
    )
}
