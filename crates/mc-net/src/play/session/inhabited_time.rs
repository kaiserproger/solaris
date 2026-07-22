use super::*;

const SPAWNING_RANGE_SQUARED: f64 = 128.0 * 128.0;

impl SessionRegistry {
    pub(crate) fn spawning_chunks_sorted(&self) -> Vec<(i32, i32)> {
        let inner = self.lock_inner("snapshot spawning chunks");
        let mut chunks = inner
            .loaded_chunk_refcounts
            .keys()
            .copied()
            .filter(|&(chunk_x, chunk_z)| {
                let center_x = f64::from(chunk_x) * 16.0 + 8.0;
                let center_z = f64::from(chunk_z) * 16.0 + 8.0;
                inner.sessions.iter().any(|(id, session)| {
                    if inner.spectator_sessions.contains(id) {
                        return false;
                    }
                    let dx = center_x - session.pose.x;
                    let dz = center_z - session.pose.z;
                    dx * dx + dz * dz < SPAWNING_RANGE_SQUARED
                })
            })
            .collect::<Vec<_>>();
        chunks.sort_unstable_by_key(|&(x, z)| (z, x));
        chunks
    }
}
