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

pub(super) async fn spawn_position(config: &ServerConfig) -> (f64, f64, f64) {
    let y = adaptive_spawn_y(config).await.unwrap_or(DEFAULT_SPAWN_Y);
    (SPAWN_X, y, SPAWN_Z)
}

async fn adaptive_spawn_y(config: &ServerConfig) -> Option<f64> {
    let world = config.world.as_ref()?;
    let mut storage = world.lock().await;
    let chunk = storage.get_chunk_mut(ChunkPos { x: 0, z: 0 }).ok()??;
    spawn_y_from_chunk(chunk, config.block_light.as_deref())
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
