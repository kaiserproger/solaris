# M52 operator performance notes

Date: 2026-05-29
Branch: `dev/M52-multicore-gates-extension-boundaries`

These notes are for operators and developers reading Solaris debug-build load
gates. M52 is not a benchmark milestone. The gates are coarse regression checks
that make lock pressure, queue pressure, and worker-permit leaks visible before
future gameplay or extension work builds on them.

## Debug-build load gates

Use debug builds for development and manual gates:

```sh
cargo test -p mc-test-harness --test load_scenarios -- --nocapture
cargo run --bin mc-server -- --config example.toml
```

The non-ignored M52 load tests in
`crates/mc-test-harness/tests/load_scenarios.rs` are:

- `multicore_login_chunk_stream_and_broadcast_stays_within_budgets`
  - Runs on a 4-worker Tokio runtime.
  - Connects 4 clients, waits for chunk streaming, runs a small entity-broadcast
    path, and asserts a coarse debug elapsed budget.
  - Asserts chunk IO/CPU active permit peaks never exceed configured worker
    counts.
  - Asserts `session_registry` and `world_storage` max lock holds stay under the
    current coarse M52 ceiling of 250 ms.
- `paused_reader_does_not_stall_active_entity_broadcasts`
  - Connects one paused reader and one active reader.
  - Produces enough reliable/coalesced outbound work to raise observable pressure.
  - Requires the active reader to keep receiving gameplay entity broadcasts.

The ignored M37 report remains diagnostic-only and can still be run manually:

```sh
cargo test -p mc-test-harness --test load_scenarios -- --ignored --nocapture
```

Treat the printed elapsed times as local context, not release-quality throughput
numbers. A failure means "investigate a regression or an overly tight debug gate,"
not "the server has a production capacity limit."

## Reading lock metrics

M52 lock metrics come from `mc_net::lock_pressure_snapshot()` and runtime tick
logs. The main lock groups are:

- `session_registry`: session/player/entity visibility and dispatch planning.
- `world_storage`: cached world/chunk reads and writes.
- `save_all_flush`: save-flush coordination.

Each lock snapshot reports:

- `wait_count`: number of observed waits before acquiring the lock.
- `wait_us`: cumulative wait time in microseconds.
- `max_wait_us`: largest single wait in microseconds.
- `hold_count`: number of observed lock holds.
- `hold_us`: cumulative hold time in microseconds.
- `max_hold_us`: largest single hold in microseconds.

How to read them:

- `max_hold_us` is the first field to check for a stalled critical section. M52's
  harness gate currently budgets `session_registry.max_hold_us` and
  `world_storage.max_hold_us` at 250,000 us in debug builds.
- High `wait_us` with low `max_hold_us` usually means many small contenders; look
  for churn or fan-out rather than one deadlock.
- High `max_wait_us` with modest `hold_us` usually means a single unlucky task or
  scheduler pause; reproduce before widening a gate.
- `hold_count` and `wait_count` are denominators. Compare totals only between
  similar scenarios and similar client/entity counts.

Runtime tick logs include lock fields such as
`world_lock_max_wait_us`, `world_lock_hold_us`, `world_lock_max_hold_us`,
`session_lock_waits`, `session_lock_wait_us`, `session_lock_max_wait_us`,
`session_lock_hold_us`, and `session_lock_max_hold_us`. The harness prints the
same pressure at the end of focused scenarios so failures can be read without
scraping full server logs.

## Reading queue and worker metrics

Outbound/session pressure is exposed through `OutboundPressureSnapshot` and
runtime tick log fields:

- `visibility_command_drops`: coalescible visibility updates dropped because the
  outbound lane was full. Occasional increases under a paused reader are expected;
  active readers must still make progress.
- `reliable_command_retries`: total reliable commands that had to wait for a full
  outbound lane.
- `reliable_command_retries_in_flight`: reliable retry tasks currently retained by
  backpressure. This should drain after the receiver catches up or disconnects.
  Persistent growth points at a missing cap/disconnect policy rather than a world
  lock deadlock.

Chunk worker pressure is exposed by `ChunkPipelineResourceSnapshot`:

- `active_io` / `max_io_active`: current and peak chunk IO permits.
- `active_cpu` / `max_cpu_active`: current and peak chunk CPU permits.

`max_io_active` must not exceed configured chunk IO threads, and
`max_cpu_active` must not exceed configured chunk worker threads. In the M52 load
harness, the debug gate also requires `max_cpu_active > 0` so the scenario proves
it exercised CPU chunk preparation.

Frame-volume pressure in latency-sensitive harness tests is reported as skipped
frames/bytes while waiting for a target packet. A cap failure means the target
packet still arrived too late in the stream, even if the test eventually observed
it.

## Expected limits

Current M52 debug expectations are intentionally coarse:

- Four-client login/chunk streaming plus a small broadcast workload should finish
  under the 30 second debug elapsed gate on the development profile.
- `session_registry.max_hold_us` and `world_storage.max_hold_us` should stay below
  250 ms in the focused M52 load gate.
- Chunk IO/CPU peak permit counts should never exceed their configured semaphore
  limits.
- A paused or slow reader may cause visible drops/retries, but it must not prevent
  an active client from receiving entity broadcasts.
- Metrics are cumulative for the process. Compare before/after snapshots for a
  specific scenario when investigating a live server.

Do not convert these values into production sizing claims. They are regression
budgets for debug builds and should be widened only with a clear reproduction,
updated notes, and a replacement signal that still catches stalls.

## Extension and script performance non-goals

M52.d and M52.e define safe boundaries, not fast plugin execution:

- `mc-extension` only provides immutable inbound event DTOs, bounded event and
  command queues, and custom payload allow-list/size policy. It does not run a
  plugin host, parallel scheduler, or async plugin runtime.
- `mc-script` only defines immutable script events, bounded command batches, and
  reserved controls for fuel, memory, timeout, and shutdown. It does not embed
  Lua, WASM, or any VM.
- Neither boundary exposes `WorldHandle`, `SessionRegistry`, `WorldStorage`, or
  other lock-owning internals to extension/script code.
- Queue-full behavior is a safety signal. M52 does not guarantee zero drops,
  automatic scaling, per-plugin fairness, or production-grade slow-consumer
  isolation.
- Custom payload forwarding is policy-bounded. Unknown channels and oversized
  bodies are rejected before extension dispatch; M52 does not promise arbitrary
  mod payload throughput.

Future plugin/runtime milestones must add their own performance gates before
claiming plugin throughput or script execution latency.
