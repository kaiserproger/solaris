# Definition Of Done

This document is the hard process contract for Solaris agents. It does
not slow down draft implementation; it prevents draft work from being
reported as release quality.

## Quality Labels

Every plan, closeout, and final response must use one of these labels:

- `draft`: implementation exists and the required cargo baseline is
  green or explicitly degraded. Vanilla parity, manual client behavior,
  and performance are not fully proven.
- `stabilization`: the slice is being hardened. Known gaps are tracked,
  oracle/client/performance evidence is being added, and regressions are
  fixed before new breadth work.
- `release-ready`: the hard DoD below is satisfied for the scoped
  mechanics. This label is not allowed for broad systems with only unit
  or focused harness coverage.

If in doubt, label the work `draft`.

## Autonomous Preflight

Run this before milestone code, before a closeout, and before any claim
that a manual/client gate is ready:

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

Also check, when relevant:

- Port `25565` is free before starting `mc-server`.
- `.analysis/test-world` is present, or tests are expected to skip.
- The current branch matches the milestone branch named in the plan.
- Local-only files such as `.serena/`, `.analysis/`, `data/vanilla/`,
  `YOLO_MODE.md`, `log.log`, and `opencode.json` will not be staged.

Report the preflight as `full`, `degraded`, or `blocked`. A degraded
preflight must name the missing item and the validation it weakens.

## Evidence Matrix

Closeouts must include this matrix, even for drafts:

| Gate | Required wording |
|---|---|
| Cargo baseline | Exact `fmt`, `xtask code-health`, `clippy`, and `test` commands run after the last code change, or why not. |
| Focused tests | Which behavior they prove, and why they are not pass-by-construction. |
| Vanilla oracle | `wire-probe`, `javap`, decompiled source, vanilla server capture, or explicit `not run`. |
| Client/manual gate | PrismLauncher or approved real-client automation status: owner-run, agent-run, prepared only, or not run. |
| Performance | Metrics for latency/TPS/lock contention when the slice touches hot paths. |
| Concurrency | Evidence for no global lock regression or blocking tick/network path when relevant. |
| Data/protocol facts | Packet IDs/layouts, registry IDs, sidecar version/schema, and stale/missing-data behavior cited or marked not run. |
| Persistence/storage | Save/restart/crash-window evidence, storage formats touched, backup/recovery limits, and unknown-NBT/sidecar preservation status. |
| Dependencies | New/changed dependencies, `Cargo.lock` reason, license/security audit status, or explicit not run. |
| Known gaps | Exact list of deferred behavior and whether it is draft debt or accepted non-goal. |

Do not write "all green" unless every required row is green or the
exceptions are listed next to the claim.

Focused/oracle tests that skip because local artifacts are absent must
emit or record degraded coverage. Silent skip is allowed only for draft
unit scaffolding and cannot count toward stabilization or release
evidence.

A known failed owner/manual scenario is `blocked` until the same or a
stricter real-client scenario is rerun and recorded. Green cargo or
harness gates cannot downgrade a failed real-client observation.

## Vanilla Parity Target

Solaris is not a bit-perfect clone, but the release target is hard:

- At least 80% coverage of scoped vanilla overworld-survival mechanics
  that a normal 26.1.2 client can observe.
- The remaining 20% must be documented as non-goal, deferred, or
  knowingly divergent Solaris semantics.
- A mechanic counts toward the 80% only when its normal path is covered
  by tests and at least one vanilla/client-visible evidence source.
- Packet IDs, field layouts, registry/data facts, and gameplay timing
  claims must cite local vanilla evidence, not memory or wiki guesses.

Use the 80% target for release/stabilization planning, not as an excuse
to water down individual claims. If a closeout says "sign editing
parity", sign editing itself needs evidence; it cannot borrow coverage
from unrelated mechanics.

## Draft Implementation Rules

Fast breadth-first work is allowed, but must stay honest:

- Draft milestones may land partial systems, scaffolding, and local
  deterministic semantics.
- Draft closeouts must say `draft` and list stabilization debt.
- Draft work may not move `main` tags, claim replacement readiness, or
  erase oracle/manual gaps from docs.
- After several draft milestones, schedule a stabilization milestone
  dedicated to regressions, manual gates, vanilla parity, performance,
  concurrency, and test-quality cleanup.

## Stabilization DoD

A stabilization milestone is done only when:

- The cargo baseline passes after the final change.
- New or changed mechanics have focused tests that exercise real runtime
  paths.
- Vanilla/Solaris divergence is measured or explicitly accepted.
- PrismLauncher or real-client automation covers the client-visible
  scenario, or the closeout remains `draft`.
- Performance budgets are measured for hot-path changes.
- Global lock, tick blocking, async blocking, and worker-pool behavior
  are reviewed when the slice touches worldgen, storage, networking,
  entities, physics, lighting, or chunk streaming.

## Client Automation Direction

Manual PrismLauncher gates are still owner-run by default. To make them
autonomous, build an MCP server or equivalent harness that drives a real
vanilla 26.1.2 client and records:

- Connection, join/rejoin, movement, block edits, inventory/container
  actions, combat, death/respawn, save/restart, and two-client visibility.
- Screenshots or structured client observations for visual desyncs.
- Packet/client logs tied to the server run and git commit.
- A deterministic scenario file that can be rerun after fixes.

Headless mocks or protocol-only bots are not a replacement for this gate;
they are harness evidence, not client/manual evidence.

## Performance And Multithreading DoD

Optimizations and multithreading work are done only when they prove both
speed and correctness:

- State the baseline and new measurements: chunk latency, TPS, tick time,
  lock wait, queue depth, memory use, or other relevant metric.
- Show the workload used to measure it and keep it reproducible.
- Prove no client-visible regression under load.
- For concurrent code, document ownership boundaries and how blocking
  disk/network/worldgen work stays off the tick path.
- A faster result with weaker correctness remains `draft`.
