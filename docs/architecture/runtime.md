# Actual runtime map

Status: current server after ADR 0014 Phase 5. The model is
[ADR 0013](../decisions/0013-core-model-v2.md); the summary is
[ARCHITECTURE.md](../ARCHITECTURE.md). Historical extraction paths are not
current runtime entrypoints.

## Entry points

```text
crates/mc-edge/src/bin/solaris-v2.rs
  operator_cli: parse --config (default server.toml), dispatch offline command
  server: resolve/validate content cache and immutable startup tables
  mc_core::Core::open: native world, recovery, gameplay and tick ownership
  EdgeServer: connections, authentication, fanout, Loader and dashboard state
  serve_with_console: listener + optional stdin command admission
```

Offline commands (`content import|verify`, `backup`, `restore`,
`world import-anvil|export-anvil`) finish without starting normal serving.
`--no-console` disables stdin commands; it is not a TUI/plain-input switch.
Normal serving, console `stop` and SIGINT/SIGTERM share native shutdown and
final checkpoint behavior. Tracing goes to stderr; console feedback to stdout.
The console rejects invalid UTF-8 input lines without terminating the stdin reader.
Play chat and command packets containing §, ASCII control characters, or DEL
are disconnected with the vanilla illegal-characters reason.

## Execution boundaries

| Boundary | Owner | Constraint |
| --- | --- | --- |
| Authoritative state and tick | `mc-core` | decoded input → native validation → entity intents → deterministic merge → block work → one tick change set |
| Parallel entity work | core worker pool | reads immutable state; writes disjoint entity next-state data, not shared regional state |
| Disk/journal/checkpoint I/O | native journal worker | freezes immutable/owned checkpoint images; no synchronous fsync on a gameplay reply path |
| Socket decode/write and sessions | `mc-edge` connection tasks | no independent inventory, world or entity authority |
| Per-viewer packet planning | edge fanout workers/pump | own workers, bounded connection credit, retained projection revisions |
| Component execution | `mc-plugin-host` worker | serial bounded guest callbacks outside the tick; `mc-script` owns DTO/admission |
| Loader wire | `mc-loader` | manifest/artifact/ACK/sound codecs, no gameplay state |
| Vanilla capture/tooling | `mc-vanilla` | content import and shared harness client/process helpers |
| Run/process/receipt lifecycle | `tools/harness` | v2-only server launch; one owned artifact directory and fail-closed receipt |

World fanout may skip to the newest immutable snapshot and reconcile the
viewer's held world state. Reliable per-player results remain ordered; capacity
exhaustion disconnects rather than dropping them. Socket completion and client
rendering are separate from native commit.

### Tick cost accounting

`/load-metrics` preserves the original completed `TickTimes` for each tick.
`total_us` is the pre-publication core cost: gameplay plus WAL image building,
bounded writer enqueue and generated-chunk capture. Its disjoint `phase_us`
fields cover inputs, chunk admission/hydration, entity/merge/block work,
commit, snapshot/views/publication construction, plugin admission/observations,
dimension coordination, journal building/queueing and generated saves.
`coordination_us` is the measured dimension envelope less its local phases;
it includes routing, travel, global projection, return-time scratch destruction
and sub-microsecond phase boundaries, not an inferred disk/network cost.

`owner_pre_us` and `owner_post_us` are **outside** `total_us`: acknowledgement
and input drains before gameplay, then reliable/world handoff and checkpoint
capture after it. Include them only when comparing the phase sum to `wall_us`
(the complete active iteration, excluding pacing). Microsecond rounding leaves
small gaps. The journal worker owns fsync, generation/lighting workers own
off-tick work, and edge fanout has its separate dashboard `publish` distribution;
writer backpressure paid by the tick is visible in `journal_queue_us`.

The five-headless-player local exploration profile identified synchronous
admission seam lighting as a major chunk-phase cost, with the tiny cross-crate
`LightLayer::get` accessor taking 13% of sampled tick-thread self CPU.
That accessor permits ordinary cross-crate inlining; light equations, admission
order, copy-on-write arrays and geometry checks are unchanged. Existing full
recompute/seam oracle tests remain the correctness fence. This is not a new
asynchronous lighting path: moving admission off-tick needs a separate
authoritative revision/reconciliation design. Remove the inline hint if future
profiles no longer justify it; timing receipts, including tail variance, live
under `.analysis/codex-logs/tick-budget/`.
Interleaved local A/B/A/B windows observed pooled tick p50/p99 of
7.576/22.008 ms before and 6.145/19.864 ms after the inline hint. These are
two-core-lane diagnostics with variable natural spawning and concurrent shared
host work, not an isolated causal speedup or a VPS performance guarantee.

## Persistent and external boundaries

The serving world is native `native/state.redb` plus tick WAL. Player, world,
entity and plugin effects recover at the same tick boundary; an unflushed tail
can roll back consistently. Anvil import/export is an explicit stopped-world
conversion with a new destination and receipt, not a second serving backend.

Production core loads independent compiled component packages. Neither
`../solaris-loader` nor `../solaris-default-plugins` is required to build it;
gates requiring actual sibling behavior declare that prerequisite explicitly.
There is no v1 server variant or old regional/storage actor fallback.

Use [state-ownership.md](state-ownership.md) for authority and
[mutations.md](mutations.md) for end-to-end commit/publication routes. Consult
current symbols before changing a boundary; generated indexes only navigate.
