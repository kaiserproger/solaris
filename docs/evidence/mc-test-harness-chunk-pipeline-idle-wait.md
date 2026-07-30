# Chunk-pipeline idle wait evidence

Date: 2026-07-30

Checkpoint base: `29af45e2fac3822d85ff1ab6eadb3981eb669de6`

## Wait dependency

`block_edit::container_support::wait_for_chunk_pipeline_idle` repeatedly read
the public CPU/IO metrics snapshot until both counters reached zero. Its
deadline was a failure bound, but `tokio::task::yield_now()` advanced the loop,
so completion depended on scheduler polling rather than the pipeline's own
state transition. The helper also could not observe active prepare tasks or
prepare requests.

The production pipeline already owns an exact `Notify`-backed
`ChunkPipelineResources::wait_for_idle` barrier. It registers the notification
before checking active IO, CPU, prepare-task, and prepare-request state, and
each final permit/task drop wakes the waiter. A narrow cloneable
`ChunkPipelineIdleHandle` now lets an integration harness retain that existing
barrier before `BoundServer::serve` consumes the server. It does not expose
resource mutation or introduce a second idle authority.

The container helper awaits that handle directly. Its five-second `timeout`
remains only a fail watchdog around the exact event-driven future and is not
used to advance state.

Benchmark: not applicable. This changes test orchestration and exposes an
existing diagnostic wait; it does not change production pipeline scheduling or
a performance contract.

## Validation

- `cargo test -p mc-net chunk_pipeline`: 17 passed.
- `cargo test -p mc-test-harness --test block_edit
  embedded_playable_short_session_soak_keeps_clients_responsive -- --exact`:
  passed.
- `cargo test -p mc-net`: 1,850 passed, 5 ignored; 3 doc tests passed.
- `cargo test -p mc-test-harness`: passed with the documented opt-in tests
  remaining ignored.
- `cargo run -p xtask -- code-health`: `0 fail`, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.

Independent read-only review verdict: `pass`; no findings.

The graphical/client gate was not run. This checkpoint changes deterministic
test orchestration only and makes no gameplay, parity, performance, or release
readiness claim.
