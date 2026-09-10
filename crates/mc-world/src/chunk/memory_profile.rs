//! On-demand accounting; never called by the tick or admission hot paths.
use std::collections::{BTreeMap, HashSet};

use super::{BiomeSection, Chunk, ChunkSection, SectionLight, Tag, tag_heap_bytes};

/// Allocation-capacity estimates, deduplicated within the observed chunk set.
/// Not allocator usable size or RSS; external old snapshots are outside this set.
#[derive(Debug, Default, Clone)]
pub struct ChunkMemoryProfile {
    pub chunks: usize,
    pub dirty_chunks: usize,
    pub reachable_bytes: usize,
    pub unique_allocated_bytes: usize,
    pub shared_bytes_deduplicated: usize,
    pub categories: BTreeMap<String, usize>,
    pub section_bits: BTreeMap<u8, usize>,
    pub light_unknown_layers: usize,
    pub light_uniform_layers: usize,
    pub light_shared_layers: usize,
    seen_blocks: HashSet<usize>,
    seen_lights: HashSet<usize>,
}

impl ChunkMemoryProfile {
    fn add(&mut self, name: &str, bytes: usize) {
        *self.categories.entry(name.to_owned()).or_default() += bytes;
        self.unique_allocated_bytes += bytes;
    }

    pub(crate) fn observe(&mut self, chunk: &Chunk) {
        self.chunks += 1;
        self.dirty_chunks += usize::from(chunk.dirty);
        let reachable = chunk.estimated_heap_bytes();
        self.reachable_bytes += reachable;
        let mut classified = 0;
        let object = std::mem::size_of::<Chunk>();
        self.add("chunk_structs", object);
        classified += object;
        let section_vectors = chunk.sections.capacity() * std::mem::size_of::<ChunkSection>();
        self.add("block_section_headers", section_vectors);
        classified += section_vectors;
        for section in &chunk.sections {
            *self
                .section_bits
                .entry(section.indices().map_or(0, |p| p.bits_per_entry()))
                .or_default() += 1;
            if let Some((identity, _, bytes)) = section.shared_heap_allocation() {
                classified += bytes;
                if self.seen_blocks.insert(identity) {
                    self.add("block_palettes_and_indices", bytes);
                } else {
                    self.shared_bytes_deduplicated += bytes;
                }
            }
        }
        let biomes = chunk.biomes.capacity() * std::mem::size_of::<BiomeSection>()
            + chunk
                .biomes
                .iter()
                .map(BiomeSection::estimated_heap_bytes)
                .sum::<usize>();
        self.add("biomes", biomes);
        classified += biomes;
        let heights = chunk.highest_opaque.estimated_heap_bytes()
            + chunk.heightmaps.capacity() * std::mem::size_of::<(String, super::Heightmap)>()
            + chunk
                .heightmaps
                .iter()
                .map(|(name, map)| name.capacity() + map.estimated_heap_bytes())
                .sum::<usize>();
        self.add("heightmaps", heights);
        classified += heights;
        let light_headers = chunk.section_lights.capacity() * std::mem::size_of::<SectionLight>();
        self.add("light_headers", light_headers);
        classified += light_headers;
        for light in &chunk.section_lights {
            for layer in [&light.sky, &light.block] {
                let Some(layer) = layer else {
                    self.light_unknown_layers += 1;
                    continue;
                };
                let bytes = layer.allocated_bytes();
                classified += bytes;
                if let Some((identity, _)) = layer.shared_allocation() {
                    self.light_shared_layers += 1;
                    if self.seen_lights.insert(identity as usize) {
                        self.add("light_arrays", bytes);
                    } else {
                        self.shared_bytes_deduplicated += bytes;
                    }
                } else {
                    self.light_uniform_layers += 1;
                }
            }
        }
        let extras = chunk.extras.capacity() * std::mem::size_of::<(String, Tag)>()
            + chunk
                .extras
                .iter()
                .map(|(name, tag)| name.capacity() + tag_heap_bytes(tag))
                .sum::<usize>();
        self.add("preserved_nbt", extras);
        classified += extras;
        // Includes typed/raw block entities, scheduled ticks, mutation versions,
        // strings and container contents already traversed by admission accounting.
        self.add(
            "block_entities_ticks_and_other",
            reachable.saturating_sub(classified),
        );
    }
}

#[cfg(test)]
#[path = "memory_profile_tests.rs"]
mod tests;
