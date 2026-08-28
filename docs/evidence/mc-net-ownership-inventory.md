# `mc-net` ownership inventory

Date: 2026-07-30

Checkpoint base: `5770c432a849d90b88bd14b7f3f0e262cfd9d709`

## Phase-2 refresh — 2026-08-18

The numeric inventory below is the original 2026-07-30 selection snapshot and is
kept as historical evidence. The current tree has since completed multiple bounded
lower-crate cutovers. Current source, focused tests, and `xtask code-health` are the
authority for ownership; the old physical-line totals are not reused as current
measurements.

Completed lower-crate ownership now includes movement geometry, bounded chunk
coverage and chunk-stream planning, natural-spawn planning, shared block semantics,
player survival/combat rules, protocol-independent gameplay values, canonical
`ItemStack`, inventory/equipment transactions, item/enchanting semantics, recipe
semantics, villager trade-input semantics, and deterministic block-placement state
rules. The corresponding `docs/evidence/*-crate-boundary.md` records name the
retained `mc-net` adapter and the code-health fence for each cutover.

The remaining `mc-net` logic is classified explicitly rather than being accepted
because it happens to live in a child module:

| Remaining area | Current ownership justification | Next disposition |
| --- | --- | --- |
| `play.rs` / `play_loop_inner` | Play packet decode, connection-local state, slow-client fencing, owner requests, and wire publication are legitimate transport/orchestration responsibilities. Movement, player-state, use/interact, container, player-control, client-metadata, chat/command, simulation-tick, chunk-stream, damage, wake, and inventory behavior now live in narrow family helpers. The root keeps select scheduling, exhaustive outbound projection routing, keepalive/teleport confirmation, liveness/rate admission, and explicit family dispatch. The frozen gateway is **731 lines**, down from 1,522 before `SOL-042`. | Keep the current transport root; future cuts require a concrete cohesive family rather than replacing the remaining outbound projection match with a generic god-dispatch. |
| `play/simulation.rs` / `process_batch` | Simulation queue admission, regional/world authority access, commit dispatch, response fencing, metrics, and post-commit publication coordination belong to the simulation owner. Pose batching, read/save/pickup/attack/effect response families, survival/player-item routing, chest/furnace publication, and opaque-block-entity/campfire response adapters now live in owner-local helpers. The frozen gateway is **998 lines**, down from 1,439 before `SOL-042`. | Keep owner coordination; the gateway is now sub-1,000 and future cuts are selected only when mutation/publication order remains literal rather than by line count alone. |
| `server.rs` / `BoundServer::serve` | Listener lifecycle, composition, startup/shutdown supervision, runtime wiring, owner drain, and error propagation are composition-root work. Script-commit setup/drain, world/save/dirty setup, command-task wiring, admitted-connection spawning, and the complete entity-ticker runtime now live outside the root. The frozen gateway is **277 lines**, down from 1,553 before `SOL-042`. | Keep the composition root at this boundary; gameplay/entity tick behavior must not return to `serve`. |
| `server/entity_ticker.rs` / `run_entity_ticker` | The entity runtime owns tick cadence, `SimulationOwner`, command-vs-tick fairness, entity physics fencing, natural/entity/world ticks, periodic save requests, runtime-control work budgets, metrics/memory-pressure workers, and the final simulation drain. Moving it out of `serve` changed no branch order or authority. The runtime has its own frozen **1,053-line** ceiling so child-module extraction cannot hide growth. | Keep this explicitly classified runtime; future decomposition must be by coherent tick phase with focused evidence, never by line count alone. |
| `play/session.rs` and authority children | Session indexes, lock acquisition, immutable snapshots, CAS/transaction authority, and publication adapters are accepted session ownership. | Keep concrete owner adapters; code-health prevents completed semantic definitions from returning to the parent/root. |
| `play/chunk_stream.rs` | Scheduler state, generation/IO, prepare admission, cache/backpressure, autoscale limits, and packet publication are runtime stream ownership; pure ordering/window/prewarm math is already in `mc-world`. | Keep runtime scheduler/IO adapter; extract additional pure rules only when a touched path exposes one. |
| `play/persistence.rs` / `play/world_journal.rs` | Crash/recovery, journal, checkpoint, and durable world/player/entity coordination are authority-sensitive persistence state machines. | Keep until a separately evidenced recovery-safe ownership cut exists; do not mechanically split for line count. |
| `play/inventory.rs`, containers, recipes, merchant, combat, placement, survival adapters | Mutable session/container/player state, registry adaptation, commit settlement, cooldowns, persistence, and wire projection remain after pure rules moved to lower crates. | Keep these adapters. The survival/mining caller/shim cleanup is complete; future wrapper/fallback deletion is tied to the next touched vertical rather than speculative global cleanup. |
| `play/session/pickups.rs` | Item/XP/arrow claim tokens, inventory credit, regional entity commit, exact readiness indexing, and dispatch are one authority-sensitive flow. | Keep the proven claim/commit path while the 200-action lock gate is green; only split publication in a separately fenced checkpoint. |
| login/configuration/loader/connection/control-plane surfaces | Protocol state machines, admission, negotiated Loader transport, liveness, and operator runtime control are network/control-plane responsibilities. | Keep in `mc-net`; these are not gameplay-semantic extraction targets. |

Generic code-health checks reject direct lower-crate Cargo dependencies on `mc-net`
and reject transport/session symbols in lower semantic crates. Domain-specific checks
pin every completed cutover and require the network adapters to consume the lower
owner. The current Phase-2 migration set is closed: no known superseded touched-domain
shim remains, all mixed runtime areas are explicitly classified, and root growth is
fenced at Play 731 / server 277 / simulation 998 with the extracted entity ticker
separately fenced at 1,053. Large exact source moves in this checkpoint used the
owner-authorized local shell path where CodexPro's targeted `edit` false-positive guard
blocked the giant file; the resulting source was reviewed and validated by the normal
Rust/test/code-health gates.

## Method

The inventory measures the current Rust sources under `crates/mc-net`:

- physical lines per file;
- `#[test]` and `#[tokio::test]` attributes per file;
- top-level struct, enum, impl, function, and async-function concentration;
- direct Cargo edges and lower-crate reference sites;
- table-driven and path-specific `xtask code-health` coverage;
- concrete state machines and whether they are domain rules or accepted
  transport, authority, commit, or publication adapters.

Physical lines and lexical reference counts are triage signals, not extraction
decisions. Large mixed files are classified by authority and dependency shape
before selecting work. Raw generated measurements remain ignored at:

- `.analysis/codex-logs/mc-net-ownership-file-metrics.tsv`;
- `.analysis/codex-logs/mc-net-ownership-test-concentration.tsv`;
- `.analysis/codex-logs/mc-net-ownership-dependency-totals.tsv`;
- `.analysis/codex-logs/mc-net-ownership-plant-tests.txt`.

## Size and test concentration

The crate contains 205 Rust files, 189,753 physical lines, and 1,741 test
attributes. This is a source-concentration count, not a claim that every test
is enabled or executable in one configuration. Test code materially inflates
several large production files, so raw file size cannot select the next
extraction.

Largest production-bearing files:

| File | Lines | Tests in file | Current role | Inventory decision |
| --- | ---: | ---: | --- | --- |
| `play/simulation.rs` | 16,962 | 87 | simulation command/owner orchestration and publication coordination | keep the coordinator; extract only bounded domain children |
| `play.rs` | 14,334 | 6 | Play packet driver and legacy gameplay adapters | shrink one vertical domain at a time; never move wholesale |
| `server.rs` | 9,948 | 97 | listener/composition root plus runtime adapters and tests | keep composition; continue child extraction |
| `play/chunk_stream.rs` | 8,794 | 69 | chunk-stream state machine, preparation, publication, and tests | queued feature-sized boundary, not selected by size alone |
| `play/persistence.rs` | 4,350 | 28 | regional journal plus player/entity persistence projections | mixed durability authority; split only with exact recovery evidence |
| `control_plane.rs` | 2,502 | 34 | operator runtime control | accepted transport/control surface |
| `play/session.rs` | 2,211 | 0 | registry fields, guard acquisition, and narrow parent routing | accepted authority skeleton; definitions must not return here |
| `play/session/entity_simulation.rs` | 2,141 | 0 | entity-owner tick and commit coordination | accepted session/entity adapter |
| `play/survival.rs` | 1,989 | 19 | health, mining, drops, and snapshot adapters | mixed domains; first split pure rules before any authority move |
| `play/block_placement.rs` | 1,907 | 12 | isolated placement planning rules | already fenced by code-health |
| `play/session/pickups.rs` | 1,838 | 0 | pickup authority, inventory credit, entity commit, and dispatch | mixed authority/publication; not a low-risk first lower-crate cut |
| `play/world_journal.rs` | 1,814 | 16 | resident mutation journal and recovery | accepted durability adapter |

The largest dedicated test clusters are:

| File | Lines | Tests |
| --- | ---: | ---: |
| `play/tests.rs` | 21,707 | 372 |
| `play/session/tests.rs` | 13,979 | 268 |
| `play/tests/spawning_and_world.rs` | 3,868 | 81 |
| `play/tests/inventory_and_survival.rs` | 2,001 | 80 |
| `play/persistence_entity_load_tests.rs` | 1,575 | 49 |

The ten most test-dense files contain 1,165 of 1,741 tests (66.9%). The top
four alone contain 824 (47.3%). A selected extraction must therefore move or
retain focused coverage explicitly; a green aggregate `mc-net` package is not
evidence that the new lower-crate boundary is directly tested.

## State-machine and authority map

| Area | Concrete owner | Lower dependencies | Classification |
| --- | --- | --- | --- |
| connection lifecycle | `connection_driver`, `server.rs` | protocol, world, script services | legitimate transport/composition |
| Play connection | `play.rs` | every gameplay lower crate | legitimate packet driver with remaining legacy adapters |
| simulation queue/owner | `play/simulation.rs`, `simulation/queue.rs`, `simulation/regional_mutation.rs` | entity, world, data, script | legitimate owner/commit coordinator with bounded extracted children |
| session registry | `play/session.rs` and authority children | entity, world, physics | legitimate snapshots, locks, commit, and publication adapters |
| chunk streaming | `play/chunk_stream.rs` | world, data, protocol | mixed stream state machine and wire publication; queued separately |
| durability | `play/persistence.rs`, `play/world_journal.rs` | entity, data, NBT | mixed recovery state machines; move only with crash/replay fences |
| entity simulation | `play/session/entity_simulation.rs` | entity, world, physics | session-owner adapter; lower entity rules are already staged elsewhere |
| survival/mining | `play/survival.rs` | data, world, entity | mixed pure rules and Play snapshots; requires more than a mechanical move |
| pickup flow | `play/session/pickups.rs` | entity, data, world, protocol | authority plus semantic credit plus dispatch; publication still mixed |
| plant rules | `play/plants.rs` | world, data, three protocol DTOs, parent helpers | pure deterministic domain planning with no lock, async, commit, or delivery |

## Dependency edges

`mc-net` directly depends on `mc-data`, `mc-entity`, `mc-extension`, `mc-nbt`,
`mc-physics`, `mc-protocol`, `mc-script`, and `mc-world`. Current lexical
lower-crate reference sites, including tests, are:

| Lower crate | Reference sites |
| --- | ---: |
| `mc-world` | 2,501 |
| `mc-data` | 1,309 |
| `mc-entity` | 881 |
| `mc-protocol` | 268 |
| `mc-script` | 163 |
| `mc-physics` | 104 |

The production reverse tree has only `mc-server -> mc-net`;
`mc-test-harness -> mc-net` is a dev edge. No lower gameplay crate depends on
`mc-net`, so the next extraction can preserve the required one-way direction.

The selected plant module currently leaks three wire/data shapes:
`mc_protocol::codec::Identifier`, `Direction`, and `ItemStack`. It also consumes
the parent-owned `BlockEdit` and `BlockPlanningRead` contracts. A lower-crate
cutover must replace those with protocol-neutral identifiers, directions,
edits, and read inputs rather than adding `mc-protocol -> mc-world` or
`mc-world -> mc-net`.

## Code-health coverage and exceptions

Current `xtask code-health` contains 170 ownership-anchor rules and 38 unique
path-specific `mc-net` branches. Multiple anchors may guard one module. The
three legacy roots, `play.rs`, `play/session.rs`, and `server.rs`, are checked
for definitions returning from accepted owners; they are not accepted domain
owners merely because they remain large.

The structural gate has these explicit limits:

- ownership anchors prove location, not behavioral correctness or exhaustive
  dependency isolation;
- unlisted files receive generic checks but no path-specific contract;
- production direct-send/async restrictions deliberately exempt `#[cfg(test)]`
  items;
- line-oriented forbidden-token scans supplement, but do not replace, compiler
  and focused behavioral tests.

Compiler-lint exceptions in `mc-net` total 70: 59
`clippy::too_many_arguments`, seven `dead_code`, two `unused_imports`, and two
`unreachable_code`. The largest concentrations are `play.rs` (15) and
`play/containers/crafting.rs` (11). They are maintenance signals, not proof
that either file should be the next extraction.

## Selected next vertical

The next bounded extraction is deterministic plant planning from
`play/plants.rs` into `mc-world`.

Selection evidence:

- 1,181 lines are pure synchronous block-state, growth, survival, tree,
  bonemeal, harvest, and drop planning with no lock, task, session, commit,
  persistence, or outbound delivery;
- five production consumers are already explicit:
  `random_ticks`, `block_placement`, `use_item_on_adapter`, `survival`, and
  `item_blocks`;
- 66 test-attributed plant-focused matches exist in the centralized `play/tests.rs`
  cluster, covering cactus, bamboo, sugar cane, crops, saplings, trees,
  bonemeal, berries, and cocoa;
- `mc-world` already owns block state and read snapshots and already depends on
  `mc-data`, while it has no reverse dependency on `mc-net` or `mc-protocol`;
- the protocol and parent-helper backedges are concrete and removable at the
  boundary.

The extraction must introduce protocol-neutral plant request/result values in
`mc-world`, move the focused pure-rule tests beside that owner, keep snapshot
acquisition, mutation commit, durability, relight, publication, and packet/item
translation in `mc-net`, delete superseded `mc-net` rule APIs after callers
move, add a code-health tripwire, and update ADR 0006. It does not authorize a
chunk-stream, persistence, pickup, or survival refactor in the same checkpoint.

Benchmark: not applicable. This checkpoint only records a current-tree
ownership inventory and selects work; it changes no runtime algorithm or
performance contract.

## Validation

- Exact source assertions reproduced 205 Rust files, 189,753 physical lines,
  1,741 test attributes, 170 ownership anchors, 38 path-specific scanner
  branches, and 70 lint exceptions.
- The corrected plant artifact contains 66 test-attributed plant-focused
  matches after excluding helper functions and the unrelated command-tree test.
- Both changed local documentation links resolve.
- `git diff --check`: passed.
- Independent read-only review found the original plant-test overcount; the
  evidence and active cursor now use the corrected count. No second review was
  run.

Cargo, graphical-client, and benchmark gates were not run because this is a
documentation-only measurement checkpoint with no runtime change.
