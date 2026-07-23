use super::chunk_stream::passable_block_name;
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
    safe_spawn_position(config, world_read).unwrap_or_else(|| {
        let y = adaptive_spawn_y(config, world_read).unwrap_or(DEFAULT_SPAWN_Y);
        (SPAWN_X, y, SPAWN_Z)
    })
}

const SPAWN_SEARCH_RADIUS_BLOCKS: i32 = 80;
const SPAWN_SEARCH_RADIUS_CHUNKS: i32 = SPAWN_SEARCH_RADIUS_BLOCKS / 16;

fn safe_spawn_position(
    config: &ServerConfig,
    world_read: Option<&mc_world::WorldReadView>,
) -> Option<(f64, f64, f64)> {
    let world_read = world_read?;
    let chunk_radius = SPAWN_SEARCH_RADIUS_CHUNKS;
    let mut positions = Vec::with_capacity(((chunk_radius * 2 + 1).pow(2)) as usize);
    for chunk_z in -chunk_radius..=chunk_radius {
        for chunk_x in -chunk_radius..=chunk_radius {
            positions.push(ChunkPos {
                x: chunk_x,
                z: chunk_z,
            });
        }
    }
    let snapshot = world_read.snapshot_chunks(&positions);
    let mut best: Option<(i64, i32, i32, i32)> = None;

    for z in -SPAWN_SEARCH_RADIUS_BLOCKS..=SPAWN_SEARCH_RADIUS_BLOCKS {
        for x in -SPAWN_SEARCH_RADIUS_BLOCKS..=SPAWN_SEARCH_RADIUS_BLOCKS {
            let distance_squared = i64::from(x) * i64::from(x) + i64::from(z) * i64::from(z);
            if best.is_some_and(|(best_distance, ..)| best_distance <= distance_squared) {
                continue;
            }
            let Some(y) = safe_spawn_y(config, &snapshot, x, z) else {
                continue;
            };
            best = Some((distance_squared, x, y, z));
        }
    }

    best.map(|(_, x, y, z)| (f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5))
}

fn safe_spawn_y(
    config: &ServerConfig,
    snapshot: &mc_world::WorldReadSnapshot,
    x: i32,
    z: i32,
) -> Option<i32> {
    let chunk_pos = ChunkPos {
        x: x.div_euclid(16),
        z: z.div_euclid(16),
    };
    let chunk = snapshot.chunk(chunk_pos)?;
    let local_x = x.rem_euclid(16) as u8;
    let local_z = z.rem_euclid(16) as u8;
    let top = chunk.highest_opaque_y(local_x, local_z)?;
    let spawn_y = top.checked_add(2)?;

    let support = chunk.get_block(local_x, top, local_z)?;
    if !safe_spawn_support(config, support) {
        return None;
    }
    for y in [top.checked_add(1)?, spawn_y, spawn_y.checked_add(1)?] {
        let state = chunk.get_block(local_x, y, local_z)?;
        if !clear_spawn_body_cell(config, state) {
            return None;
        }
    }
    Some(spawn_y)
}

fn safe_spawn_support(config: &ServerConfig, state_id: mc_world::BlockStateId) -> bool {
    if config.block_facts.fluid(state_id.0).is_some() {
        return false;
    }
    let Some(state) = config.blocks.by_id(state_id) else {
        return false;
    };
    let path = state.block.id.path();
    if path.ends_with("_leaves") || hazardous_spawn_block(path) {
        return false;
    }
    !passable_block_name(state.block.id.as_str())
}

fn clear_spawn_body_cell(config: &ServerConfig, state_id: mc_world::BlockStateId) -> bool {
    if config.block_facts.fluid(state_id.0).is_some() {
        return false;
    }
    let Some(state) = config.blocks.by_id(state_id) else {
        return false;
    };
    if hazardous_spawn_block(state.block.id.path()) {
        return false;
    }
    passable_block_name(state.block.id.as_str())
}

fn hazardous_spawn_block(path: &str) -> bool {
    matches!(
        path,
        "cactus"
            | "campfire"
            | "fire"
            | "magma_block"
            | "powder_snow"
            | "soul_campfire"
            | "soul_fire"
            | "sweet_berry_bush"
    )
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
