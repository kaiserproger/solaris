# v0.0.3-alpha.1 release closeout evidence

Date: 2026-08-30
Candidate tree: `6ea4e058` (dashboard + plugin-pack + review/lint fix commits
on top of the carried alpha-3 worktree commit `5b8b54c7`).

## Automated gates

- Fresh benchmark matrix: [`../performance/2026-08-30-benchmark-matrix.md`](../performance/2026-08-30-benchmark-matrix.md).
  All frozen budgets pass on the candidate tree. Two honest annotations: the
  debug-only VD8 rare-stall flap (max 52–53.5 ms in 3 of 5 runs; p99 stays
  ≤34.5 ms; release VD8 tick p99 ≈1.1 ms) awaits an owner decision or a
  follow-up profiling checkpoint, and worldgen throughput measured ~2.6×
  below the 2026-08-18 refresh while still 40 % above the frozen 743.578
  chunks/s floor on six physical CPUs.
- Real-client smoke: **passed** (agent-run through the repo client MCP, no-debug
  manifest, scenario `playable-01-join-generated-spawn`; validator exit 0;
  artifacts `.analysis/real-client-runs/20260831T000011Z-real-client-playable-loop-7SgQ3j`,
  zero WARN/ERROR server lines). Two release blockers were found and fixed by
  this gate:
  1. The configuration-phase server-brand payload, previously sent first,
     disconnected NeoForge 26.1.2.76 clients (`Duplicate handler name:
     neoforge:vanilla_filter` in `ClientConfigurationPacketListenerImpl` — the
     features and brand handlers each run the vanilla-fallback connection
     initialization without an idempotence guard). Publishing the brand
     immediately before `FinishConfiguration` (after registry/tag and Loader
     negotiation) resolves it; wire preambles in `tests/configuration.rs`,
     `tests/login.rs`, `tests/play.rs`, and the harness `Client` were
     migrated to that order.
  2. A teleport-confirmation race (`expected=2 received=1`) warned on the
     normal stale-confirm path when the server supersedes a pending teleport
     before the client confirms the earlier one; stale lower-id confirmations
     are now accepted quietly (ids are monotonic), keeping the warn for
     genuinely unexpected ids.
- Release L2 (pending): `xtask code-health`, `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo fmt --all -- --check`.

## Owner-pending items

1. Terrain verdict: review package
   [`alpha3-terrain-owner-review-004-2026-08-30.md`](alpha3-terrain-owner-review-004-2026-08-30.md)
   (mosaics verified byte-identical against the candidate generator).
2. VD8 debug flap disposition (see above).

`v0.0.3-alpha.1` is tagged only after both are resolved.

## 2026-09-05 current-worktree revalidation — draft

Base HEAD: `638543ab4771f7db9aef93cfaeed7e2fae832312`; the starting tree
already contained 83 unstaged/untracked paths. This is not a new release verdict.
The source delta is recorded separately from that carried work in
`.analysis/codex-logs/core-overhaul-2026-09-05/own-changes.patch`.

- `BoundServer::serve` loses eight redundant staging locals, an unnecessary
  telemetry clone, and repeated error exits, preserving subscription timing and
  error precedence. Its architecture budget passes again.
- Biome-restricted ore anchors reuse the existing column cache instead of
  recalculating surface height and climate for each anchor. The existing debug
  25-chunk profile changes from **751 to 461 ms** total and **471 to 178 ms**
  in ores. A separate generation/serialization smoke covers 48 chunks, seeds
  `0`, `712816`, `-1`, and both generation modes: all six serialized fingerprints
  match the pre-edit tree. Combined generation/serialization time changes from
  **2.101174 to 1.430373 seconds**. These are narrow debug measurements, not
  release throughput or an all-hardware capacity claim.
- The blaze wire scenario now provides an unobstructed combat arena and publishes
  its actual heightmap before joining. The original charge/reset, projectile,
  damage, and removal assertions pass; production projectile/collision rules
  were not changed.
- The obsolete shutdown test asserting `draining == true` and CPU limit `1` was
  removed, not inverted: the carried production shutdown branch deliberately
  avoids collapsing owner lanes while calls drain. Completion/final-save tests
  remain. Independent read-only review agreed that the old empty fixture did
  not exercise the in-flight lane-safety race.

| Gate | Current evidence |
| --- | --- |
| Cargo baseline | `cargo run -p xtask -- code-health`: KEEP; `cargo clippy --workspace --all-targets -- -D warnings`: PASS; `cargo fmt --all -- --check`: PASS. |
| Workspace tests | `cargo test --workspace`: FAIL in `mc-test-harness --test block_edit`, 32 passed / 4 failed / 70 ignored. Failing scenarios: `embedded_playable_flat_move_jump_input_and_wall_collision_behave`, `embedded_generated_seed_survival_crafts_tool_and_persists_without_debug`, `embedded_short_grass_break_delivers_wheat_seeds_over_wire`, `embedded_survival_mines_logs_and_crafts_wooden_pickaxe_at_table`. Three observe movement corrections; the grass test times out waiting for inventory. They have not been attributed to this delta, fixed, ignored, or waived. Targets after this failure were not exercised by the workspace command. |
| Focused behavior | Corrected blaze TCP lifecycle PASS (5.12 s); 48-chunk generation/serialized-output comparison PASS. |
| Vanilla oracle | Owner-provided `.analysis/server.jar` downloaded and SHA-1 verified as `97ccd4c0ed3f81bbb7bfacddd1090b0c56f9bc51`; embedded metadata says Minecraft 26.1.2. No new vanilla gameplay comparison was run. |
| Real client | Agent-run graphical `playable-01-join-generated-spawn` PASS under Xvfb, seed `712816`, no debug/operator privileges. Artifacts: `.analysis/real-client-runs/20260905T011426Z-real-client-playable-loop-xC3etP`; screenshot visually checked for rendered terrain/HUD. This is a join smoke, not a survival or multiplayer verdict. |
| Performance/concurrency | Matched debug worldgen probes above; no new shared cache, locks, worker topology, AI cadence, or skipped physics. Fixed 100k-entity acceptance was not rerun or reclassified. |
| Data/persistence | No protocol layouts, data IDs, storage schemas, or worldgen revision changes; serialized chunk fingerprints match on the measured sample. No new crash/restart oracle run. |
| Dependencies | No dependency or lockfile changes in this delta. Downloaded Mojang bytes remain ignored. |
| Independent review | `CoreFinalReview`: PASS, no findings; included negative-code, cache bounds, fixture validity, and obsolete-test review. |
| Known gaps | Four wire failures above, unexercised later test targets, owner terrain verdict, and broader performance/client acceptance remain open. Alpha is not release-ready. |

The temporary generation probe was removed. No commit, staging, push, or tag was
performed. The next owner-requested order is dead-code deletion, LOC/complexity
reduction, crate ownership cutover, then measured R&D across declared workloads
and hardware envelopes; existing red behavior gates remain visible throughout.

### Core ownership cutover and CPU-affinity R&D

This closes the subsequent owner-ordered dead-code, complexity, and ownership
work. It supersedes the earlier dependency and workspace-run status above, not
the open Alpha-3 acceptance requirements.

- Deleted 667 lines of unreferenced `mc-entity` AI scaffold and its standalone
  implementation. Removed the unused `bytes` edge from `mc-worldgen` and
  `tracing` edge from `mc-test-harness`; `Cargo.lock` changes only those two
  dependency-list entries.
- Removed `ScriptInventoryPlan`; callers now consume `PlayerInventory`
  directly. Required committed-event delivery, failure notification, and queue
  accounting now belong to `mc-script::commit_events`. `mc-net` constructs
  gameplay events and routes them; no compatibility reexports or optional-event
  delivery branch remain. Tokio channel types stay private to `mc-script`.
  ADR 0006 records the boundary.
- The captured cutover patch adds 386 and removes 1,584 Rust lines, including
  tests: net **−1,198**. The real-client manifest test retains artifact,
  forbidden-substitute, unique-ID, and ledger-coverage guards without pinning
  mutable QA status or prose.
- Core cutover validation: `mc-net` all-feature tests **2,026 passed, 8 ignored**;
  `mc-script` **207 passed**, including the five queue tests; `mc-net` doc tests
  **3 passed**. The real-client manifest target passes **50 tests**. Code-health,
  affected-crate all-target/all-feature Clippy, and formatting pass.
  Independent read-only `CoreFinalReview`: **PASS**, no findings.
- The full workspace `--no-fail-fast` run observed **12 failing tests in nine
  targets**. The manifest target was subsequently repaired and verified
  separately. Eleven other observed failures remain unresolved: four
  `block_edit` movement/pickup cases, breeze, Lua committed-event pickup, dragon,
  parity-oracle configuration, village defense, witch, and wither. The Lua case
  also fails in isolation while waiting for system chat. These failures are
  neither waived nor attributed to the ownership cutover.
- Agent-run graphical generated-spawn join smoke passes again:
  `.analysis/real-client-runs/20260905T022740Z-real-client-playable-loop-4rRzBN`.
  The screenshot was visually checked for rendered terrain and HUD. This remains
  a join smoke, not a full survival, multiplayer, or owner terrain verdict.

#### Frozen workload matrix

The owner-approved hardware envelope is **one Ryzen 5 7535HS host**, six physical
cores / twelve logical CPUs. The three profiles use `taskset` affinities `0`,
`0-3`, and `0-11`; they are not three different machines. Every registered ignored
`load_scenarios` workload ran once per profile, sequentially, with its unchanged
default parameters and a debug executable. No entity-count, client-count,
tick-count, or assertion reduction was accepted. None of the 42 executions hit
the external runner's 3,600-second deadline.

Frozen executable SHA-256:
`3c34a2c9a80d3e17cce9297c321a2bde9a79d7d4c7b599ceac94ebf5b88c6e4b`.
Captured source-delta SHA-256:
`aa541a28bdb62e93d7bc799d933bf9d1bd8f36b02f6379794ccfac51c67e3307`.

| Workload | 1 logical CPU | 4 logical CPUs | 12 logical CPUs |
| --- | --- | --- | --- |
| Bounded multiplayer survival replay | FAIL | FAIL | FAIL |
| Checked conservative transaction replay | PASS | PASS | PASS |
| Same-target placements | FAIL | FAIL | FAIL |
| Same-state shared-chest transaction | PASS | FAIL | PASS |
| Duplicate lethal commands and restart | PASS | PASS | PASS |
| 40,000 hostiles / 60 clients | FAIL | FAIL | FAIL |
| Multicore login, chunks, and broadcast | PASS | PASS | PASS |
| Paused reader with active entity broadcasts | PASS | PASS | PASS |
| Paused-reader pressure and healthy observers | FAIL | FAIL | FAIL |
| Four-active / one-slow 36,000-tick soak | FAIL | FAIL | FAIL |
| Short transaction-soak preflight | FAIL | FAIL | FAIL |
| Spawn, exploration, block, entity load report | PASS | PASS | PASS |
| VD8 stop/drain/disk flush | FAIL | PASS | PASS |
| VD8 twenty-client full-window drain/stop | FAIL | FAIL | PASS |
| **Totals** | **6 PASS / 8 FAIL** | **6 PASS / 8 FAIL** | **8 PASS / 6 FAIL** |
| Peak process RSS | 1,169.5 MiB | 1,147.0 MiB | 1,285.7 MiB |

The authoritative result is **20 PASS / 22 FAIL**, not a capacity approval:

- At one CPU, the entity workload never establishes the complete active
  population: 17,744 observed versus 40,000 required. At four CPUs it activates
  40,000 but times out waiting for warmup ticks. At twelve CPUs it completes
  200 warmup and 1,200 measurement ticks, then fails because only **7 of 60**
  sessions remain active. That 1,106-second run is not a valid 60-client
  throughput or latency acceptance sample.
- Both soak variants fail the quiescent-session-registry precondition on every
  profile. Their intended long-running workload was not completed; the
  36,000-tick name is not evidence that 36,000 ticks ran.
- Bounded replay misses its center-chunk frame; same-target placement misses
  game-mode or inventory synchronization; pressure traffic gets a broken pipe.
  The one-CPU VD8 stop/flush case does not receive its required chunk window.
- The twenty-client VD8 workload exceeds its 50 ms maximum-tick gate at one and
  four CPUs: **147.132 ms** and **68.857 ms** respectively. It passes at twelve.
- The four-CPU chest case passes its one-winner cursor and exact item
  conservation checks, then observes **3 aggregate container commits instead
  of 2**. The original counter assertion remains; that row stays FAIL. A
  diagnostic run with the assertion temporarily removed is explicitly
  discarded as acceptance evidence, and the test source was restored
  byte-for-byte. No matrix result is replaced by that diagnostic run.

Exact scenario names, commands, binary/source fingerprints, per-case exit
status, wall/CPU time, peak RSS, and raw logs are retained under
`.analysis/codex-logs/core-overhaul-2026-09-05/rnd/`; `matrix-summary.json` contains
all 42 original results. `rnd-run-config.json` is retained as the immutable run
manifest. The temporary runner and reverted diagnostic patch are removed only
after all three profiles complete.

No commit, staging, push, or tag was performed. Alpha remains **draft / not
release-ready**: ordinary wire failures, the red workload gates, and the owner's
terrain verdict are still open. The next outcome is reliable normal
movement/pickup and Lua authoritative-event delivery under the unchanged wire
contracts, followed by remeasurement of the affected failed workloads.
