# Chunk-stream cancellation wait evidence

Date: 2026-07-30

Checkpoint base: `c6841d2e0bf4f6619320f0e505ac2dbcbe8e68f4`

## Wait dependency

`load_scenarios::wait_for_chunk_cancellation` repeatedly read the pipeline
cancellation counters until `cancelled_streams` advanced. Its deadline bounded
failure, but `tokio::task::yield_now()` advanced the loop, so completion
depended on scheduler polling rather than the cancellation event produced when
a chunk stream is dropped.

`ChunkPipelineResourceMetrics` now owns a cancellation `Notify` beside the
authoritative counters. `wait_for_stream_cancellation_after` creates and
enables the notification before reading the current snapshot, then awaits only
when `cancelled_streams` has not advanced. This closes both notification-before-
registration and state-before-registration races.

The publisher records `cancelled_requests` first, publishes the incremented
`cancelled_streams` counter with release ordering, and only then notifies
waiters. The snapshot reads `cancelled_streams` with acquire ordering before
reading `cancelled_requests`; observing a new stream count therefore also
observes the complete request count used by the load assertion.

The harness keeps a five-second `timeout` only as a fail watchdog and captures
a fresh snapshot for its before/after failure diagnostic. Focused unit tests
cover both a registered waiter and a cancellation recorded before waiter
registration; their one-second timeouts are also fail-only watchdogs.

Benchmark: not applicable. This changes deterministic test observation and
metric publication ordering without changing chunk-stream cancellation
semantics or a performance contract.

## Validation

- `cargo test -p mc-net cancellation_wait --lib`: 2 passed.
- `cargo test -p mc-test-harness --test load_scenarios --no-run`: passed.
- `cargo test -p mc-net`: 1,852 passed, 5 ignored; 3 doc tests passed.
- `cargo test -p mc-test-harness`: passed with the documented opt-in tests
  remaining ignored.
- `cargo run -p xtask -- code-health`: `0 fail`, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.

Independent read-only review verdict: `pass`; no findings.

The ignored sidecar-dependent replay scenario and graphical/client gate were
not run. This checkpoint changes test determinism only and makes no gameplay,
parity, performance, or release-readiness claim.
