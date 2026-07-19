use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use mc_data::block_light::BlockLightTable;
use mc_protocol::packets::play::LightData;
use mc_world::light::{
    ChunkLight, LightCache, LightWorkspace, apply_block_change_to_light, compute_chunk_light_in,
};
use mc_world::wire::encode_chunk_light;
use mc_world::{Chunk, ChunkPos, WorldStorage};
use tracing::warn;

use super::session::OutboundLightUpdate;
use super::{BlockEditBatchOutcome, block_edit_changes_light};
#[cfg(test)]
use super::{OUTBOUND_LIGHT_NEIGHBOURHOOD_CAPTURE_COUNT, OUTBOUND_LIGHT_UPDATE_ENCODING_COUNT};

#[derive(Debug)]
pub(super) struct IncrementalLightSources {
    pub(super) chunks: HashMap<ChunkPos, Option<Arc<Chunk>>>,
}

fn light_edit_centres(table: &BlockLightTable, outcome: &BlockEditBatchOutcome) -> Vec<ChunkPos> {
    let mut seen = HashSet::new();
    let mut centres = Vec::new();
    for edit in &outcome.applied {
        if !block_edit_changes_light(table, edit.previous, edit.new_state) {
            continue;
        }
        let centre = ChunkPos {
            x: edit.pos.x.div_euclid(16),
            z: edit.pos.z.div_euclid(16),
        };
        if seen.insert(centre) {
            centres.push(centre);
        }
    }
    centres
}

pub(super) fn capture_incremental_light_sources(
    storage: &WorldStorage,
    table: &BlockLightTable,
    outcome: &BlockEditBatchOutcome,
) -> IncrementalLightSources {
    let mut chunks = HashMap::new();
    for centre in light_edit_centres(table, outcome) {
        #[cfg(test)]
        OUTBOUND_LIGHT_NEIGHBOURHOOD_CAPTURE_COUNT.with(|count| count.set(count.get() + 1));
        for dz in -1..=1 {
            for dx in -1..=1 {
                let pos = ChunkPos {
                    x: centre.x + dx,
                    z: centre.z + dz,
                };
                chunks
                    .entry(pos)
                    .or_insert_with(|| storage.cached_chunk_snapshot(pos));
            }
        }
    }
    IncrementalLightSources { chunks }
}

pub(super) fn capture_incremental_light_sources_from_read_view(
    read_view: &mc_world::WorldReadView,
    table: &BlockLightTable,
    outcome: &BlockEditBatchOutcome,
) -> IncrementalLightSources {
    let mut positions = Vec::new();
    let mut seen = HashSet::new();
    for centre in light_edit_centres(table, outcome) {
        for dz in -1..=1 {
            for dx in -1..=1 {
                let position = ChunkPos {
                    x: centre.x + dx,
                    z: centre.z + dz,
                };
                if seen.insert(position) {
                    positions.push(position);
                }
            }
        }
    }
    let snapshot = read_view.snapshot_chunks(&positions);
    IncrementalLightSources {
        chunks: positions
            .into_iter()
            .map(|position| (position, snapshot.chunk(position)))
            .collect(),
    }
}

fn incremental_light_source_refs(
    sources: &IncrementalLightSources,
    centre: ChunkPos,
) -> [[Option<&Chunk>; 3]; 3] {
    std::array::from_fn(|dz| {
        std::array::from_fn(|dx| {
            let pos = ChunkPos {
                x: centre.x + dx as i32 - 1,
                z: centre.z + dz as i32 - 1,
            };
            sources.chunks.get(&pos).and_then(Option::as_deref)
        })
    })
}

pub(super) fn compute_incremental_light_updates(
    sources: &IncrementalLightSources,
    table: &BlockLightTable,
    outcome: &BlockEditBatchOutcome,
) -> Vec<OutboundLightUpdate> {
    let mut cache = LightCache::new();
    let mut fallback_workspace = LightWorkspace::new();
    let mut full_fallback_chunks = HashSet::new();
    let mut update_order = Vec::new();
    let mut update_positions = HashSet::new();

    for edit in &outcome.applied {
        if !block_edit_changes_light(table, edit.previous, edit.new_state) {
            continue;
        }
        let centre_pos = ChunkPos {
            x: edit.pos.x.div_euclid(16),
            z: edit.pos.z.div_euclid(16),
        };
        if full_fallback_chunks.contains(&(centre_pos.x, centre_pos.z)) {
            continue;
        }
        let refs = incremental_light_source_refs(sources, centre_pos);
        if refs[1][1].is_none() {
            continue;
        }

        seed_background_light_cache(
            &mut cache,
            sources,
            centre_pos,
            &outcome.previous_light_chunks,
        );
        if !cache.contains(centre_pos) {
            let light = compute_chunk_light_in(&mut fallback_workspace, refs, table);
            cache.insert(centre_pos, light);
            if update_positions.insert(centre_pos) {
                update_order.push(centre_pos);
            }
            full_fallback_chunks.insert((centre_pos.x, centre_pos.z));
            continue;
        }

        let touched = apply_block_change_to_light(
            &mut cache,
            &refs,
            table,
            centre_pos,
            edit.pos.x.rem_euclid(16) as u8,
            edit.pos.y,
            edit.pos.z.rem_euclid(16) as u8,
            edit.previous,
            edit.new_state,
        );
        for pos in touched {
            if cache.get(pos).is_none() {
                continue;
            }
            if update_positions.insert(pos) {
                update_order.push(pos);
            }
        }
    }

    update_order
        .into_iter()
        .filter_map(|pos| {
            cache
                .get(pos)
                .cloned()
                .map(|light| outbound_light_update(pos, light))
        })
        .collect()
}

pub(super) fn incremental_light_sources_are_current(
    storage: &WorldStorage,
    sources: &IncrementalLightSources,
) -> bool {
    sources.chunks.iter().all(|(pos, expected)| {
        let current = storage.cached_chunk_snapshot(*pos);
        match (expected, current) {
            (Some(expected), Some(current)) => Arc::ptr_eq(expected, &current),
            (None, None) => true,
            _ => false,
        }
    })
}

pub(super) fn collect_full_light_updates_for_current_world(
    storage: &mut WorldStorage,
    table: &BlockLightTable,
    outcome: &BlockEditBatchOutcome,
) -> Vec<OutboundLightUpdate> {
    let mut targets = Vec::new();
    let mut seen = HashSet::new();
    for centre in light_edit_centres(table, outcome) {
        for dz in -1..=1 {
            for dx in -1..=1 {
                let pos = ChunkPos {
                    x: centre.x + dx,
                    z: centre.z + dz,
                };
                if seen.insert(pos) && storage.cached_chunk_snapshot(pos).is_some() {
                    targets.push(pos);
                }
            }
        }
    }

    let mut source_chunks = HashMap::new();
    for target in &targets {
        for dz in -1..=1 {
            for dx in -1..=1 {
                let pos = ChunkPos {
                    x: target.x + dx,
                    z: target.z + dz,
                };
                source_chunks
                    .entry(pos)
                    .or_insert_with(|| storage.cached_chunk_snapshot(pos));
            }
        }
    }
    let sources = IncrementalLightSources {
        chunks: source_chunks,
    };
    let mut workspace = LightWorkspace::new();
    let updates = targets
        .into_iter()
        .map(|pos| {
            let refs = incremental_light_source_refs(&sources, pos);
            outbound_light_update(pos, compute_chunk_light_in(&mut workspace, refs, table))
        })
        .collect::<Vec<_>>();
    persist_baked_light_updates(storage, &updates);
    updates
}

pub(super) fn collect_incremental_light_updates_for_applied_edits(
    storage: &mut WorldStorage,
    table: &BlockLightTable,
    outcome: &BlockEditBatchOutcome,
) -> Vec<OutboundLightUpdate> {
    let sources = capture_incremental_light_sources(storage, table, outcome);
    let updates = compute_incremental_light_updates(&sources, table, outcome);
    persist_baked_light_updates(storage, &updates);
    updates
}

fn outbound_light_update(pos: ChunkPos, light: ChunkLight) -> OutboundLightUpdate {
    #[cfg(test)]
    OUTBOUND_LIGHT_UPDATE_ENCODING_COUNT.with(|count| count.set(count.get() + 1));
    let wire = encode_chunk_light(&light);
    OutboundLightUpdate {
        pos,
        light,
        wire: LightData {
            sky_y_mask: wire.sky_y_mask,
            block_y_mask: wire.block_y_mask,
            empty_sky_y_mask: wire.empty_sky_y_mask,
            empty_block_y_mask: wire.empty_block_y_mask,
            sky_updates: wire.sky_updates,
            block_updates: wire.block_updates,
        },
    }
}

pub(super) fn light_update_chunks(updates: &[OutboundLightUpdate]) -> HashSet<(i32, i32)> {
    updates
        .iter()
        .map(|update| (update.pos.x, update.pos.z))
        .collect()
}

fn seed_background_light_cache(
    cache: &mut LightCache,
    sources: &IncrementalLightSources,
    centre_pos: ChunkPos,
    previous_lights: &HashMap<(i32, i32), ChunkLight>,
) {
    for dz in -1i32..=1 {
        for dx in -1i32..=1 {
            let pos = ChunkPos {
                x: centre_pos.x + dx,
                z: centre_pos.z + dz,
            };
            if cache.contains(pos) {
                continue;
            }
            if let Some(light) = previous_lights.get(&(pos.x, pos.z)) {
                cache.insert(pos, light.clone());
                continue;
            }
            if let Some(chunk) = sources.chunks.get(&pos).and_then(Option::as_deref)
                && let Some(light) = ChunkLight::from_chunk(chunk)
            {
                cache.insert(pos, light);
            }
        }
    }
}

fn persist_baked_light_update(storage: &mut WorldStorage, pos: ChunkPos, light: &ChunkLight) {
    if storage.cached_chunk_snapshot(pos).is_none() {
        return;
    }
    match storage.set_baked_light(pos, light) {
        Ok(_) => {}
        Err(err) => {
            warn!(error = %err, cx = pos.x, cz = pos.z, "baked light update write failed");
        }
    }
}

pub(super) fn persist_baked_light_updates(
    storage: &mut WorldStorage,
    updates: &[OutboundLightUpdate],
) {
    for update in updates {
        persist_baked_light_update(storage, update.pos, &update.light);
    }
}
