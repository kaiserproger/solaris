# NEXT_SESSION - Solaris Agent Start Prompt

Use this for the next Claude / opencode session unless the owner gives a
more specific milestone prompt.

## 1. Read First

1. `AGENTS.md`
2. `docs/DEFINITION_OF_DONE.md`
3. `docs/PROJECT_SPEC.md` §9-10
4. `docs/CORE_M77_M100_ROADMAP.md` for M77-M100 core work
5. Latest `docs/milestones/M*.md` closeout and the active milestone plan
6. `docs/REPLACEMENT_READINESS.md` when touching vanilla/client-visible behavior
7. Relevant ADRs in `docs/decisions/`

## 2. Autonomous Preflight

Before code or claims, run the preflight from `docs/DEFINITION_OF_DONE.md`:

```sh
pwd
git rev-parse --show-toplevel
git status --short --branch
rustc --version
cargo --version
cargo fmt --version
java --version
javap -version
test -f .analysis/server.jar
test -d data/vanilla/reports || test -d data/vanilla
```

Report it as `full`, `degraded`, or `blocked`. If degraded, say which
gate becomes weaker.

## 3. Current Quality Mode And Default Mission

Default mode is `stabilization`. The owner now wants a larger autonomous push
toward vanilla-level gameplay plus stabilization, not another tiny cleanup-only
pass. That does not relax the DoD: broad gameplay can be implemented, but rows
remain `partial`, `draft debt`, or `stabilization` until runtime tests plus
vanilla oracle or real-client evidence exist for the exact mechanic.

Use `https://kaiserproger.github.io/minecraft-how-it-works/` as required
background reading for core gameplay. Start with:

1. `server-how-it-works-book/11-core-mechanics-step-by-step.md`
2. `server-how-it-works-book/05-world-simulation.md`
3. `server-how-it-works-book/06-entities-items-and-progression.md`
4. `server-how-it-works-book/07-world-generation.md`
5. `version-research/26.1.2/report.md`

Use the handbook for architecture and vanilla ownership chains. Do not use it as
a substitute for local protocol/data/oracle proof. Packet IDs and layouts still
come from `javap`, `.analysis/protocol-dump.txt`, or `wire-probe`; gameplay
parity claims still need vanilla captures, decompiled-source inspection within
ADR limits, or real-client evidence.

## 4. Large Next Slice

Treat the next autonomous session as a multi-lane M100 stabilization push. The
goal is to move several high-value mechanics from "implemented locally" toward
"vanilla-observable and operationally stable" in one coherent pass.

### Lane A - Vanilla Gameplay Vertical Slices

Pick at least two adjacent gameplay rows from the ledger and drive them through
normal runtime paths, not helper-only tests. Good first targets:

- B1/B3/B4: block break/place/use rejection, bucket/fluid placement, scheduled
  fluid ticks, water/lava replacement, and resync after rejected or stale edits.
- I1/I2/K1/K2: crafting, recipe execution, recipe-book/window sync, furnace
  family and common station safe rejection vs implemented behavior.
- G1-G4/N1: damage sources, shields/equipment durability, projectiles, death,
  drops/XP, hostile targeting/pathing, and entity persistence.
- S1/S2: save/restart and two-client visibility for the mechanics touched in
  the same session.

For each chosen vertical slice, preserve the vanilla loop shape:

```text
incoming packet or scheduled tick
-> permission/distance/cooldown/game-rule checks
-> data-driven resolution
-> authoritative world/entity/inventory mutation
-> neighbour/scheduled/entity follow-up
-> exact packet/state sync outward
```

Do not count a mechanic as improved because only a unit helper passes. Add or
extend wire-level harness tests under `crates/mc-test-harness/tests/` or
real-path `mc-net` tests that exercise the packet/session/tick path.

### Lane B - Oracle And Real-Client Evidence

For every gameplay row touched, add at least one evidence hook:

- Extend a M79 oracle scenario or add a new manifest under
  `tools/m79-oracle-scenarios/` when a vanilla server comparison is practical.
- Extend `crates/mc-test-harness/tests/parity_oracle.rs` only when the scenario
  emits normalized observations and reports local-artifact degradation honestly.
- Extend the M94 real-client manifest when the behavior is client-visible and a
  PrismLauncher check is the right proof. Protocol bots do not count as
  real-client evidence.
- Update `docs/VALIDATION_LEDGER.md`,
  `docs/VALIDATION_COVERAGE_AUDIT.md`, and
  `docs/REPLACEMENT_READINESS.md` for every row touched, including unchanged
  debt and skipped evidence.

### Lane C - Performance, Lock Ownership, And Queues

Keep reducing M90/M91 blockers while adding gameplay breadth:

- Generated-world chunk/light streaming: `crates/mc-net/src/play/chunk_stream.rs`,
  `crates/mc-world/src/storage.rs`, `crates/mc-world/src/light.rs`, and
  `crates/mc-worldgen/`.
- Dirty flush and save/restart: keep plan/write/commit work outside long world
  locks; do not re-encode unchanged chunks under lock when a retained snapshot
  or generation check proves they are clean.
- `SessionRegistry`: avoid holding registry locks while building large outbound
  command batches or doing channel sends.
- Slow clients: preserve bounded outbound queues and prove a paused reader does
  not stall active clients.

Run focused lock/queue tests for touched paths. If a performance claim is made,
record workload, debug build, hardware/profile when known, and metrics such as
tick p95/p99, chunk first/ring/full latency, lock wait/hold, queue depth,
dirty-flush plan/write/commit time, and slow-client pressure.

### Lane D - Persistence And Recovery

Any gameplay mechanic expanded in Lane A must also answer what survives:

- Save/restart for world blocks, block entities, scheduled ticks, item entities,
  player inventory/cursor state, entity health/goal/lifecycle, and world time
  where relevant.
- Crash-window limits: if fsync/atomicity is not proven, say so.
- Unknown NBT/sidecar preservation: do not destroy fields Solaris does not yet
  model.

## 5. Suggested First Work Packet

If no narrower owner prompt exists, start with this packet:

1. Scout B1/B3/S1/O2 together: block edit transaction safety, bucket/fluid
   scheduled ticks, save/restart, and dirty/lock behavior.
2. Add a failing real-path test for one stale/rejected block or fluid edit that
   currently lacks resync, persistence, or two-client visibility evidence.
3. Implement the smallest behavior fix through packet/session/world storage
   paths.
4. Add one oracle or real-client-manifest hook for the exact scenario.
5. Update ledger/readiness docs with exact evidence and remaining debt.
6. Run focused tests, `cargo fmt --all -- --check`,
   `cargo run -p xtask -- code-health`, then workspace `cargo test` and
   `cargo clippy --workspace --all-targets -- -D warnings`.

If that packet is already green or not the best blocker after scouting, choose
the next highest ledger blocker using the same four-lane pattern.

## 6. Hard DoD For Closeout

Every closeout or final answer must include:

- Quality label: `draft`, `stabilization`, or `release-ready`.
- Cargo baseline: exact commands run after the final change.
- Code-health gate: exact `cargo run -p xtask -- code-health` result after
  the final change.
- Focused tests: what behavior they prove and why they are real-path.
- Vanilla oracle: run/cited or explicitly not run.
- Client/manual gate: owner-run, agent-run through real-client automation,
  prepared only, or not run.
- Performance/concurrency: measured or explicitly not relevant.
- Known gaps: concrete list, not generic "future work".

Do not claim vanilla parity, replacement readiness, or production-quality
behavior without this matrix.

## 7. Vanilla Parity Target

The project is not bit-perfect vanilla, but the release target is at
least 80% of scoped overworld-survival mechanics covered and implemented
well enough for a normal vanilla 26.1.2 client. The remaining behavior
must be explicit non-goal, deferred debt, or documented Solaris
semantics.

## 8. Client Automation Backlog

If asked to improve validation infrastructure, a high-value direction is
an MCP server or equivalent tool that drives a real PrismLauncher/vanilla
client and records reproducible observations. Protocol-only bots do not
replace this gate.

## 9. Owner Boundaries

Agents prepare branches, commits, docs, and validation. The owner merges,
tags, pushes, and performs default PrismLauncher gates unless explicitly
delegated in the current session.

## 10. Known Local Caveats

- The working tree may already be dirty on `main`; do not revert unrelated
  owner/previous-agent changes.
- Local-only files such as `.serena/`, `.analysis/`, `data/vanilla/`,
  `YOLO_MODE.md`, `log.log`, and `opencode.json` must not be staged unless the
  owner explicitly asks.
- In the managed sandbox, tests that bind local listeners can fail with
  `PermissionDenied`. Re-run the exact failing `cargo test ...` outside sandbox
  via approval before treating it as a code regression.
