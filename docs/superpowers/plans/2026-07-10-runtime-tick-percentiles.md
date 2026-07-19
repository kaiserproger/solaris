# Runtime Tick Percentiles Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add bounded p50/p95/p99/max runtime tick-stage measurements without
changing simulation or autoscale decisions.

**Architecture:** A private `mc-net` module owns a fixed-capacity deque of
complete tick samples. Snapshot creation sorts one small value vector per stage
at the existing periodic log boundary. The server ticker records every tick and
emits a separate tracing event; current slow-tick reporting and control-plane
inputs remain unchanged.

**Tech Stack:** Rust 1.94 standard library, existing `tracing`, `mc-net` unit
tests.

## Global Constraints

- Capacity is 1200 samples, approximately one minute at 20 TPS.
- Percentiles use documented nearest-rank semantics.
- Empty windows return no snapshot; zero-duration stage samples remain valid.
- Recording is O(1), bounded, allocation-free after deque growth.
- Solaris code remains unsafe-free and adds no dependency.
- Do not feed percentile values into autoscale in this slice.

---

### Task 1: Bounded Percentile Model

**Files:**
- Create: `crates/mc-net/src/runtime_tick_metrics.rs`
- Modify: `crates/mc-net/src/lib.rs`

**Interfaces:**
- Produces: `RuntimeTickSample`, `RuntimeLatencyPercentiles`,
  `RuntimeTickPercentiles`, and `RuntimeTickMetricsWindow` for crate-private
  server use.

- [x] **Step 1: Write RED tests**

Tests must require:

- nearest-rank p50/p95/p99/max for a known 1..=100 distribution;
- bounded eviction of the oldest sample;
- independent stage values in one snapshot;
- no snapshot before the first sample.

- [x] **Step 2: Verify RED**

```sh
cargo test -p mc-net --lib runtime_tick_metrics -- --nocapture
```

Expected: compilation fails because the module/types do not yet exist.

- [x] **Step 3: Implement the minimal model**

Add a 1200-capacity default, normalized nonzero test capacity, O(1) record,
nearest-rank snapshot computation, and the exact stage fields measured by the
server ticker.

- [x] **Step 4: Verify GREEN**

```sh
cargo test -p mc-net --lib runtime_tick_metrics -- --nocapture
```

Expected: all new model tests pass.

### Task 2: Server Tick Integration

**Files:**
- Modify: `crates/mc-net/src/server.rs`
- Test: `crates/mc-net/src/server.rs`

**Interfaces:**
- Consumes: stage durations already measured in the entity/server ticker.
- Produces: periodic `runtime tick percentile window` tracing fields.

- [x] **Step 1: Record complete samples**

Create one window before the tick loop and record total tick, world-time,
entity-goal, entity-physics, entity-dispatch, campfire, entity-save, random-tick,
scheduled-block, and scheduled-fluid durations after each tick.

- [x] **Step 2: Emit the periodic snapshot**

At `log_interval_ticks`, emit sample count, capacity, and p50/p95/p99/max for
every measured stage. Keep the existing per-tick debug/slow-warning events.

- [x] **Step 3: Verify focused and full paths**

```sh
cargo fmt --all -- --check
cargo test -p mc-net --lib runtime_tick_metrics -- --nocapture
cargo test -p mc-net --lib server::tests -- --nocapture
cargo run -p xtask -- code-health
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

### Task 3: Evidence And Diff Review

**Files:**
- Modify: `docs/VALIDATION_LEDGER.md`
- Modify: `docs/milestones/M91.md`

**Interfaces:**
- Produces: an exact measurement-capability claim, not a performance pass.

- [x] **Step 1: Record scope and limitations**

Document the bounded percentile surface and tests. State that no profile run,
optimization, autoscale-policy change, real-client run, or soak was performed.

- [x] **Step 2: Review and verify**

```sh
git diff --check
git status --short --branch
```

Inspect all changed lines in the new module, server integration, module wiring,
and evidence docs. Do not stage unrelated dirty-worktree paths.
