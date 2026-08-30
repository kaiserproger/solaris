# Alpha-3 operator dashboard (P1 item 4)

Date: 2026-08-30
Base tree at session start: `09e6670a068e685e9f67595e1f776a3beaad826c` (worktree
already carried uncommitted alpha-3 agent-pool work; see commit body).

## What changed

Optional, default-off, first-party read-only HTTP dashboard:

- `crates/mc-server/src/dashboard.rs` (+ `dashboard_tests.rs`): frozen
  `StatsPayload` contract, `DashboardStats` provider trait, hand-rolled
  HTTP/1.1 server on tokio (GET-only, `/` embedded single-file dark page,
  `/stats` JSON, 405/404/400/431 paths, 8 KiB head cap, 5 s read timeout,
  `Connection: close`), no new dependencies, no unsafe.
- `crates/mc-server/src/dashboard_stats.rs`: live provider bridging
  `RuntimeTelemetryHandle`, `RuntimeControlHandle`, `OutboundPressureHandle`,
  and the new `OperatorFactsHandle`; TPS derived from the simulation-tick
  watch between polls; bounded 128-line WARN/ERROR ring wired as a second
  `tracing_subscriber` fmt layer (`LevelFilter::WARN`).
- `crates/mc-net/src/operator_metrics.rs`: process-global monotonic counters
  (chunks loaded from region / freshly generated / streamed to sockets /
  outbound framed bytes) recorded at the existing completion points in
  `chunk_stream.rs` (disk-load commit, generate commit, post-socket-write)
  and `connection.rs::write_packet`.
- `SessionRegistry` retains the latest `SaveAllReport` (written in the single
  `save_all_with_context_snapshot_locked` funnel) and the cumulative
  natural-spawn report (published at the existing 1 200-tick surfacing in
  `record_natural_spawn_report`); new `OperatorFactsHandle` exposes online
  player names, entity category counts, and both retained reports through
  brief authoritative-mutex reads at dashboard poll rate only.
- Config: `[dashboard]` section (`enabled`, `bind_address`, `port`,
  `allow_remote`), default-off loopback; non-loopback binds fail closed
  without explicit `allow_remote = true`; validated by `--check` and serve.
- Docs: `docs/OPERATING.md` dashboard section + boundaries update,
  `example.toml` section, README status paragraph.

## Data coverage

Uptime/version/brand, players (count + bounded names), TPS + tick p50/p95/p99
per stage, memory RSS/limit, autoscale limits/decisions/draining, chunk
ticketed/prepared + cumulative loaded/generated/streamed, entity totals by
category, natural-spawn cumulative metrics per category, last save report
with timings/errors, network bytes + drops/retries/sheds, plugin ids, recent
WARN/ERROR lines.

## Verification

- `cargo test -p mc-server --lib dashboard`: 17/17 (HTTP routing, JSON shape,
  HTML self-containment, config defaults/remote refusal/invalid address).
- `cargo test -p mc-net --lib`: 2016 passed / 0 failed (includes 5 new
  operator-metrics/operator-facts tests).
- `cargo test -p mc-server` (all 10 targets): 195 passed / 0 failed. This
  required migrating the wire-level preambles in `tests/configuration.rs`,
  `tests/login.rs`, `tests/play.rs` to expect the pre-existing (uncommitted)
  configuration-phase brand packet first — the earlier agent pool added the
  brand publish without updating these callers.
- `cargo clippy -p mc-net -p mc-server --all-targets`: 0 warnings.
- `cargo run -p xtask -- code-health`: verdict KEEP, 0 fail.
- Live smoke (debug build, real `mc-server` binary, temp world): server
  started with `dashboard.enabled = true`; `GET /` returned 200
  `text/html`, `GET /stats` returned the full payload with live telemetry
  (tick percentiles over 100+ samples, TPS 19.93 on the second poll,
  autoscale limits, memory), `POST /` returned 405 with `Allow: GET`,
  unknown path 404; SIGINT shutdown completed with exit code 0 (dashboard
  task aborted after the listener future, final save and Luau join intact).
- UI render/poll/error-banner behavior was browser-verified headlessly by the
  implementation subagent against the same module (screenshots recorded).

## Memory and hot-path discipline

The dashboard adds no per-tick work: counters are relaxed atomic increments
at existing completion points, retained reports are written only when the
owning authority already surfaces them, and all reads ride existing
lock-free/benign-lock snapshots at 2 s poll rate. Retained state is bounded
(128-line ring, one save report, one spawn report) — no measurable RSS delta
expected; the release benchmark matrix re-run remains the authority.

## Boundaries

Read-only surface; no ADR-owning authority, threading, or persistence
ordering change. No operator mutations are exposed.
