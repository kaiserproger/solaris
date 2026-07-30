# `mc-net` ownership inventory

Date: 2026-07-30

Checkpoint base: `5770c432a849d90b88bd14b7f3f0e262cfd9d709`

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
