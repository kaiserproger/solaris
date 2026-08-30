//! Process-global operator counters and retained operator-facing views.
//!
//! The optional first-party dashboard polls these values. The counters are
//! process-global monotonically increasing totals, mirroring the existing
//! lock-pressure globals: the server binary runs one authoritative server per
//! process, and in-process test servers may add to the same counters. Retained
//! reports (last save, cumulative natural spawn) are owned by the
//! [`crate::play::SessionRegistry`] instead, so parallel test servers keep
//! independent state.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

static CHUNKS_LOADED: AtomicU64 = AtomicU64::new(0);
static CHUNKS_GENERATED: AtomicU64 = AtomicU64::new(0);
static CHUNKS_STREAMED: AtomicU64 = AtomicU64::new(0);
static OUTBOUND_BYTES: AtomicU64 = AtomicU64::new(0);

/// One chunk centre was committed into storage from a region-file read.
pub(crate) fn record_chunk_loaded() {
    CHUNKS_LOADED.fetch_add(1, Ordering::Relaxed);
}

/// One freshly generated chunk centre was committed into storage.
pub(crate) fn record_chunk_generated() {
    CHUNKS_GENERATED.fetch_add(1, Ordering::Relaxed);
}

/// One chunk packet was written to a client socket.
pub(crate) fn record_chunk_streamed() {
    CHUNKS_STREAMED.fetch_add(1, Ordering::Relaxed);
}

/// Outbound framed bytes were written to a client socket.
pub(crate) fn record_outbound_bytes(bytes: u64) {
    OUTBOUND_BYTES.fetch_add(bytes, Ordering::Relaxed);
}

/// Cumulative operator counters for one process.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OperatorCounterSnapshot {
    pub chunks_loaded: u64,
    pub chunks_generated: u64,
    pub chunks_streamed: u64,
    pub outbound_bytes_written: u64,
}

/// Read the current process-global operator counters.
#[must_use]
pub fn operator_counter_snapshot() -> OperatorCounterSnapshot {
    OperatorCounterSnapshot {
        chunks_loaded: CHUNKS_LOADED.load(Ordering::Relaxed),
        chunks_generated: CHUNKS_GENERATED.load(Ordering::Relaxed),
        chunks_streamed: CHUNKS_STREAMED.load(Ordering::Relaxed),
        outbound_bytes_written: OUTBOUND_BYTES.load(Ordering::Relaxed),
    }
}

/// The most recent save report plus when it completed.
#[derive(Debug, Clone)]
pub struct RetainedSaveReport {
    pub at: Instant,
    pub report: crate::server::SaveAllReport,
}

/// The most recent cumulative natural-spawn report plus when it surfaced.
#[derive(Debug, Clone, Copy)]
pub struct RetainedNaturalSpawnReport {
    pub at: Instant,
    pub tick: u64,
    pub report: mc_entity::natural_spawn_26_1_2::NaturalSpawnReport,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_counters_are_monotonic_under_recording() {
        // Parallel tests in this binary may record into the same globals, so
        // only monotonic growth and inclusion of this test's own records are
        // observable.
        let before = operator_counter_snapshot();
        record_chunk_loaded();
        record_chunk_generated();
        record_chunk_generated();
        record_chunk_streamed();
        record_outbound_bytes(512);
        let after = operator_counter_snapshot();
        assert!(after.chunks_loaded.saturating_sub(before.chunks_loaded) >= 1);
        assert!(
            after
                .chunks_generated
                .saturating_sub(before.chunks_generated)
                >= 2
        );
        assert!(after.chunks_streamed.saturating_sub(before.chunks_streamed) >= 1);
        assert!(
            after
                .outbound_bytes_written
                .saturating_sub(before.outbound_bytes_written)
                >= 512
        );
    }
}
