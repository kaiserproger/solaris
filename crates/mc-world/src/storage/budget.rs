use crate::chunk::{Chunk, ChunkPos};

use super::{WorldError, WorldStorage};

impl WorldStorage {
    pub fn set_chunk_byte_budgets(
        &mut self,
        resident_budget: usize,
        dirty_budget: usize,
    ) -> Result<(), WorldError> {
        if resident_budget == 0 || dirty_budget == 0 || dirty_budget > resident_budget {
            return Err(WorldError::InvalidChunkCacheBudgets {
                resident_budget,
                dirty_budget,
            });
        }
        let (resident_bytes, dirty_bytes) = self.chunk_byte_usage();
        if resident_bytes > resident_budget || dirty_bytes > dirty_budget {
            return Err(self.chunk_cache_pressure(0, resident_bytes, dirty_bytes));
        }
        self.resident_byte_budget = resident_budget;
        self.dirty_byte_budget = dirty_budget;
        Ok(())
    }

    pub(crate) fn chunk_byte_usage(&self) -> (usize, usize) {
        let mut resident_bytes = 0usize;
        let mut dirty_bytes = 0usize;
        for (_, chunk) in self.resident.snapshots() {
            let bytes = chunk.estimated_heap_bytes();
            resident_bytes = resident_bytes.saturating_add(bytes);
            if chunk.dirty {
                dirty_bytes = dirty_bytes.saturating_add(bytes);
            }
        }
        (resident_bytes, dirty_bytes)
    }

    fn chunk_cache_pressure(
        &self,
        requested_bytes: usize,
        resident_bytes: usize,
        dirty_bytes: usize,
    ) -> WorldError {
        WorldError::ChunkCachePressure {
            requested_bytes,
            resident_bytes,
            resident_budget: self.resident_byte_budget,
            dirty_bytes,
            dirty_budget: self.dirty_byte_budget,
            save_healthy: self.save_healthy,
        }
    }

    pub(crate) fn prepare_new_chunk_admission(
        &mut self,
        cpos: ChunkPos,
        chunk: &Chunk,
    ) -> Result<(), WorldError> {
        if self.resident.contains(cpos) {
            return Ok(());
        }
        let requested_bytes = chunk.estimated_heap_bytes();
        loop {
            let (resident_bytes, dirty_bytes) = self.chunk_byte_usage();
            let count_ok = self.resident.len() < self.capacity;
            let resident_ok =
                resident_bytes.saturating_add(requested_bytes) <= self.resident_byte_budget;
            let dirty_ok = !chunk.dirty
                || dirty_bytes.saturating_add(requested_bytes) <= self.dirty_byte_budget;
            let health_ok = self.save_healthy || dirty_bytes == 0;
            if count_ok && resident_ok && dirty_ok && health_ok {
                return Ok(());
            }
            if (count_ok && resident_ok) || !self.evict_clean_chunk() {
                return Err(self.chunk_cache_pressure(
                    requested_bytes,
                    resident_bytes,
                    dirty_bytes,
                ));
            }
        }
    }

    pub(crate) fn mark_save_healthy(&mut self) {
        self.save_healthy = true;
    }

    pub(crate) fn mark_save_unhealthy(&mut self) {
        self.save_healthy = false;
    }
}

#[cfg(test)]
#[path = "budget_tests.rs"]
mod tests;
