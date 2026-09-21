# Solaris current cursor

Живой курсор проекта. Держим его маленьким: здесь только то, что верно **сейчас**, и карта
остального. Исторические чекпоинты вынесены в [`docs/memory/`](memory/) — это **не** стартовый
контекст (`AGENTS.md`); открывай часть, когда вопрос про ту эпоху.

## Как этим пользоваться

- Текущее состояние, гейты, следующий шаг — ниже, в этом файле.
- История по эпохам — таблица частей; внутри каждой части есть список её секций.
- Быстрый поиск по темам: деревни/поселения — части 01, 06, 07, 08; сундуки/контейнеры/склад — 02, 04;
  Loader/плагины/Lua — 03, 04, 06; производительность — 05, 06; релизы и поле — 04.

## Архив чекпоинтов

| Файл (историческое, не startup context) | Строки исходного файла | О чём |
| --- | --- | --- |
| [`docs/memory/log-01-villages-and-content-source.md`](memory/log-01-villages-and-content-source.md) | 3–630 | Ранняя разведка деревень, границы фич, источник ванильного контента (кэш + импортёр). |
| [`docs/memory/log-02-settlement-c1-and-handover.md`](memory/log-02-settlement-c1-and-handover.md) | 631–1266 | Профили поселений, закрытие ядра, путь записи склада (C1), снапшот передачи. |
| [`docs/memory/log-03-gameplay-and-loader.md`](memory/log-03-gameplay-and-loader.md) | 1267–1913 | Постройки в рельефе, мобы/блуждание, связка плагин↔C4, бой C4, cutover Loader wire. |
| [`docs/memory/log-04-releases-and-handoff-fixes.md`](memory/log-04-releases-and-handoff-fixes.md) | 1914–2533 | Релизы v0.0.6, починка тестов, серия handoff-issue (сундуки, вёдра, рыба, двойной сундук). |
| [`docs/memory/log-05-performance-and-worldgen.md`](memory/log-05-performance-and-worldgen.md) | 2534–3166 | Проверенные оптимизации и верификация мира (реки, биомы, берега, свет). |
| [`docs/memory/log-06-checkpoints-09-12-to-09-14.md`](memory/log-06-checkpoints-09-12-to-09-14.md) | 3167–3810 | Чекпоинты 09-12…09-14: tab-list, реки, деревни+редстоун, сигналы shutdown, feature executor A1. |
| [`docs/memory/log-07-village-generation.md`](memory/log-07-village-generation.md) | 3811–4201 | Feature executor A2 (деревья), активация ванильной генерации деревень, village solver + decor lane. |
| [`docs/memory/log-08-audit-closeout-and-inhabitants.md`](memory/log-08-audit-closeout-and-inhabitants.md) | 4202–4400 | Закрытие аудита ядра и жители деревень из маркеров частей (закрытый чекпоинт 09-15). |


## v0.0.8 release candidate (2026-09-20)

**Versioning.** `v0.0.7` is already published at `9e5e20e4`; the next release
therefore targets `v0.0.8`, never retags the published version.

**Outcome.** The harness now reports live stage start/exit records; native
Linux links use mold without leaking flags into nested Wasm guest builds;
default Rust components are deployed by the sibling package repository; guest
fixture builds serialize only their shared Cargo artifact writes. The final
repair restores outbound player survival updates and effect packet dispatch.

**Evidence.**
- `python3 -m tools.harness run correctness`:
  `.analysis/validation/20260920T030823-correctness-sbbjo0y4/result.json`
  — passed.
- `cargo test -p mc-net --lib` — 2301 passed, 8 ignored.
- TCP regressions: shulker effect lifecycle and village golem spawn/attack —
  passed inside the final correctness receipt.
- Independent read-only review `CoreCheckpointReview`: two P1 findings
  (survival-effect dispatch and Wasm-inherited mold flag) fixed before final
  L2; no remaining static blocker in core or the five converted default
  component packages.

**Publication blocker.** The core worktree contains 104 modified or untracked
paths, including owner files outside this release slice. The converted sibling
default-plugin packages are also uncommitted while CI remains pinned to their
earlier revision `2d51ae5559cdd5b7cbab32888ef2b11d931e3e6e`. No commit, tag, push,
or publication is safe until the exact core release set and the sibling
publication decision are authorized.

**Release scope and maturity.** This is a draft release. No fresh graphical
client, oracle, performance, or manual owner gate was run. The converted
packages (`solaris-permissions`, `solaris-essentials`, `solaris-economy`,
`solaris-towns`, `solaris-audit`) were statically verified by the independent
review; their Wasm binaries were not independently executed in this close.

**CP-014 boundary correction and source-owned component migration.**
`solaris-default-plugins` owns the first-party `solaris-settlements` product.
The core server and SDK contain only generic SDK/fixture code: they neither
build, package, test, nor import a first-party settlement component. Generic
durable world, inventory, resident-work, structure, and receipt boundaries
remain available to every plugin without product dependency.

The previous component source was recovered from the saved session unified diff
and verified against its original Git blob
`3b152101304c1a1f052bd6848ea13b0141e913e2`; it now lives in
`../solaris-default-plugins/sources/solaris-settlements/`. The deployed sibling
package now has API 0.7 `entry = "plugin.wasm"` and only the capabilities its
component uses. `main.lua`, legacy config, and client bundle are removed from
the deployable directory; the legacy Luau source is retained only under
`sources/solaris-settlements/legacy/` as porting input. Authoring blueprints
remain under `structures/`.

Focused core extraction checks passed: `cargo fmt --all -- --check`;
`cargo test -p mc-net resident_settlement` (17);
`cargo test -p mc-net settlement_tests` (53); the generated-inhabitant
authority test (1); and
`.analysis/validation/20260920T081931-code-health-_j4x8m7h/result.json`
(`passed`). The sibling source now passes its 8 component behavior tests and a
`wasm32-unknown-unknown --release` build. The generated package passed
`cargo run -p mc-server -- --check --config
/tmp/solaris-settlement-component-check/server.toml`, which strictly loaded
only `solaris-settlements` with its declared grants.

The sibling component now persists owner-scoped settlement creation, roles,
condition, supported site adoption, generic-core survey tokens/tags, a bounded
index of named project buildings, selected specialties, and named residents.
`/settlement project` re-reads its exact site immediately before preparing a
structure, so its revision fence is current rather than the adoption-era value;
a committed plan is stored under a generated building name such as
`warehouse_1`. `/settlement fund <name> <building>` and `advance <name>
<building>` authorize the owner or steward, select only that durable building,
then re-read its exact core structure before material reservation or a bounded
core work portion. `/settlement specialize <name> ...` allows at most two legacy
specialties only when each matching survey tag exists and its named workplace's
current core state is `Committed`; it persists only after those exact status
checks. `/settlement populate <name>` permits a member to exact-query an
adopted core-authored `site_*`, reserve its first free home, and spawn its
core-managed resident. A committed spawn is then persisted behind the same
registry CAS fence under a generated alias such as `resident_1`; `info` exposes
the durable aliases. If that first post-spawn CAS refuses or fails, the component
re-reads the registry once, reapplies the same opaque handle and revision, and
re-CASes it without creating another resident. Its command-level and state tests
total 39; `cargo fmt --manifest-path
../solaris-default-plugins/sources/solaris-settlements/Cargo.toml -- --check`,
the sibling `cargo test`, a `wasm32-unknown-unknown --release` build, component
encoding, and the strict server package check passed. Independent review found
that the first matching workplace could be uncommitted while a later matching
one was committed, and that a specialization status continuation could race a
registry update. The component now queries every matching saved workplace until
one is currently committed, and blocks concurrent registry reads during that
continuation.

The generic SDK now makes `log` a native no-op while retaining the WIT host
import on `wasm32`, so component answer-path tests can execute without a host
stub. `cargo test --manifest-path sdk/rust/solaris-plugin-sdk/Cargo.toml`
passed, alongside the 39 component tests and component package check above.

The independent command-port review caught three authority defects before this
close: named fund/build selected an unrelated plugin-global structure; generated
`village_*` sites could not reserve residents through the generic core authority;
and project reused an adoption-era site revision. The unsafe named
build/populate routes were removed rather than advertised; named funding and
advancement were reintroduced only after a durable project record and exact
core status read. A site-compatible named population route was reintroduced
only for core-authored `site_*`; generated `village_*` sites remain unsupported.
Adoption now rejects unsupported generated sites, and project queries its exact
site before the core call. The current product port is
`create → role → ruin/restore → site → adopt → survey → project → fund →
advance → specialize → populate`; population on generated villages, family,
job, claim, and abandonment remain unported.

Adoption now uses the generic targeted site query rather than a first-page list
scan; a storage CAS server failure is reported as unavailable, not as a version
conflict. The corrected component rebuild, encoding, and strict package check
also passed.

Independent review found stale core-owned settlement regression wiring and two
documentation boundary errors. The obsolete `m94-09` scenario, driver, harness
test, and product package installation path were removed from core; only generic
regression machinery remains. Its replacement validation passed:
`.analysis/validation/20260920T085538-harness-check-snl7oa0g/result.json`.

No real-client R0/R1 product scenario, two-observer flow, or full
food/material lifecycle has run. The component migration proves source
ownership, reproducible component packaging, command-flow unit behavior, and
strict admission—not campaign readiness. CP-014 remains blocked.
- CP-015, CP-016, CP-017, CP-018, CP-019, CP-020, CP-021, CP-022, CP-023,
  CP-024, CP-025, CP-026, CP-027, CP-028, CP-029 remain pending.
- CP-030, CP-031, CP-032, CP-033, CP-034, CP-035, CP-036, CP-037, CP-038,
  CP-039, CP-040, CP-041, CP-042, CP-043 remain pending.
- CP-044, CP-045, CP-046, CP-047, CP-048, CP-049, CP-050, CP-051, CP-052,
  CP-053, CP-054, CP-055, CP-056, CP-057 remain pending.
- CP-058, CP-059, CP-060, CP-061, CP-062, CP-063, CP-064, CP-065, CP-066,
  CP-067, CP-068, CP-069 remain conditional/pending.
- CP-070, CP-071, CP-072, CP-073, CP-074, CP-075, CP-076, CP-077, CP-078
  remain pending.
- CP-079, CP-080, CP-081, CP-082, CP-083, CP-084, CP-085, CP-086 remain
  pending.
- CP-087 remains a conditional handoff; CP-088, CP-089, CP-090, CP-091,
  CP-092, CP-093, CP-094, CP-095, CP-096 remain pending.
**P0 component-host revalidation (2026-09-21).** The real SDK-built component
passes the host safety matrix: raw core-module/ABI refusal before guest code,
pre-compilation artifact cap, fuel/epoch/memory/stack limits, bounded
guest-to-host lifting for oversized, cumulative, nested and now directly
aliased outputs, unpublished trap and canonical cleanup behavior, and
independent-instance isolation. The hostile alias fixture retargets a real
SDK-built component result after canonical lowering; five result records reuse
the first string pointer, have distinct declared lengths, and lift the modified
shared first byte. The strict one-package deployment baseline records Linux
x86_64 hardware/workload/RSS/latency/tick/queue/counter data. Receipt:
`.analysis/codex-logs/p0-revalidation-20260921/receipt.txt`; 14 focused tests,
formatter and code-health passed. Rust 1.94 resolves Wasmtime 36.0.15 and WIT
tooling 0.259. Native Linux AArch64 execution has not run from this workstation:
it lacks both `aarch64-linux-gnu-gcc` and a compatible sysroot. The configured
native Arm CI job is not claimed as execution evidence.
The current target check confirms the recorded blocker rather than merely assuming
it: `CARGO_BUILD_JOBS=2 cargo check --target aarch64-unknown-linux-gnu -p
mc-plugin-host` exits 101 in Wasmtime's build script, before Rust checking,
because `cc-rs` must compile `src/runtime/vm/helpers.c` and
`aarch64-linux-gnu-gcc` is absent. The installed Rust target alone is
insufficient; this is not a linker-only gap. Receipt:
`.analysis/codex-logs/p0-aarch64-target-check-20260921/receipt.txt`.

**P0 Arm CI handoff prepared (2026-09-21).** The owner authorized publishing
the exact current branch, including base `b454200b`; this prevents a default
checkout of stale `origin/main` from being misattributed to P0. The
`wasm-host-arm` job now runs only the bounded P0 matrix:
`host_bounds`, `cleanup_isolation`, `component_roundtrip`, `contract_refusal`,
and `host_baseline`. The exact command passed locally with 16 tests across five
suites in 54.07 seconds; independent review found the Cargo selectors, wasm32
guest prerequisite, and P0 coverage valid. The former full package CI command
timed out after 480.09 seconds while starting `timer_operations`; its
non-attributable preflight remains in the receipt below. Native Arm execution
is still pending the published-commit CI run. Receipt:
`.analysis/codex-logs/p0-aarch64-target-check-20260921/receipt.txt`.

**P5 precommit revalidation (2026-09-21).** Six real-component host hook cases,
15 bounded ticket/roster/deadline cases, and 27 native build/damage precommit
cases passed. The cross-region receipt test now reflects the actual
preparation/submission boundary: a cancelled `before-build` approval reaches the
regional worker but is refused before block mutation or world-journal
reservation; its prepared storage is never projected. ADR 0004 records that
boundary. Formatter and code-health passed again at
`.analysis/validation/20260921T015413-{fmt-du9288he,code-health-wjjcq4_u}/`.
Receipt: `.analysis/codex-logs/p5-precommit-revalidation-20260921/receipt.txt`.

**P6 reload/shutdown revalidation (2026-09-21).** The prior L2 workspace-test
blocker was an unrelated `set -u` defect in real-client prepare: the optional
`SCENARIO_SERVER_PORT` override was never initialized. Its
`${SCENARIO_SERVER_PORT:-}` initialization preserves an explicitly exported
port and otherwise leaves the source `network.port` intact. All 49
manifest-preparation tests passed after the correction; the failed workspace
test stage had already passed on rerun. The prior formatter, strict workspace
Clippy and code-health stages remain green. Receipt:
`.analysis/codex-logs/p6-revalidation-20260921/receipt.txt`.

**P7 package-source recovery and Basic Economy Wasm port (2026-09-21).** The
five API 0.7 standard-pack sources were recovered from core revision
`7f68cc2a`; boundary revision `b454200b` removed those paths from core so the
sibling owns first-party content. The explicit sibling build script regenerates
and byte-verifies the strict five-package deployment plus the `basic-economy`
client fixture; the strict five still stage as the only expected production
pack. The standard-pack profile passed its source check, ignored component
behavior target, strict startup, and five-package configuration:
`.analysis/validation/20260921T121331-standard-pack-7q_6e3_v/result.json`.

Following the owner's port decision, `../solaris-default-plugins/basic-economy`
is now an API 0.7 component with `entry = "plugin.wasm"` and independently
reproducible Rust source at `sources/basic-economy/`. It preserves the
`basic-economy` ID, `shop:economy:<uuid>` v2 ledger, configured zone/menu
entry, one-dirt/two-apple primary purchase, secondary refund, and the
inventory-storage/CAS authority path. Independent review found that an early
departure could retain pending state and that a rejected multi-command callback
could roll back only its final correlation. The component now cleans each exact
departed session (including pending reads, transactions, menus, notices, and
rejection state) and rolls back every correlation in a rejected output batch.
Four component unit tests, including both regressions, and the final
six-component `tools/build_standard_pack.py --write` plus `--check` passed; the
deployed Basic Economy component SHA-256 is
`45a3e70fa5c1a1fad9d3d9105a3e30c6da142d2b43e0711bb723e2d128e45d3d`.

The final real-client core profile passed:
`.analysis/validation/20260921T133734-core-client-a7okwwgp/result.json`.
Its server-only flow mined and picked up natural dirt, purchased two apples
with the ledger at one, then refunded to one dirt, zero apples, and ledger
ownership zero. The final inventory profile passed:
`.analysis/validation/20260921T134225-inventory-omn1dveo/result.json`; it
preserved the purchase result into the existing cursor/held-item/crafting
probe. The inventory-only branch intentionally leaves the bought apples
available to that pre-existing probe; refund remains asserted by `core-client`.
`harness-check` passed at
`.analysis/validation/20260921T122939-harness-check-ud9nbnj3/result.json`.
The latest root `fmt` and `code-health` passes are
`.analysis/validation/20260921T134726-fmt-ueordzad/result.json` and
`.analysis/validation/20260921T134736-code-health-rtmmc29e/result.json`.

The historical API 0.6 and `main.lua` component-rejection failures remain
recorded in the P7 receipt; they are not passing evidence. The host now reports
bounded per-component callback counts and p50/p95/p99/max latency at generation
stop. The strict five-component behavior test asserts that its actual
normal/event-storm economy run produces ordered metrics, and the final
core-client record observed Basic Economy at 13 callbacks with p50 185 us and
p95/p99/max 276 us:
`.analysis/validation/20260921T133734-core-client-a7okwwgp/server-only/server.log`.
The final inventory record observed eight callbacks with p50 172 us and
p95/p99/max 245 us:
`.analysis/validation/20260921T134225-inventory-omn1dveo/server-only/server.log`.

The canonical strict-pack profile binds the five actual components to a real
`mc-net` server under the debug profile, seed 0, zero connected clients, and a
fixed 100-tick post-normal phase. Independent review found and corrected two
gate defects: the telemetry snapshot now must reach `source_tick >= target`,
and a continuously fed queue is sampled rather than asserted instantly empty.
The corrected
`.analysis/validation/20260921T180218-standard-pack-djhdo778/result.json`
recorded tick p50/p95/p99/max 746/1045/1482/1619 us, Economy callback
p50/p95/p99/max 171/263/284/344 us over 165 callbacks, storm queue peak 62,
sampled pre-shutdown depth zero, 67/5 user/system CPU ticks, VmHWM
181204→211744 KiB, and zero host refusals. The strict-pack Xvfb gate passed:
`.analysis/validation/20260921T141704-standard-pack-qvsi6yy9/result.json`;
all five components reached Play and `/money` returned `Balance: 100 coins.`

P7 is accepted. The zero, normal, flood, infinite, slow-storage, event-storm,
and reload scenarios execute in strict component tests; the canonical live
workload adds actual-server tick/latency/queue/CPU/RSS evidence. The owner
explicitly waived the plan's unrecoverable historical Luau comparison after
confirming that the retained P0 baseline is current Wasm-only and recovering
the old runner would cross the 453-file `d4d65062`→`v0.0.7` transition. Receipt:
`.analysis/codex-logs/p7-first-party-source-audit-20260921/receipt.txt`.

```yaml
base_tree: b454200b8249ec307b831838b6b2ddf295db0a4d
diff_hash: 8a27f57ff0fcdbd281049397cecef57cc752558fb6683968a6255fa09fbfd005
diff_hash_recipe: >
  SHA-256 of `git diff --binary b454200b8249ec307b831838b6b2ddf295db0a4d`,
  snapshot immediately before this record update.
changed_files:
  - crates/mc-net/src/play/ingress/player_control.rs
  - crates/mc-net/src/play/simulation/tests/precommit_tests.rs
  - crates/mc-plugin-host/src/limits.rs
  - crates/mc-plugin-host/src/host.rs
  - crates/mc-plugin-host/src/lib.rs
  - crates/mc-plugin-host/tests/host_bounds.rs
  - crates/mc-script/src/lib.rs
  - crates/mc-script/src/tick_delivery_tests.rs
  - crates/mc-server/src/startup/components.rs
  - crates/mc-test-harness/tests/load_scenarios.rs
  - docs/AGENT_TOOLING.md
  - docs/MEMORY.md
  - docs/decisions/0004-staged-single-writer-simulation.md
  - tools/harness/backends/regression.sh
  - tools/harness/compatibility.py
  - tools/harness/precommit.py
  - tools/harness/profiles.py
  - tools/harness/test_harness.py
sibling_checkpoint:
  base_tree: 92c51a37dee282e75b8fce709b8e720509f4f43f
  tracked_diff_hash: f313f6fc2295873e598eddb00d27daaabae58d8a8dd541ee701f8d2676efd754
  basic_economy_component_sha256: 45a3e70fa5c1a1fad9d3d9105a3e30c6da142d2b43e0711bb723e2d128e45d3d
validation:
  - cargo test --manifest-path ../solaris-default-plugins/sources/basic-economy/Cargo.toml: passed (4 tests)
  - ../solaris-default-plugins/tools/build_standard_pack.py --write/--check: passed
  - cargo test -p mc-plugin-host callback_latency_window_reports_bounded_nearest_rank_percentiles: passed
  - cargo test -p mc-plugin-host: failed (480-second timeout; do not treat as package-wide pass)
  - .analysis/validation/20260921T133208-standard-pack-e1l_zsad/result.json: passed
  - .analysis/validation/20260921T133734-core-client-a7okwwgp/result.json: passed
  - .analysis/validation/20260921T134225-inventory-omn1dveo/result.json: passed
  - .analysis/validation/20260921T134726-fmt-ueordzad/result.json: passed
  - .analysis/validation/20260921T134736-code-health-rtmmc29e/result.json: passed
  - .analysis/validation/20260921T180218-standard-pack-djhdo778/result.json: passed
  - .analysis/validation/20260921T141704-standard-pack-qvsi6yy9/result.json: passed (strict pack Xvfb `/money`)
  - .analysis/validation/20260921T142707-harness-check-kky8e73o/result.json: passed
  - .analysis/validation/20260921T180822-fmt-fv2tg8hu/result.json: passed
  - .analysis/validation/20260921T180822-code-health-2uehntbf/result.json: passed
status: P7 accepted under the owner's explicit waiver of the unrecoverable legacy-Luau comparison; strict source, behavior, live-server workload, and real-client evidence passed.
next: P8 remains blocked only by P0's unavailable native Linux AArch64 execution receipt.
```

**P8 production-cutover audit (2026-09-21).** The core production source and
lockfile have no Luau runtime, legacy entrypoint or Lua dependency reference,
and the standalone `mc-server` build passed without a sibling checkout or guest
compiler. The explicit standard-pack profile proves the sibling only participates
in its integration gate. P7 is accepted; P8 remains unaccepted only because P0
still lacks native Linux AArch64 execution evidence. CP-001 onward is no longer
source-blocked, but P8 cannot be treated as a verified prerequisite.
Receipt: `.analysis/codex-logs/p8-cutover-audit-20260921/receipt.txt`.


**Blocker.** The canonical profile list has no settlement scenario, and
`docs/real-client-regression/manifests/` declares none. The client MCP credential
is absent. The sibling package rules prohibit restoring product scenario wiring
to core. Unblock with an approved product-owned canonical R0/R1 manifest and
client credential, then run the declared graphical scenario.

## v0.0.7 release close (2026-09-18)

**State.** The requested v0.0.7 release was published as `9e5e20e4` and tag
`v0.0.7`. It
adds passive-animal food temptation, sight-gated skeleton bow attacks, vanilla
chest lid block events, and documented first-start Mojang content-cache reuse.
Settlement policy remains in plugins; core only provides the generic durable
world/inventory transaction boundary.

**Evidence.** Canonical `correctness` passed at
`.analysis/validation/20260918T183211-correctness-0m_g7trd`; focused
regressions cover fractional skeleton sight traversal, blocked draw cancellation,
cross-region chest commits, durable settlement portions, and TNT damage to an
immobile chicken. Release preflight passed: installer
`20260918T182514-installer-xx4oj04t`, harness wiring
`20260918T182514-harness-check-xu1vbvf6`, and release build
`20260918T182514-build-bnct9702`.

**Boundary.** Graphical real-client, oracle, and performance gates remain
unrun, so v0.0.7 is draft. Next active task: CP-010.

## Checkpoint — CP-008 real agricultural cycle (2026-09-18)

**Outcome.** Resident `Harvest` previews canonical drops before changing a crop;
`Replant` consumes a configured seed only in the same durable world decision
that places the crop and advances work. Both paths use source-image
preconditions, fail closed for missing tool/input/unloaded terrain, and split
at the simulation's 512-edit boundary. This is generic worker/world
transaction machinery, not settlement policy.

```yaml
base_tree: 9e5e20e4b53dbb769a38f6b1e54e4257609e9ec9
diff_hash: 1b0dbccef47da92a59861b7ec2105369886621f81d1cb31a6e402e78139d1652
changed_files:
  - crates/mc-net/src/play/resident_work.rs
  - crates/mc-net/src/script/storage/resident_order_execution.rs
  - crates/mc-net/src/script/storage/resident_order_tests.rs
validation:
  - CP-008 harvest/replant, missing seed/tool, stale player edit, and 512-edit split:
      passed
  - .analysis/validation/20260918T190426-fmt-0pftdzmu/result.json: passed
  - .analysis/validation/20260918T190426-code-health-jebmaofu/result.json: passed
  - CP008AgricultureReview: pass; no findings
next: Begin CP-009 real-tree-constrained logging.
```

## Checkpoint — CP-009 real-tree-constrained logging (2026-09-18)

**Outcome.** `CutTree` accepts only a bounded rooted trunk crowned by a real
canopy block, so an adjacent grounded log column without connected canopy
evidence remains untouched. The tree still uses the existing source-image
preview, durable world decision, cargo, tool, capacity, route, and permission
boundaries; unloaded classifier reads fail closed.

```yaml
base_tree: 9e5e20e4b53dbb769a38f6b1e54e4257609e9ec9
diff_hash: 3bc2838d5b4ace37ec6165c65415dd3bf6297173b40d4ee07fa34c1f2c727295
changed_files:
  - crates/mc-net/src/script/storage/resident_order_execution.rs
  - crates/mc-net/src/script/storage/resident_order_tests.rs
validation:
  - cut_tree_commits_the_log_and_worker_cargo_together: passed
  - cut_tree_keeps_adjacent_grounded_house_logs: passed
  - .analysis/validation/20260918T191534-fmt-c1a1_isw/result.json: passed
  - .analysis/validation/20260918T191534-code-health-2gi9aje8/result.json: passed
  - CP009LoggingReview: finding fixed; no second review by policy
next: Begin CP-010 honest mining.
```

## Checkpoint — CP-010 honest resident mining (2026-09-18)

**Outcome.** `Mine` visits only the named, loaded work cells and obtains a
source-conditioned canonical break preview. A tool that produces no canonical
drops leaves its ore intact and pauses `MissingTool`, including when it follows
an already committed valid break. A full worker likewise leaves the ore intact
with `NoStorage`; no profession grants ore or scans for hidden resources.

```yaml
base_tree: 9e5e20e4b53dbb769a38f6b1e54e4257609e9ec9
diff_hash: ca9573fce49b628752d128c424a61127e2ea42607358d4d84e94d4dafbb36d9e
changed_files:
  - crates/mc-net/src/script/storage/resident_order_execution.rs
  - crates/mc-net/src/script/storage/resident_order_tests.rs
validation:
  - mined_ore_reaches_the_worker_cargo: passed
  - mine_with_unsuitable_tool_preserves_ore: passed
  - mine_pauses_for_a_later_ore_that_needs_a_better_tool: passed
  - mine_with_full_cargo_leaves_ore_untouched: passed
  - .analysis/validation/20260918T192546-fmt-mfrwyyox/result.json: passed
  - .analysis/validation/20260918T192546-code-health-7ea4rzeb/result.json: passed
  - CP010MiningReview: MissingTool after partial progress fixed; no second review by policy
next: Begin CP-011 fishing and animal-resource contract.
```

## Checkpoint — CP-011 fishing and livestock resource contract (2026-09-18)

**Outcome.** `Fish` credits the resident only from named loaded water columns;
its deterministic cod output is explicitly a bounded worker abstraction, not
vanilla fishing loot/timing parity. `TendLivestock` now admits only an alive
locally tracked adult accepted by the canonical food tags, then debits the
resident's own feed and mints no livestock goods. Missing water, animals,
compatible food, or maturity leaves inputs untouched with a typed pause.

```yaml
base_tree: 9e5e20e4b53dbb769a38f6b1e54e4257609e9ec9
diff_hash: 6a5710ccf6cfc51ca94dc02183b2024d8c89b42b9160cb2915ff86bfcd86c508
changed_files:
  - crates/mc-net/src/play.rs
  - crates/mc-net/src/play/session/resident_orders.rs
  - crates/mc-net/src/play/simulation.rs
  - crates/mc-net/src/script/storage/resident_order_execution.rs
  - crates/mc-net/src/script/storage/resident_order_tests.rs
validation:
  - fish_uses_real_water_and_credits_resident_cargo: passed
  - tend_livestock_consumes_feed_for_a_visible_animal: passed
  - tend_livestock_refuses_incompatible_or_immature_animals: passed
  - fish_and_livestock_without_sources_preserve_inputs: passed
  - .analysis/validation/20260918T194406-fmt-i9p3_95b/result.json: passed
  - .analysis/validation/20260918T194406-code-health-hx1ld9ur/result.json: passed
  - CP011ResourceReview: pass; no findings
next: Begin CP-012 warehouse-backed production.
```

## Checkpoint — CP-012 station-backed warehouse production (2026-09-18)

**Outcome.** The owner selected a mandatory station reference. Every `Craft`
now names one singleton world cell; the native executor verifies a loaded
`crafting_table` before material debit, reporting `missing_station`, `unloaded`,
or `protected` without inventing output. The existing durable chain remains
explicit: warehouse withdrawal → craft → warehouse deposit. Resident output
placement proves its full capacity before any slot changes. A failed craft
therefore restores every tentatively removed ingredient and leaves no partial
output before reporting `no_storage`. The exact craft receipt replays without a
second recipe decision.

```yaml
base_tree: 9e5e20e4b53dbb769a38f6b1e54e4257609e9ec9
diff_hash: d3a132b54d0a5a06287aa7dc6c72d546d679544adbde29c341e3df23b275010c
changed_files:
  - crates/mc-script/wit/residents.wit
  - crates/mc-script/src/resident_order_operations.rs
  - crates/mc-script/src/resident_order_operations_tests.rs
  - crates/mc-plugin-host/src/domain_residents.rs
  - crates/mc-net/src/script/storage/resident_order_execution.rs
  - crates/mc-net/src/script/storage/resident_order_tests.rs
  - crates/mc-net/src/script/storage/resident_settlement_tests.rs
validation:
  - work_orders_require_a_concrete_bounded_target: passed
  - craft_requires_inputs_and_consumes_them_exactly_once: passed
  - craft_requires_the_named_crafting_table: passed
  - workshop_returns_warehouse_inputs_as_one_real_recipe_output: passed
  - mc-plugin-host domain-residents compile: passed
  - craft_without_output_capacity_preserves_ingredients: passed
  - cargo test -p mc-net --lib craft: 51 passed
  - .analysis/validation/20260919T025512-fmt-v_atb9ld/result.json: passed
  - .analysis/validation/20260919T025522-code-health-0qjqmzan/result.json: passed
  - .analysis/validation/20260918T220935-fmt-v2rp0oy1/result.json: passed
  - .analysis/validation/20260918T220935-code-health-0l0r1dk8/result.json: passed
  - CP012WorkshopReview: pass; no findings
next: Implement owner-selected native event-gated CP-013 work resumption.
```

## Checkpoint — CP-013 event-gated resident work resumption closed (2026-09-19)

**Outcome.** Paused durable resident work resumes only from a causal event for
its exact prerequisite: world/chunk edits, chunk publication, owned-inventory
transfers (resident endpoints plus the exact-destination warehouse index), and
accepted protection-zone mutations. Each wake re-runs the original assignment
once from its committed watermark and delivers the native receipt; repeated or
non-overlapping events select nothing; an inactive owner route leaves work
paused; a reopened Store retains the exact paused job. The component Store
reload proof shows the replacement generation starts with fresh host globals
and timer schedules (fresh `probe-init`/`probe-join-1` fires) while durable
state stays core-owned.

```yaml
base_tree: 9e5e20e4b53dbb769a38f6b1e54e4257609e9ec9
diff_hash: 49889f13afc9068d8012b9c581987e0b549bf845092fc7ed728aa3963ddf4695
changed_files:
  - crates/mc-script/src/lib.rs
  - crates/mc-net/src/connection_driver.rs
  - crates/mc-net/src/server.rs
  - crates/mc-net/src/play.rs
  - crates/mc-net/src/play/block_edit_commit.rs
  - crates/mc-net/src/play/chunk_stream.rs
  - crates/mc-net/src/play/tests.rs
  - crates/mc-net/src/play/tests/{tab_list,outbound_delivery,scheduled_hoppers}.rs
  - crates/mc-net/src/script/router.rs
  - crates/mc-net/src/script/zone.rs
  - crates/mc-net/src/script/storage.rs
  - crates/mc-net/src/script/storage/{resident_order_execution,resident_orders,world_inventory}.rs
  - crates/mc-net/src/script/storage/{resident_order_tests,resident_settlement_tests}.rs
  - crates/mc-plugin-host/tests/host_deployment.rs
  - sdk/rust/examples/hello/src/resident_ops.rs
validation:
  - world_event_resumes_the_same_paused_craft_once: passed
  - chunk_load_event_resumes_the_same_unloaded_craft_once: passed
  - inventory_event_resumes_the_same_paused_harvest_once: passed
  - zone_change_resumes_the_same_protected_harvest_once: passed
  - storage_actor_delivers_one_native_resume_receipt: passed
  - warehouse_change_resumes_the_same_paused_haul_once: passed
  - compatible_reload_restarts_the_runtime_timer_store: passed
  - .analysis/validation/20260919T040028-fmt-dy1_4tl1/result.json: passed
  - .analysis/validation/20260919T040037-code-health-b2uap84q/result.json: passed
next: v0.0.8 core+survival outcomes (owner directive); L2 before any release close.
```

## Checkpoint — v0.0.7 release CI repair, iterations 1-3 (2026-09-19)

**Outcome.** Main is red-gate-free by local evidence across four root causes:
(1) the v0.0.7 release commit never added the precommit/WASM host source set —
landed complete in `7f68cc2a`; (2) the Loader job pinned the pre-wire-3 loader
`3aaa9266` — re-pinned to `0972926`; (3) the runner lacked the wasm32 target and
both heavy jobs died with the cgroup exit-241 kill under the 4G scope — test and
Loader jobs now run with an 8G cap; (4) the shipped live-gate fixture carried the
unfinished loader cycle's `input_bindings`, `sounds` and prefixed action ids,
which the pinned wire-3 index refuses — the fixture is realigned to the pinned
contract (sounds/bindings/audio tooling stripped, `confirm` unprefixed) and
proven green against the pinned loader-core suite plus the full six-module
gradle command in clean worktrees. The loader side's own `input_bindings`/sounds
cycle stays as uncommitted sibling work for its own checkpoint.

## Active checkpoint — CP-014 canonical R1/R0 chain scenario (partial)

**Landed (uncommitted).** `crates/mc-test-harness/tests/wasm_settlement_operations.rs`
gains `existing_village_walks_the_chain_and_survives_a_restart`: site list →
durable reservation → revision-fenced refresh → survey unloaded/loaded →
warehouse prepare → materials reservation charged against the player's real
inventory → funded advance committing a stage cell → warehouse bind → server
save/stop → second deployment over the same world with durable receipts proving
exact-once (re-reserve refused operation-conflict). Focused run 3/3 green twice.

**Missing-API findings (recorded, not faked).** The shipped settlement component
cannot express production at a station from warehouse inputs, worker deposit
into the bound warehouse, or food — the plan chain's production/warehouse/food
legs need real surface work (resident-Craft wiring into the settlement
component or new settlement operations) before CP-014 closes. Staged building
is drivable only through the first portion. «Точные три ревизии» read as the
three durable revision lines (reservation, structure commit, warehouse binding)
the scenario pins across restart.

```yaml
base_tree: 32a50c9c
validation:
  - cargo test -p mc-test-harness --test wasm_settlement_operations: 3 passed (x2)
  - fixture-check naevnb2z/y_gmk878: passed
  - clean-worktree gradle six-module suite against pinned loader: BUILD SUCCESSFUL
next: CI run 35426736944 verdict, then implement the missing production/deposit/food surface for CP-014.
```



## Живое состояние

## R0 of the settlements overhaul — vanilla villages as sites (landed 2026-09-15; acceptance deferred by the owner)

**Why.** The owner's three-document spec (`../solaris-default-plugins/docs/settlements/`) makes R0 the first finished outcome: one compatible Loader-required package that adopts a real vanilla village, keeps its people and blocks, and shows a truthful overview and one confirmed warehouse content on all three adapters. Entry point confirmed by the owner, including the wire/schema cutover; no scope menu.

**Contract decision (owner-confirmed).** Core already declares Loader wire **3** and client artifact index **schema 2** (`crates/mc-net/src/loader.rs:11`, `MAX_LOADER_VIEW_MESSAGE_BYTES`, `docs/PLUGINS.md:483`). The `solaris-loader` workspace was staged at protocol 2 / schema 1 and its own `AGENTS.md` froze that. The cutover is therefore *the Loader catching up* to the declared contract, not a new number, and `solaris-loader/AGENTS.md` was updated in the same change rather than contradicted silently.

**What landed.**

- **Loader wire 3 / schema 2** (`solaris-loader`, pushed as `0972926`): `PROTOCOL_VERSION = 3`, `INDEX_SCHEMA = 2`, content kinds `views`/`view_actions`/`world_previews`/`world_selection`, the eight schema-2 widget types with their bounds, the `solaris:loader/view` and `solaris:loader/view_action` wire-3 messages, the protocol-3 sound channel, and the view screen in `loader-platform-common`. Verified by my own forced run (`:loader-core:cleanTest … :forge:test`) — 94 tests, 0 failures, 0 skipped across 27 suites; the earlier all-up-to-date run proved nothing and was superseded.
- **A generated vanilla village is a settlement site** (`crates/mc-net/src/script/storage/settlement_village_sites.rs`): identity minted from world identity + dimension + the generator's start chunk (`village_<x>_<z>_<hash8>`, reversible with digest re-derivation), listed in the same page and cursor space as authored sites, queryable by that id, `provenance = vanilla_village`.
- **Contents come from the materialized world, identity from generation.** POIs are read from the placed blocks (beds by the `minecraft:beds` tag, `minecraft:bell`, and the sixteen job-site blocks pinned from the decompiled 26.1.2 `PoiTypes.bootstrap`; receipt at `.analysis/codex-logs/settlement-village-bridge/poi-types-registration.txt`), occupancy from the handles residents hold. Inhabitants come from the chunk's stored `SettlementInhabitantMarker`s, and the entity identity published is the one the spawn lane mints from the placement's claim. A village whose chunks are not generated reports `contents_known = false` rather than an empty village (`ACC-05`).
- **Adoption is the existing `claim_resident`.** `a_generated_inhabitant_is_adopted_by_the_identity_its_placement_mints` proves the chain: marker → spawned villager → descriptor identity → `claim_resident` → one resident; the same operation replays to the same resident, and reopening the storage keeps exactly one. This is the first coverage `claim_resident` has had at all.

**Deliberately not done.**

- **No binding path for a container generation placed inside a village.** R0's evidence is "одним подтверждённым содержимым склада", and that warehouse is the settlement's own authored one (`structures/warehouse.toml`), bound and read through the single existing `bind_warehouse`/`resolve_warehouse_container` path — already covered by tests that read real container contents and refuse unknown/foreign/unloaded handles. A synthetic `DurableStructure` over generated chunks would be a second binding authority that removes nothing (spec §5 certificate semantics, `AGENTS.md` "no duplicate authorities").
- **No removal of the core-village suppression branch.** `main.rs` only suppresses core villages for a plugin that declares a `[worldgen]` settlement plan; the shipped package declares none, so `ACC-01` already holds and editing that branch would be speculative work on a path nothing reaches.
- **No persistent registry of generated villages.** The plan lookup plus live chunk state already answer; a registry would drift on unload, restart and regeneration.

**Recorded gaps (not R0).** No script inventory endpoint reaches an arbitrary world container, so "community starting stock out of the village's own chests" (spec §4.1) has no API yet — that belongs with R1's hauling work, not with the warehouse binding. Village work POIs are classified from the full vanilla registration while the resident lane can still model only `none`/`nitwit`/`toolsmith`; the two limits are separate.

**The client half landed.**

- **The shipped package is Loader-required.** `solaris-settlements/plugin.toml` declares one schema-2 `[client]` bundle (`client/settlements-ui.zip`, 2116 bytes, sha256 `985a8386…`) with `views`/`view_actions` content and `present_views`/`send_view_actions`; the artifact is a deterministic ZIP whose first entry is a schema-2 index declaring one `settlement` screen `solaris-settlements:overview` (a `paged_table`, a `resource_panel` and `refresh`/`page_next`/`page_prev` buttons). The screen shows only what the package holds or reads: settlement identity, the resident roster, the cycle's stop reason and gate needs, and the contents of the warehouse container core bound for it.
- **The server now routes key-driven view requests in production.** `LoaderManifest::from_script_bundles` reads each views bundle's verified artifact index, the manifest exposes `declared_view_kinds()`, and `BoundServer::bind_internal` declares them into the session registry, so `view_request{settlement}` resolves to the plugin that ships that screen. Before this, `declare_loader_view_kinds` was dead code with no production caller and the whole view surface was unreachable. A views bundle with no screen, an unknown screen kind, or a screen id another owner owns now fails at startup (`view_index_routing_fails_closed_on_unroutable_screens`), and the shipped package routes to `solaris-settlements` (`the_shipped_settlement_package_routes_its_declared_view_kind`).
- **The artifact activates under the real Loader.** `solaris-loader` gained `LoaderShippedSettlementsTest`, which activates the shipped bytes through `LoaderContentArchive` and pins the screen kind, title, table, panel and the three declared action ids. It skips when the plugin checkout is absent, keeping the Loader workspace self-contained.
- **Test deployment helpers now copy whole packages** (`settlement_lifecycle.rs`, `mc-server`'s `deploy_sibling_plugin`) instead of a fixed file list, so a shipped artifact cannot silently fall out of a gate.

**Acceptance: deferred by the owner.** The R0 evidence runs (`ACC-01…06`, `CLIENT-01…03`, `REC-01`) are explicitly postponed ("приёмку через харнесс позже проведём"); what exists instead is the code-level half — the Loader activation test above, the core routing tests, and the green gates below. The real-client profiles additionally need `SOLARIS_CLIENT_JAR` and client credentials this machine does not have. **Nothing in R0 is claimed as client-verified.**

## R1-A: worker production becomes real cargo (landed 2026-09-15, committed with the R1-B core in `84de0bf9`)

**Why.** R0's overview can only be truthful about a warehouse if something really puts items there. The read-only R1 map showed the first blocking defect: `ItemLedger::extend_drops` only summed a per-item delta for the receipt, so `Harvest`, `CutTree`, `Mine` and `Fish` removed real blocks, rolled real canonical loot, and left it owned by nobody. Every later step of R1 — haul, warehouse stock, food, construction consumption — had nothing real to move.

**What landed.**

- **Capacity is decided before the world changes.** `ResidentWorld` gained `preview_break`, which computes the *same* canonical loot as the commit without touching the world (loot is a pure function of state, tool and the block's own seed), and `LiveResidentWorld` now shares one `break_loot` between preview and commit so the two can never disagree. The work loop previews, asks `drops_fit`, and only then breaks the block: a worker that cannot hold the loot leaves the field, tree or ore standing and reports `no_storage` instead of producing items nobody owns.
- **One cargo, real stack semantics.** `put_resident_item` now takes the item's own max stack size, merges only into *compatible* stacks (same id, no damage, no enchantments, no custom name or model) and fills fresh slots when merging cannot absorb the rest, so a deposit can no longer create an illegal stack or absorb loot into a tool. `deposit_drops` records in the receipt exactly what entered the cargo.
- **`Fish` was minting cod.** It added `RESIDENT_FISHING_CATCH` straight to the receipt per water column with no owner; the catch now goes through the same capacity check and cargo deposit, and the fixed canonical catch (rather than a loot roll) stays a recorded limitation.
- **A namespaced tool resolved to nothing.** `LiveResidentWorld::item_stack` built `minecraft:{item}`, so a work order naming `minecraft:iron_pickaxe` — the only form the Lua contract accepts — looked up `minecraft:minecraft:iron_pickaxe` and yielded no held tool. Tool-gated blocks then dropped nothing while the worker still counted the work unit: the resident mined ore and produced an empty receipt. `item_stack` now accepts a resource id or a bare path, and the new mine test fails without the fix.

**Evidence.** `harvest_deposits_the_real_crop_into_the_worker_cargo` (cargo holds the crop, the receipt never claims more than the cargo holds, and the same stacks come back through `query_owned_inventory{resident_carry}`); `a_full_worker_reports_no_storage_and_leaves_the_crop_standing` (pause `no_storage`, zero units, no positive change, crop still in the world); `mined_ore_reaches_the_worker_cargo`. Suites: `mc-net --lib resident` 57/0, `settlement` 68/0, `play::` 1789/0.

**Recorded gaps.** The block edit is still committed before the resident record batch, so blocks and receipt are not yet one journal decision (`REC-03`); that composite is the next checkpoint and reuses `WorldInventoryCommit`/`commit_owned_decision`. Fishing yields a fixed canonical catch, not a vanilla loot roll. (The category-like tool ids this section used to record — `minecraft:hoe`/`axe`/`pickaxe`, which are not items — were replaced with real items by CP-001; see the closeout below. The jobs still pause `missing_tool` because nothing issues a tool *to* a worker yet.)

**Checkpoint state (no commit authorization).** base_tree `78b4f39d`; diff_hash
`60072da87f479bddf130c500514127141d7c8f3dcd4be2f7b194fec6e1185e31` (SHA-256 over
`git diff -- crates/` plus the two untracked new files); 48 modified + 2 new
files under `crates/`, `+3920/-261` tracked. Sibling state, which this checkpoint depends on and which is **uncommitted**:
`../solaris-default-plugins/solaris-settlements` — `main.lua` +596 lines (6542
total), `plugin.toml` +13 lines (the `[client]` schema-2 block), `config.toml`
+4/-4, `README.md` +46, and the new `client/settlements-ui.zip` (2116 bytes,
sha256 `985a8386382e37b032533c59b4080843bfe948dc810d2c317106226aefc305a3`,
which `plugin.toml` declares and core verifies at startup). A clean or checkout
of that repository would silently drop the package this checkpoint's core
changes are verified against; the package must be preserved and deployed at the
same revision. Nothing staged, committed or pushed in either repository.

**Gate state at this checkpoint.** `correctness` was run four times; it is **red**, and the red is one test, not the change:

| run | profile | result |
| --- | --- | --- |
| `20260915T061232-correctness-f49zubwq` | correctness | failed — `mc-server --test play`: `lua_script_oversized_payload_is_rejected_before_the_wire` |
| `20260915T061813-correctness-r9tzyw9r` | correctness | failed — same test |
| `20260915T062359-correctness-5wii0zp2` | correctness | failed — `mc-test-harness --test commands`: three Lua chat waits |
| `20260915T063303-correctness-_r712u9v`, `20260915T063737-correctness-z9kkwpzp` | correctness | failed — `mc-test-harness --test settlement_pause_repro`: `settlement_fund_reserves_materials_and_answers_the_player`, 33 s, chat `["Settlements are still loading."]` |

Every failure is a load-sensitive wait that passes standalone: `mc-server --test play` 19/19 (0.67 s), `mc-test-harness --test commands` 13/13 (6.9 s), `settlement_pause_repro` 1/1 twice (3.5 s). This is the family already recorded above (load-sensitive, standalone-green). Two things were tried and **reverted**, because neither converged: widening the per-file wait bounds (`play.rs`, `commands.rs`, `plugin_standard_pack.rs` — reverted in full, `git diff` clean for those files) and `nice -n 10`, which additionally starves the Lua host's own wall-clock slice (`HOST_PLUGIN_MAX_WALL_SLICE` 10 ms inside a 50 ms event budget, `crates/mc-script/src/lua.rs:53`).

The one new fact worth keeping, **measured** rather than sampled: under a controlled load (four `yes` spin clients) this scenario fails 4/4 runs with the current tree **and 4/4 runs with the pre-change package** (`git show HEAD:` copies of `main.lua` + `plugin.toml`, restored afterwards and verified: 6542 lines, `[client]` present, artifact sha256 `985a8386…`); at rest both arms pass in ~3.5 s. So the stall is pre-existing load sensitivity of this scenario, and this checkpoint's plugin change is **not** the trigger — an earlier single-sample A/B that suggested otherwise was noise and is retracted here. Recipe: `for i in 1 2 3 4; do (yes >/dev/null &); done; cargo test -p mc-test-harness --test settlement_pause_repro; pkill yes`. No further bound edits and no plugin-side change for it. It stays an open item: the window is a fixed 30 s while loaded runs take 33 s, so either the scenario's readiness wait or the plugin boot path under contention deserves the fix.

## R1-B: worker cargo reaches the settlement warehouse (core landed 2026-09-15, committed and pushed; plugin half landed 2026-09-16)

**Outcome reached in core.** A `Haul` work order whose destination is a bound warehouse container really moves the worker's cargo into that container in one durable decision; a container that cannot take the cargo leaves every item with the worker and reports `no_storage`.

**What landed.**

- **The chest composite's actor is optional.** `SimulationCommand::CommitChest` now carries `actor_session: Option<SessionId>` and `player: Option<Box<ContainerPlayerPlan>>`; a deposit with no player participant is enqueued with `enqueue_with_fence(None, …)`, exactly like every other server-owned command, and the validator refuses the session-authored menu shape when its session or player plan is missing. `WarehouseTransferRequest` gained `player: Option<WarehousePlayerParticipant>` (one shape with an optional participant, not an actor enum).
- **One container half, factored.** `commit_container_half` (`crates/mc-net/src/play/session/transactions.rs`) owns the state-id fence → `commit_chests_conditionally` → state-id bump → `ChestSlots` publication for both the menu and the server-owned path; only the excluded actor and the refusal family stay caller-specific. The server-owned path keeps publishing to *every* viewer including the actor (verified by the pre-existing `server_owned_warehouse_deposit_appends_before_publishing_and_recovers_both`).
- **The worker's record rides the receipt.** `PreparedStorageBatch::validate_inventory_participant` now accepts a decision whose participants are the receipt and a non-empty `order` change, so the container's after-image and the worker's canonical record are durable together and replay together through the same `append_inventory_projection` / journal recovery the player's after-image uses.
- **Planning is canonical and all-or-nothing.** `plan_warehouse_deposit` (`crates/mc-net/src/play/owned_inventory.rs`) applies every step through `plan_owned_item_transfers`, so a deposit can never state a stack the planner would refuse; a container with no room answers `capacity`, an empty worker answers `insufficient_items`, and the two map to `no_storage` / `missing_input` respectively.
- **A haul is directed again.** `ScriptResidentWorkOrder::canonicalize` swapped `Haul { source, destination }` whenever `source > destination` in enum order (`PlayerInventory < Warehouse < ResidentEquipment < ResidentCarry`), which silently inverted `carry → warehouse` deposits *and* resident→resident hauls. The swap is deleted (`crates/mc-script/src/resident_order_operations.rs`); the fingerprint stays stable because both endpoints are named in the contract. The `Haul` arm also returned its step's move count instead of `already_done + moved`, reporting a watermark that could go backwards on resume; it is now consistent with every other work arm.
- **The plugin half landed 2026-09-16** (`../solaris-default-plugins/solaris-settlements`, uncommitted). `hauling` now binds the committed `solaris:warehouse` container through one shared `S.bind_warehouse` helper (the overview used the same call site before, so the operation id, the core-minted handle, and the replay behaviour are one implementation, not two), carries that core-minted handle into the work order's `destination`, and fails closed: a refused bind abandons the assignment and says so instead of sending a guessed destination. `JOB_WORK` names real items (`minecraft:iron_hoe`/`iron_axe`/`iron_pickaxe`) instead of the categories `minecraft:hoe`/`axe`/`pickaxe` that `gear_has` could never match. **No worker can hold those tools yet** — nothing issues items *from* the warehouse (that is CP-003) — so farm/forestry/mining still pause as `missing_tool`, now for the honest reason.

**Review follow-ups closed in the same checkpoint.**

- **One deposit tail.** `InventoryRuntime::commit_prepared_deposit` now owns everything after a caller has prepared its batch (encode the receipt → commit the container → project the ledger frame → acknowledge the decision); the player's deposit and the worker's deposit share it, so the append/projection ordering ADR 0004 exists to protect has one implementation, not two.
- **Participant pairing is enforced.** The validator admits a chest command only when `actor_session.is_some() == player.is_some()`, and `commit_server_owned` asserts the same against the durable state it holds: a plan that moves a player's items can no longer be committed with the player fence silently skipped.
- **A covered stack no longer stalls the haul.** `plan_warehouse_deposit` advances to the next source slot when the container cannot take the current stack, instead of ending the plan: a worker's cargo whose first stack fits nowhere but whose later stack does deposits the later one. Only a cargo that fits nowhere reports `capacity`/`no_storage`. Guarded by `a_haul_deposits_past_a_stack_the_container_refuses`, which fails against the previous `break` (verified by reverting the one token and rerunning the test).

**Evidence (all green on this tree).** `cargo test -p mc-net --lib` 2206/0; `cargo test -p mc-script` 129/0; `harness run fmt` PASS; `harness run code-health` PASS. New coverage: `worker_haul_deposits_its_cargo_into_the_bound_warehouse` (container merged to the exact count, carry emptied, one decision carrying the receipt, read back through a warehouse query), `a_full_container_leaves_the_cargo_with_the_worker` (no storage, cargo intact, no decision spent, world never asked), `a_replayed_haul_deposits_once` (`REC-02` shape: one real move, one decision), `a_haul_deposits_past_a_stack_the_container_refuses` (mixed cargo progresses past a stack the container refuses). The L2 `correctness` profile was NOT run this checkpoint; the pre-existing load-sensitive red it is known for (`mc-test-harness --test settlement_pause_repro`) is unchanged and unexplained by this work.

**Still open.** Warehouse *withdrawal* by a worker (the same principal in the other direction, CP-003); reserved-stock withdrawal for construction (BUILD-02/03); the construction composite (`REC-03`).

## CP-001 closeout — the plugin half of R1-B, plus the documentation sweep (2026-09-16, tree dirty, no commit authorization)

**Outcome.** `solaris-settlements` assigns a haul whose destination is the settlement's own bound warehouse container, and its job table names real items.

**Evidence.** Two integration tests in `crates/mc-test-harness/tests/settlement_lifecycle.rs` drive the shipped package through its own durable state machine with a scripted core (create → adopt → survey → project → fund → four one-unit build stages → authoritative committed status → populate/spawn → `job hauling`):

- `hauling_work_names_the_bound_warehouse_container` — asserts the admitted `BindWarehouse` comes first and names the committed structure and authored ordinal `0`, then that the admitted `AssignWork` equals `Haul { source: ResidentCarry { handle }, destination: Warehouse { handle } }` with `handle == warehouse_handle(PLUGIN, structure_id, 0)`, fenced on the revision the package just read. Its falsifiability was checked by hand: flipping the package's destination back to `resident_equipment` fails the test, and `main.lua` was restored and re-verified.
- `refused_haul_bind_assigns_no_work_and_reports_the_refusal` — an `unloaded` bind answers with the exact chat refusal and queues nothing: the next admitted command is the reply to a fresh `/settlement buildings`, so no assignment can be sitting behind it.

Gates on this tree: `cargo test -p mc-test-harness --test settlement_lifecycle` 9/9; `harness run fmt` PASS (`20260916T020935-fmt-pmxqzir0`); `harness run code-health` PASS (`20260916T020939-code-health-sxkk4xyt`) — all three re-run on the final tree after the review fixes. L2 `correctness` was **not** run — the known load-sensitive red below is unchanged by this work.

**Not reached, and why.** The plan's full acceptance (`добыча → carry → назначенная доставка → наблюдаемый chest` on a real server) needs a worker that can hold cargo, and nothing issues items *from* a warehouse yet: farm/forestry/mining pause `missing_tool` because no path equips a civilian worker, and a hired soldier is refused a civilian job. That chain belongs to CP-003. This checkpoint proves the order core receives and how the package fails closed, not a filled chest.

**Documentation sweep in the same checkpoint.** `docs/` root now holds current documents only: the two campaign archives (`docs/spark-team/` 87 files, `docs/superpowers/` 56 files) and nine superseded reviews/specs/plans were deleted, milestone sub-documents and the closed alpha plans moved under `docs/milestones/`, the field-test handoff under `docs/evidence/`, and every reference rewritten — a link check over `docs/**` plus the root documents reports 0 broken local links. Stale claims were fixed against code, not against memory: README (worldgen revision 24, the deferred owner acceptance, the Loader wire-3 contract, a documentation map), `CONTRIBUTING.md`, `example.toml` and the `SettlementProfile` docs (the vanilla profile generates villages now), `OPERATING.md`'s worldgen revision, `SOLARIS_LOADER.md` and `ARCHITECTURE.md` wire numbers, ADR 0010's status line, machine-specific paths in `AGENT_TOOLING.md`, and the accidentally tracked `sarvar/` runtime logs plus the superseded `REVIEW_FEEDBACK.md`.

**Checkpoint state (no commit authorization).** base_tree `b676669c`; core `git status`: 165 deletions and 46 modifications (143 of the deletions are the two archive trees), 11 documents re-homed as untracked files; `diff_hash` `53ee06e0f2fb42367966bc77a14925251bd02e692ca1aa5535989968c9792fa9` (SHA-256 over `git diff` excluding this cursor file itself, plus the untracked file contents). Sibling `../solaris-default-plugins`: `solaris-settlements/main.lua` +70/−21 and `README.md`, uncommitted, at base `08e4c3d`. Nothing staged, committed, or pushed in either repository.

## CP-002 closeout — the load-sensitive settlement red, and the worker-thread bound (2026-09-16)

**The red reproduced, and it was not "slow ready".** Under the gate
(`python3 -m tools.harness run correctness`, every test binary at once) the
scenario failed twice at its **first** command with `["Unknown command"]`
(`20260916T035934`, `20260916T042704`): the vanilla dispatcher answered because
the `settlement` root was not routed to the plugin. The root exists before the
server binds — `start_prepared_lua_host` blocks on the host's startup report,
which is sent after the loop that calls `register_plugin_routes` — so it can
only disappear through `unregister_plugin_routes`, i.e. the host's
"Lua plugin disabled after handler failure" path (`crates/mc-script/src/lua.rs:2856`),
or the authority's permanent `clear()` on shutdown. The host cuts an invocation
off on a **wall-clock** slice (10 ms per plugin in a 50 ms event budget), which a
descheduled thread trips exactly like a script that really burns 10 ms.

**Why it was reachable.** One scenario process peaked at **18 threads**
(sampled `/proc/<pid>/task`). Note for the record: `cargo test --workspace
--all-targets` runs test *binaries* one at a time (every `Running tests/...`
block in `20260916T042704-correctness-epbdmcnp/test.log` is followed by its own
`test result` before the next one starts), so the failure is not cross-binary
contention - it happened with the rest of the machine idle, which points at
process-internal stalls (allocation, page faults) or a borderline 10 ms slice
rather than at a dozen concurrent servers.

**The fix (owner: the process's worker budget).** `[chunk_pipeline] worker_threads`
is an absolute bound — never a percentage, never above the derived default unless
the operator says so: it caps the shared chunk/entity CPU pool and the chunk IO
pool, the region owner lanes (they already size from the same `cpu_capacity()`),
and the startup bake (`startup_chunk_worker_threads` returned
`max(configured, available)` before, so a configured bound was silently raised to
the CPU count). `playable.toml` now sets `worker_threads = 2`, `example.toml`
documents the key, and in-process servers in `mc-server`'s play/configuration
tests and in the settlement scenario use `ChunkPipelinePolicy::bounded(2)`.
Measured: that scenario process now peaks at **9 threads**; a temporary probe
(removed) showed no host invocation above 5 ms at rest or under 16 spinners, so
the slice was lost to scheduling, not to script work.

**Evidence.** `correctness` PASS three times in a row on this tree
(`20260916T043818` 223.3 s, `20260916T044201` 221.6 s, `20260916T044543` 224.2 s)
against the same gate that failed twice before. Focused: `mc-server --lib` 77/0,
`--bin mc-server` 67/0, `--test play` 19/0, `--test configuration` 14/0,
`settlement_lifecycle` 9/0, `settlement_pause_repro` 1/0, plus the new
`chunk_worker_threads_bound_replaces_the_derived_split` and the extended
`startup_chunk_workers_cover_configured_and_available_parallelism`. Receipts and
the full reasoning: `.analysis/codex-logs/settlement-readiness-stall/README.md`.

**Residual, named.** The host's budget metric is unchanged: an invocation that
trips the wall slice still disables the plugin. The bound removes the condition
that made it reachable here, and the scenario now installs a `warn` subscriber so
a future timeout carries the host's own reason instead of a chat dump alone. If
the red returns, the metric is the next owner.

## CP-003 closeout — warehouse → worker issue, both halves (2026-09-16, tree dirty, no commit authorization)

**Outcome.** A settlement can issue a *named* item out of its own bound warehouse
container into a worker's own endpoint, in core and in the shipped package, and
the assignment reports what left the container.

**Core.** A `Haul` order is now directed by both endpoints and carries an optional
`item`:

- `ScriptResidentWorkOrder::Haul` gained `item: Option<String>`, validated as a
  contract resource id (`crates/mc-script/src/resident_order_operations.rs`), and
  the Lua parser accepts `item` next to `kind`/`source`/`destination`
  (`crates/mc-script/src/lua/operations.rs`).
- `plan_warehouse_deposit` (`crates/mc-net/src/play/owned_inventory.rs`) takes the
  filter and skips stacks it did not ask for; its two sides are now
  `updated_source`/`updated_destination` because a move runs either way. A
  filtered move that matches nothing answers `insufficient_items` (a missing
  input, made exact by the filter) while a destination that cannot take a stack
  the source really holds answers `capacity` (no storage).
- `plan_resident_warehouse_move` (was `plan_resident_warehouse_deposit`,
  `crates/mc-net/src/script/storage/world_inventory.rs`) accepts either direction,
  validates the resident handle against the record, and writes the planned
  after-image back into whichever of `carry`/`equipment` the worker's endpoint is.
- The `Haul` executor arm stages the container composite when a Warehouse is on
  **either** side (`stage_warehouse_move`, was `stage_warehouse_haul`) and maps
  refusals honestly: an unresolvable or refusing container is `no_storage`, a
  full container is `no_storage`, a worker whose own endpoint has no room is
  `interrupted` (its room can be freed by the opposite move), and an item the
  container does not hold is `missing_input`.
- The assignment's `changes` are signed for the container a haul moved through:
  a deposit reports what entered it, a withdrawal what left it
  (`ResidentWarehouseMove::container_delta`).
- A resident→resident haul honors the same filter (`haul_resident_items`), so
  `item` never silently means something different by direction.

**Package** (`../solaris-default-plugins/solaris-settlements`, uncommitted). The
refusing `deposit` stub is deleted and replaced by
`/settlement issue <name> <resident> <item> <count> [equipment|carry]`: it
validates the item and the batch (1–4096, core's `MAX_WORK_UNITS`), reads the
resident back from durable storage, resolves the settlement's committed
`solaris:warehouse` through the same `S.bind_warehouse` helper the hauling job
uses, and assigns `Haul { source: Warehouse{handle}, destination: resident_equipment
| resident_carry, item }` with the stated count. The bind is idempotent (one
deterministic operation id per settlement and structure), a refused bind assigns
nothing, and the durable intent is parked (`issue-intent`) before the call so a
restart recovers the committed receipt instead of issuing twice.

**Evidence.** Core, in `crates/mc-net/src/script/storage/resident_settlement_tests.rs`
(5 new, all against the real storage/journal/deposit harness): a withdrawal takes
the named item even when it sits behind another stack (receipt `delta = -2`, one
journal decision carrying the record and the container, warehouse query reads the
remainder back); the same into equipment; an absent item pauses `missing_input`
with no decision spent; a full carry pauses `interrupted` and leaves the container
untouched; an unknown handle pauses `no_storage` and stays durably paused; a
replay takes the item once. `resident_order_tests.rs` adds the filtered
resident→resident case and its absent-item case. `mc-script`'s DTO test now covers
`item` acceptance and rejection (bad ids, empty id, identical endpoints).
Package-side, `crates/mc-test-harness/tests/settlement_lifecycle.rs` drives the
shipped `main.lua` through its own state machine: the admitted `AssignWork` equals
`Haul { source: Warehouse { warehouse_handle(PLUGIN, structure_id, 0) },
destination: ResidentEquipment { handle }, item: Some("minecraft:iron_hoe") }`
with `work_units = 2`, the carry variant reads and fills the carry endpoint, and a
refused bind assigns nothing. Falsified by hand both ways: `item = nil` in the
package fails the two issue tests, and swapping the planner's sides fails all four
withdrawal tests; both files were restored and re-verified green afterwards.

**Not reached.** The plan's full R1 acceptance (`добыча → carry → назначенная
доставка → наблюдаемый chest`, and now `issue → worker really uses the tool`)
still needs a *world* source of settlement stock: the warehouse container in
these tests is filled by the fixture, and a village's starting stock has no
container owner yet. That is the next item of the plan's CP-003 list ("закрыть
пробел источника world-container/village-stock через того же владельца
контейнера"), plus the CP-004-onward checkpoints.

**Gate state.** L2 `correctness` **PASS**
(`20260916T051759-correctness-z1ry_tg2`, 382.4 s; all four commands exit `0`:
`fmt --check`, workspace `clippy -D warnings`, `code-health`, `cargo test
--workspace --all-targets`), on the tree *before* the review fixes below. One
earlier run of this gate failed and is recorded here rather than dropped:
`20260916T051715-correctness-e18e3isk` **failed in 24 s on clippy** - the new
`item` parameter pushed `plan_warehouse_deposit` and
`plan_resident_warehouse_move` past clippy's argument limit and left
`seed_equipment` unused, all three fixed before the passing run. L1 on the same
tree: `fmt` PASS `20260916T052448-fmt-_2v26lhp`, `code-health` PASS
`20260916T052453-code-health-m4t65g3y`.

**Coverage argument for the plan's remaining CP-003 checks.** The plan also asks
for a concurrent player click, a stale container binding, open viewers seeing
the accepted version, and a real storage reopen. Those live in the *shared*
container half: a resident warehouse move and a player deposit both commit
through `commit_prepared_deposit` → `commit_container_half` →
`commit_chests_conditionally`, one function, with the same state-id fence,
publication to every viewer and journal recovery. That half is already covered by
`server_owned_warehouse_deposit_appends_before_publishing_and_recovers_both`
(`crates/mc-net/src/play/simulation.rs`) and
`warehouse_transfer_refuses_foreign_unknown_unloaded_and_stale_containers`
(`settlement_tests.rs`); what is direction-specific - which side is the resident,
the signed receipt, the after-image written back into `carry` vs `equipment` - is
what the new tests pin. The one case the work path adds on top is a stale
*resident* revision, refused by `resident_order_execution.rs:227` before any
planning, which is direction-agnostic.

**Checkpoint state (no commit authorization).** base_tree `b676669c02bcb4b4d5d886aa6f3b19ed2b072f17`;
`diff_hash` `f3729e60e8cfc5b30389e67692c503648cb7380f72201d0914f97f8432a50b47`
(SHA-256 over `git diff` with this cursor file excluded, plus the 11 untracked
documents' contents). Core files this checkpoint touched:
`crates/mc-script/src/resident_order_operations.rs` (+`Haul::item`),
`crates/mc-script/src/lua/operations.rs` (parser + import),
`crates/mc-script/src/resident_order_operations_tests.rs`,
`crates/mc-net/src/play/owned_inventory.rs` (item filter, renamed plan fields),
`crates/mc-net/src/script/storage/world_inventory.rs` (both-direction planner,
signed receipt), `crates/mc-net/src/script/storage/resident_order_execution.rs`
(staged move on either side, filter), and the two core test suites plus
`crates/mc-test-harness/tests/settlement_lifecycle.rs`. Sibling
`../solaris-default-plugins` at base `08e4c3d`: `solaris-settlements/main.lua`
+296/−34 and `README.md` +35/−…, uncommitted. Nothing staged, committed or pushed
in either repository.

## WASM plugin host, P0 (2026-09-16, uncommitted, new plan)

**Task added by the owner** (`/home/user/Загрузки/solaris-wasm-plugin-plan-deepseek.md`):
replace the Luau server-plugin runtime with WebAssembly components on Wasmtime,
behind a versioned WIT contract and a Rust guest SDK, finishing with the removal
of the Luau path (phases P0-P8). Owner also directed: implement everything first,
run the heavy gates once at the end.

**Landed (P0 vertical, executed, not documented-only):**

- `crates/mc-script/wit/` - the single public schema, `solaris:plugin@0.7.0`:
  `types`, `host` (log + own id), `commands` (`send-message` to a player, a
  session or the operator log), `events` (join/left/chat/command/operation
  settled), `lifecycle` (`configure` -> rule plan, `init`, `shutdown`) and the
  `plugin` world. One package statement, one source of truth.
- `crates/mc-plugin-host` - the Wasmtime host: engine with fuel and epoch
  interruption, per-store limits applied *before* instantiation
  (`memory_size`/tables/instances/stack), an independent epoch-ticker thread, a
  bounded `CommandBatch` staging area, `configure`/`init`/`on-events`/`shutdown`
  calls that re-arm the budget per call and retire a trapped instance, and
  `HostError` separating a package that is wrong from a guest that misbehaved.
- `sdk/rust/` - a separate workspace with `solaris-plugin-sdk` (guest bindings
  generated from the same WIT, a `Plugin` trait with defaults, config parsing,
  message helpers, `export_plugin!`) and a real example plugin
  (`examples/hello`) that greets a joining player and answers `/hello`.
- `crates/mc-plugin-host/tests/component_roundtrip.rs` - five cases against the
  bytes of that real component: it loads and answers a join and a command with
  the configured greeting; a raw core module is refused before any guest code
  runs; the artifact bound is enforced before compilation; a guest that never
  returns is stopped by the epoch watchdog as `HostError::Budget`; a guest that
  allocates without bound is stopped by the store's memory limit. All five pass
  (`cargo test -p mc-plugin-host`, 5/5).

**Dependency decision (P0 requires it).** Wasmtime 48 needs rustc 1.95 and the
workspace MSRV is 1.94; the plan says not to bump Rust without established
necessity, so the host pins **wasmtime 36.0.15**, which builds on the pinned
1.94.1 toolchain. Guest target `wasm32-unknown-unknown` installed and used.
AArch64 is not verified here (no such machine) - recorded as unverified rather
than assumed.

**P1 started.** The batch-submission seam is open and runtime-neutral:
`HostCommandAdmission` (public, built from a validated manifest) and
`ScriptHostEndpoint::try_submit_plugin_batch` (public) no longer sit behind
`cfg(any(test, feature = "lua-runtime"))`, together with the admission ledger,
`AdmittedScriptCommand::for_issued_host_plugin`, the manifest's capability
conversion and the `CommandCapabilities` builders it needs. Provenance is
unchanged: the endpoint's ledger is still the only thing that stamps
`ScriptCommand::HostAttached`, a batch that already claims provenance is
rejected, and every command is checked against the admission's capabilities
before one is queued. `cargo check --workspace --all-targets` passes both with
and without `lua-runtime`.

Package fixes found by review after the CP-003 closeout (sibling, uncommitted):
the duplicate `S.valid_resource_id` is gone in favour of the package's existing
`S.resource_id`, and `S.issue_stock_call` now sets `entry.job = "hauling"` before
assigning - the shared work-result path writes `entry.job or entry.detail or
DONE` into `resident.job`, so without it an issue would have cleared the worker's
job label (or written an item id into it).

**P1 metadata extraction.** The runtime-neutral types lost their language
prefix: `PluginPackage`, `PluginDeployment`, `PluginDiscovery`,
`PluginDisableStage`/`PluginDisableDiagnostic`, `PluginReload*`,
`ClientBundle`/`ClientBundleDiscovery`/`ClientContentKind`/`ClientLoader`/
`ClientPermission` and `GameplayRules` (16 symbols, 12 files). Genuinely
Luau-owned names stay: `LuaHost*`, `LuaString`, `LuaScriptRuntime`,
`LuaRuntimeLimits`, `LuaPlugin`. The startup-rule payload types
(`LuaClayRule`, `LuaTreeRule`, `LuaSpawnPlacement`, `LuaWorldgen*`,
`LuaSettlement*`, `LuaBiomeSpawns`) are deliberately left until P4 materializes
the WASM `configure` plan, so they are named once, next to the WIT rule plan
they become. `cargo check --workspace --all-targets` passes after both changes.

**Package layer landed (P2 manifest half).** `crates/mc-plugin-host/src/package.rs`
reads a package directory (`plugin.toml` + `plugin.wasm`) into the repository's
existing `ScriptPluginManifest` and validates it *for the component contract*:
`validate_for(COMPONENT_PLUGIN_API_VERSION)` was added next to `validate()` so the
2 runtimes keep their own versions without loosening either check (Luau packages
keep requesting 0.6.0, and the existing test that refuses 0.7.0 there still
holds). The capability vocabulary moved out of `lua.rs` into the contract
(`ScriptPluginManifest::declare_capability` + `parse_api_version`), so both
runtimes name the same capabilities and an unknown name fails the package. Five
cases in `crates/mc-plugin-host/tests/package_contract.rs` pass: a package loads
with its contract (id, event subscription, capability, command root, artifact
bytes); a Luau `api = "0.6.0"` package is refused here; an unknown capability is
refused rather than ignored; `entry = "../outside.wasm"`/absolute/empty is
refused by the canonical-path check; an artifact past the bound is refused before
compilation.

**Discovery, grants, `--check` and the adapter landed (P2 remainder, minus the
composition-root wiring).** `crates/mc-plugin-host/src/discovery.rs` reads a
deployment directory with the *unchanged* production contract: strict mode
requires every entry to be a package directory (a stray file fails), the
discovered id set must equal `expected` exactly (an empty declaration admits
nothing, a malformed/duplicated expected id is refused), duplicate ids fail in
both modes because two packages of one id would share durable state, and
permissive mode skips an ordinary broken package *with a diagnostic the caller
sees*. Grants are the operator's: with `require_grants`, every capability a
manifest requests must be granted by `[plugins.grants.<id>]` or the package fails
instead of silently running with fewer rights.

`src/check.rs` implements `--check` with no game-side effect: it compiles every
selected component, runs `configure` for the rule plan and `init` against a real
`script_boundary_pair` whose command queue nobody drains - so the opening batch
passes the same admission a live run would give it while nothing is applied - and
never creates a world, writes storage or opens a listener.
`src/adapter.rs` converts a staged batch into `mc_script::CommandBatch`: a session
target becomes `SendChatMessage`, and a *stable player identity* is resolved to
the session that identity holds at admission through a `PlayerSessions` lookup,
so an offline player is refused rather than addressed by a stale runtime id. The
WIT `message-target` lost `server-log`: an operator log line changes nothing in
the game and is the `host.log` import, not an admissible command.

Evidence: 21 focused tests in `crates/mc-plugin-host` pass (component round-trip
5, package contract 5, discovery 7, check + adapter 4), including a check of the
real SDK-built component (`api = 0.7.0`, no rule plan, no opening command) and a
check refusing a package whose `init` never returns (reported as a budget, not a
hang). `cargo check --workspace --all-targets` exits 0.

**Isolation pinned.** `a_trapped_guest_does_not_disturb_a_live_one` hosts two
instances of the same component in one engine and asserts that the spinning one is
retired with a budget error while the other still answers an `on_events` call -
the property the composition root relies on when it hosts a whole deployment
(6 tests in `component_roundtrip.rs` now).

**The host runtime landed (`crates/mc-plugin-host/src/host.rs`).**
`start_deployment(packages, limits, queues, sessions)` builds one engine, one
epoch watchdog, one `script_boundary_pair`, starts every package (compile,
`configure`, `init` through the real admission), registers each manifest's routes
and runs one `mc-plugin-host` thread that owns the endpoint. The loop maps each
`ScriptEvent` to the contract's events, delivers to the instances that want it and
submits what they answer through `to_script_batch` + `try_submit_plugin_batch`.
Decisions taken while writing it, rather than guessed:

- A player command is delivered by *routing* on the declared root, not by
  subscription: `player.command` is not a subscribable name in the contract, and a
  guest must not receive commands it never claimed.
- `player-left` now carries the session only. The server's leave event has no uuid,
  and a host-held session->identity map would be lost by the host's own restart and
  then guess; a plugin correlates a leave with the join it already saw.
- The stable-identity -> session lookup is the *server's* (`PlayerSessions`,
  implemented by the composition root), because sessions are a game-side owner.
  The host never invents a runtime id: an offline player is refused.
- A failed callback retires the instance and unregisters its routes; a package
  whose `init` never returns fails the whole deployment start instead of
  half-running.

Evidence: `tests/host_runtime.rs` (3 cases, all green with the other 22 focused
host tests): a `player.joined` from the real boundary becomes an admitted
`HostAttached { provenance: hello, request: SendChatMessage { player_id: 7,
"Hi there Ada" } }` command, with the counters reporting one delivered event and
one submitted command; a deployment whose second package cannot finish `init` is
refused at start; a package loads through the public path.

**Composition-root surface landed (P2).** `[plugins] runtime = "luau" | "wasm"`
selects the deployment's runtime - one runtime per deployment, and the Luau
loader now refuses a directory configured as `wasm` instead of silently loading
nothing. `[plugins.grants.<id>] capabilities = [...]` carries the operator's
grants; a strict (production) component deployment requires them, an
unrestricted local one does not. `mc-server --check` on a component deployment
runs the host's own `check_deployment` (compile every selected component, then
`configure` and `init` against a boundary nobody drains) and **fails closed when
any package was skipped**, because a check that reports success for a deployment
it could not read end to end is worse than no check. Evidence: `mc-server --test
cli` 43/43, including a new case where a `runtime = "wasm"` deployment holding a
package that asks for the Luau contract version is refused by name.

**Independent review of the host, and its fixes (reviewer `HostReview`).** Verdict
`changes`, four majors and three nits, all folded:

- **The runtime watchdog was dead.** `start_deployment` dropped the `EpochTicker`
  before returning, so no callback deadline could ever elapse for the whole host
  lifetime; `epoch_ticks_per_call` was an inert knob. The host now owns the ticker
  and stops it in `stop()`, and a watchdog that cannot spawn fails the start
  (`EpochTicker::start` returns `io::Result`) instead of running unbounded.
- **Retirement was far too broad.** A guest answering a contract-sanctioned
  `plugin-error`, a batch refused for transient state (full command queue, full
  admission ledger) or a message to a player who disconnected before admission all
  retired the instance *and* unregistered its routes - one "not found" answer from
  one event killed the package. `deliver` now retires only when the call itself
  retired the instance (trap, budget, answer past a bound) or when the *contract*
  refused the batch (forged provenance, invalid DTO, denied capability);
  everything else drops the batch, counts it and keeps serving.
- **The context tick was fabricated**: it counted deliveries rather than the
  server's tick. The loop now tracks `ScriptEventKind::ServerTick` and passes that
  value, with 0 before the first tick.
- **`operation-settled` left the contract**: no guest could ever receive it (no
  event kind mapped to it, and no command carried the `request-id` it needs), so
  it comes back with the phase that adds request ids, per the file's own rule.
- Retirement now records the classified failure, so a trap or a budget is no
  longer reported as an invalid answer.

Evidence: two new host-runtime cases pin the policy with the real component - a
guest that answers `plugin-error` on every event keeps its `/hello` route and is
not counted as delivering commands, and a message to an offline player costs the
batch (`commands_refused == 1`) while the instance stays live; 27 focused host
tests pass; `cargo check --workspace --all-targets` exits 0.

**P3 first slice: durable storage in the contract.** The guest-facing storage
surface is now part of `solaris:plugin@0.7.0` (`crates/mc-script/wit/storage.wit`
plus the two requests in `commands.wit` and their typed answers in `events.wit`),
and the host runs the whole two-phase path on real DTOs: a guest's `storage-get`
becomes `ScriptCommand::PluginStorageGet`, the server's typed result comes back
as `storage-get-answered` correlated by the plugin's own request id, and the
plugin's reaction is what a player sees. Semantics are the server's, not a
paraphrase: `expected-version` absent means "only if the key holds nothing", a
swap that did not commit reports only `refused`, and a failure is
`unavailable`/`durability-failed` - never "the key holds nothing". Two decisions
worth naming:

- **Conversion runs under the package's own grants.** `to_script_batch` now takes
  the capabilities of the instance's admission and pushes through
  `try_push_authorized`, so a command the manifest never declared fails in the
  adapter as the plugin's own bug instead of being silently dropped; the boundary
  checks the same grants again on submission. `HostCommandAdmission::capabilities`
  exposes what the manifest declared, never what a guest claims.
- **A malformed answer retires; transient state does not.** An unconvertible
  command (`AdapterError::InvalidCommand`) or a batch past the server's own bound
  is a broken plugin and loses its routes; an offline player, a full command queue
  and a full admission ledger drop the batch and keep the instance serving.

Evidence: `mc-plugin-host` 32 tests green, including three new cases - a full
round trip where the player is shown exactly what storage reported, a durability
failure that reaches the plugin as a failure rather than an empty key, and a
package that never declared `storage` being refused and un-routed (with the
route asserted *present* before the event so the test is not vacuous). The
fixture build is now memoized per test process: ten tests in one binary were
racing on the same guest `cargo build` and failing on the package lock.

**P3 second slice: the online-players query, and targeted results.** The same
shape as storage (`list-online-players { request, limit }` ->
`ScriptCommand::ListOnlinePlayers`, answered by `online-players-answered {
request, list<player-snapshot>, truncated }`), with the snapshot renamed from the
server's own DTO: stable identity, session and dimension all come from the
server, nothing is re-derived. Two things this slice proves that storage did
not:

- **A guest's list bound is the plugin's own.** The requested limit travels
  unchanged into `ScriptOnlinePlayersRequest`, which validates it against the
  server's 256 bound, so a plugin cannot ask for an unbounded snapshot.
- **A result addressed to another plugin does not reach this instance.** The
  test reads the very same storage result twice, once with another package as
  the target and once with this one: the first must be silence and the second
  must be answered, so neither half can pass by the host dropping results
  wholesale. This is the targeted-delivery branch that had no user before.

Evidence: `mc-plugin-host` 34 tests green (host_runtime 12), and
`cargo check --workspace --all-targets` exits 0.

**Next: P2's composition-root wiring, `serve()` half.** `serve()` still starts
only the Luau host (`start_prepared_lua_host` :980, `bind_with_scripts` :987,
`join_lua_host` :1229). The component path needs `prepare` (already written as
`component_deployment`) plus `start_deployment` and the same bind/join, and it
depends on P4 for one thing it cannot fake: `serve()` feeds `EffectiveConfig`,
`StartupData` and the Loader manifest from the prepared plugins (worldgen ore
profile, settlement plan, gameplay rules, client bundles), which for a component
deployment come from `configure`'s rule plan and the package's `[client]`
metadata. Wiring `serve()` before that would silently run a deployment with no
startup contribution, so P4's startup half comes first or lands together.

**Next after that: P3.** A package is
`plugin.toml` + `plugin.wasm` (+ optional `config.toml`). Two decisions already
made and to keep: the manifest is parsed with `toml` + `deny_unknown_fields` and
turned into the *existing* `mc_script::ScriptPluginManifest` (one contract, no
second schema), and the `capabilities` names are the ones that already exist in
this repository (the `declare_*` vocabulary the Luau manifest uses), not a new
dotted vocabulary - the plan's `chat.send` sample is illustrative, not the
contract. The host's own contract version is `ScriptApiVersion::new(0, 7, 0)` and
a component whose world differs must be refused at instantiation. Then:
`mc-script` gains a name-to-capability constructor so the vocabulary lives with
the contract, discovery adds strict/expected + grants + `--check`, and P2 wires
the host into `crates/mc-server/src/main.rs` (`prepare_configured_luau_plugins`
:150, start/bind :980-987, reload :1110, join :1229) with the WIT -> ScriptCommand
conversion in `mc-net` (including resolving a stable player id to a live
session).

**Not done yet (P1 onward).** `mc-script` still owns the Luau host: the runtime-
independent extraction (`try_submit_plugin_batch` is `pub(crate)` and cfg-gated,
so no outside host can submit a batch today) and the composition-root rewiring in
`crates/mc-server/src/main.rs` (`prepare_configured_luau_plugins` :150, host
start/bind :980-987, reload :1110, join :1229) are the next seams. P0's API
matrix is done (61 registered `solaris.*` functions: 32 direct command pushers,
29 operation variants, 20 with no first-party consumer, 3 with none at all;
admission runs `ScriptBoundary::accept_host_command` -> `HostAdmissionLedger`;
the only un-admitted routed commands are chat/broadcast/disconnect).

## Post-closeout: what changed after the L2 pass, and the machine bounds (2026-09-16)

**After the passing `correctness` run** (`20260916T051759-correctness-z1ry_tg2`) an
independent reviewer (`Cp003Review`, read-only) returned `changes` with one
material finding: `S.command_issue` had no resident guard, so `/settlement issue`
on a resident whose record carries `handle = "-"` committed a real warehouse bind
before failing, and a resident whose record says `dead`/`released` could really
receive warehouse stock that no command can take back. Fixed in the package
(`S.start_issue` now applies the same `life`/`handle == DONE` guards as
`S.start_job`/`S.start_hire`) with a harness case that rewrites the stored
resident field and proves nothing is bound, plus three smaller fixes: the
selective resident→resident test now reads each endpoint on its own instead of the
concatenated `gear()` view, the `issue` endpoint argument is case-folded like
every other keyword argument, and one refusal comment in
`stage_warehouse_move` now matches the code (`interrupted`, not `missing_input`,
for a full worker endpoint while withdrawing). Falsified by hand: removing the
guards fails `issue_refuses_a_resident_without_a_core_handle_or_a_life`;
restored and re-verified 13/13.

**L2 on the final tree is NOT re-run yet** - the owner directed that heavy gates
wait until the work is done ("сначала ВСЕ СДЕЛАЙ, потом гоняй тесты"), so the last
green `correctness` covers the pre-review-fix tree and the current tree carries
only focused suites (`settlement_lifecycle` 13/13, `resident_order_tests` 16/16,
`resident_settlement_tests` 15/15, `mc-script` DTO 7/7). Re-run
`python3 -m tools.harness run correctness` before any commit.

**Every harness run is now bounded** (owner: a workspace test run was freezing the
desktop). `run`/`client` re-exec into one systemd user scope whose properties every
child inherits: `CPUQuota=600%`, `MemoryHigh=3G`, `MemoryMax=4G`,
`MemorySwapMax=1G`, and `RUST_TEST_THREADS` matched to the quota (`600%` -> 6) so
each concurrent test keeps the CPU share it has unbounded. Measured: 12 spinners
for 4 s consume 45.6 CPU-seconds unbounded, 12.3 at `CPUQuota=300%`, 4.1 at
`100%`; a full `correctness` run peaked at ~2.2 GB inside the scope and never
touched `MemoryHigh`. Knobs (each takes a systemd value or `off`):
`SOLARIS_HARNESS_CPU_QUOTA`, `SOLARIS_HARNESS_MEMORY_HIGH`,
`SOLARIS_HARNESS_MEMORY_MAX`, `SOLARIS_HARNESS_MEMORY_SWAP_MAX`,
`SOLARIS_HARNESS_TEST_THREADS`. Note for the next run: at `CPUQuota=300%` with
unmatched threads the fixed 5 s packet waits in `plugin_examples.rs` and
`commands.rs` fail; at `600%` with matched threads they are the same share as an
unbounded run. `docs/AGENT_TOOLING.md` carries the table.

**The caps failed to protect the session once, on 2026-09-16.** A `correctness` run
under the then-defaults (`5G`/`7G`) while the desktop already held ~11 GiB was
killed by `systemd-oomd`, which acts on the *whole* `user@1000.service` tree's 50%
memory-pressure limit and then picks the largest units in it — the run's scope (10
processes, 15:30:27) *and* an interactive terminal scope (`vte-spawn-…`, 12h CPU,
15:30:24). The kernel OOM killer never fired (`journalctl -k` is empty): a cgroup
cap does not stop oomd, because oomd never looks at the scope's own limit.
Measured afterwards: a full `test` phase peaks at **1.13 GiB** (483 samples of the
scope's `memory.current`), so no run needed the allowance it had — the tree was
already near its limit and the run's share tipped it. Defaults are now
`MemoryHigh=off` (throttling is what feeds oomd: `MemoryHigh` reclaims *inside*
the scope, and that reclaim is the pressure signal) with `MemoryMax=4G` — three
times the measured need — and `run`/`client` refuse to start when `MemAvailable <
MemoryMax + 1 GiB`, naming the knob; a refusal is "the gate did not run", never a
pass. Two operational rules
follow: run one heavy thing at a time (plain `cargo test`/`clippy` outside the
harness get **no** cap, and two concurrent workspace builds is how the machine fell
over earlier in the day), and do not raise `SOLARIS_HARNESS_MEMORY_MAX` while the
desktop is loaded.

## P0 closeout — the host's own memory bound, hostile-guest evidence, and the baseline (2026-09-16, tree dirty, no commit authorization)

**Outcome.** P0's safety gate is closed with running evidence: a package can no
longer make the host allocate what it claims. The plan blocks P2's admission to
the game host on exactly this row ("отказ до неограниченного раскрытия в host
memory; учитывается суммарный объём копий"), and it was open — every limit the
host set bounded the *guest*, and nothing bounded the host's own lifting.

**The hole, measured.** Wasmtime charges a guest→host transfer budget
(`Store::set_hostcall_fuel`) and its default is `2 << 30`
(`wasmtime-36.0.15/src/runtime/component/store.rs:15`, rustdoc `:196-211` calling
it "a DoS mitigation mechanism"); the host never set it. The store limiter cannot
stand in: it bounds the guest's memories/tables/instances, not the `Vec`/`String`
the host builds while lifting an answer. For this contract's answer type the claim
is unbounded in the ways the plan names — `WasmList::new`
(`func/typed.rs:1859`) charges the descriptor array the guest declares, then the
host allocates `size_of::<Command>()` per element, and strings the guest aliases
are each copied whole.

**What landed.** `PluginLimits::hostcall_bytes` (`crates/mc-plugin-host/src/limits.rs`),
default 8 MiB derived from the contract's own admitted maximum (32 commands at
the 8192-byte text bound with the largest storage value each, plus the widest
`configure` plan) and 256x below Wasmtime's default; `store()` sets it
(`src/lib.rs`); the guest fixture grew `oversized`/`wide`/`nested`/`trap`/`recurse`
modes (`sdk/rust/examples/hello/src/lib.rs`); `tests/host_bounds.rs` holds five
cases, each running the *same* guest with the bound as the only difference, so
none can pass by the guest being unable to make the claim — with Wasmtime's
default the claim is copied and only staging refuses it, with the shipped bound
the transfer is refused and the cause is asserted to be the transfer budget.

**Falsification (run, not argued).** With the `set_hostcall_fuel` line removed and
nothing else changed, `one_answer_past_the_transfer_bound_is_never_copied` fails
with `the refusal must be the transfer budget, not guest answer rejected: on-events
returned text past the bound of 8192 bytes` — i.e. the 12 MiB claim was lifted into
host memory first. Line restored, binary green. Full reasoning and the not-proven
list: `.analysis/codex-logs/p0-transfer-bound/README.md`.

**Same checkpoint, agent lanes (each verified on this tree).**

- **Contract refusal** (`tests/contract_refusal.rs`, 2 cases): a component that
  carries the accepted version string but a foreign world is refused at
  instantiation with `HostError::Instantiate` naming the missing import, and the
  same bytes through a lax linker instantiate fine — the linker's type check
  decides, not the encoder (proved with `.validate(false)`). The matched half: an
  unmodified-world dummy component is admitted and its first callback's fault is
  reported as a guest `Trap` with the instance retired. This required a real fix:
  `HostError::Instantiate` was **unreachable** — `PluginInstance::instantiate`
  funnelled every failure through `classify` into `Trap`, so a package that did
  not match the contract was reported as a misbehaving guest
  (`src/instance.rs`). Non-`Trap` instantiation failures now map to `Instantiate`;
  a guest initializer fault stays `Trap`/`Budget`.
- **Baseline** (`tests/host_baseline.rs` + `.analysis/codex-logs/p0-bounds/README.md`):
  one callback p50 11.7 µs / p95 12.5 / p99 17.4 (250 iterations, 1 event, debug,
  i5-12400), fixture compile 4.4 s, process `VmHWM` 8.7 MB → 46 MB for one hosted
  instance. These are a P7 comparison reference, not an acceptance; the fuel and
  memory numbers in `limits.rs` are still **not** calibrated against them.
- **One fixture build** (`tests/fixture/mod.rs`): the guest build and its
  component encoding now exist once, shared by every test binary, instead of four
  copies. `host_runtime` gained a deployment-level case for the unpublished batch
  (a guest that traps is retired and loses the routes it can no longer answer,
  with zero commands submitted).

**Two pre-existing gate blockers on this route, fixed here.** `harness run fmt`
was **red** on 18 files of this route's uncommitted work (the crate was never
formatted; `cargo fmt --all`, no semantic change) and `harness run code-health`
was **red** on `ScriptBatchSubmissionError` missing `#[non_exhaustive]` (fixed,
plus the wildcard policy in `host.rs`: an unknown refusal variant is treated as
backpressure, because retiring an instance is the destructive answer).

**Gates.** `cargo test -p mc-plugin-host`: 43 tests in 8 binaries, 0 failed.
`harness run fmt` PASS `20260916T074511-fmt-xzbh_llj`; `harness run code-health`
PASS `20260916T074611-code-health-8dnybcsy`. L2 `correctness` was **not** run for
this checkpoint.

**Not proven, and not to be implied.** No byte-level measurement of host
allocations during lifting (the obvious instrument is a counting
`#[global_allocator]`, which needs `unsafe impl` and the workspace forbids unsafe
code — the plan says not to weaken that); address aliasing is argued, not
reproduced (a Rust guest cannot alias two live strings; the cumulative charge
covers it); post-return/cleanup hostility; a fault inside a component initializer
(no way to build one with `dummy_module`); AArch64; aggregate instance/table
limits beyond the memory-growth and stack cases.

**Checkpoint state (no commit authorization).**

```yaml
base_tree: b676669c02bcb4b4d5d886aa6f3b19ed2b072f17
diff_hash: 9f04fd7381ad569a7d8c5b96db3240692e7971d14b2848e3317ff4f6d9d29f45
paths: [crates/mc-plugin-host, crates/mc-script/wit, sdk/rust, docs/PLUGINS.md, .gitignore]
validation: [cargo test -p mc-plugin-host 43/0, harness run fmt PASS, harness run code-health PASS]
next: land the two in-flight lanes, then wire serve()
```

The digest is SHA-256 over `git diff -- <paths>` followed by the path and bytes of
each untracked file under those paths, sorted; recompute it the same way (the
recipe is the seven lines this checkpoint used) because the two in-flight lanes
were started after it and will move `crates/mc-plugin-host/src/{lib.rs,check.rs}`
and `crates/mc-net`.

`.gitignore` gained `/sdk/rust/target/` in the same checkpoint: the guest SDK is
its own workspace and its build output was untracked-but-unignored, which made
`git status -uall` report 1376 fixture artifacts. `sdk/rust/Cargo.lock` stays
untracked on purpose — whether the guest lockfile is committed is P7's call (the
plan asks for reproducible fixture builds and says to account for guest lockfiles
separately).

## Composition root: the component host runs in the server (2026-09-16, tree dirty, no commit authorization)

**Outcome.** A `[plugins] runtime = "wasm"` deployment is prepared, started, bound
and stopped by `serve()`, and the rules its `configure` produced are the rules the
world is opened with. The Luau path is unchanged.

**Three lanes, disjoint write sets, one integration owner.**

1. **Startup contribution** (`crates/mc-script/src/gameplay_rules.rs` new,
   `crates/mc-plugin-host/src/startup.rs` new): the startup-rule payload and its
   validator moved out of `mc-script`'s `lua-runtime` gate to the crate root (a
   startup contract is not a Luau detail), and a WIT `rule-plan` now converts into
   it with one typed refusal per field — a value wider than its contract field is
   refused rather than truncated, because a truncated rule set would fingerprint
   as a different world. `PluginHost::contribution()` reports per package
   `NoPlan | Rules | Refused`, and `check_deployment` runs the same conversion and
   validation the run path does, so a plan cannot pass one and fail the other.
   Falsified twice by hand: removing the shared `validate()` fails four cases;
   truncating instead of refusing fails the width case.
2. **Session lookup** (`crates/mc-net/src/server.rs` and the session module):
   `mc_net::PlayerSessionsHandle` keeps no table of its own — it holds a
   `Weak<SessionRegistry>` that `BoundServer::register_player_sessions` publishes,
   and delegates to the registry `list-online-players` already answers from, so a
   plugin's uuid-addressed command reaches the connection that identity holds and
   an offline player is refused instead of addressed by a stale runtime id.
3. **Wiring** (mine, `crates/mc-server/src/main.rs`):
   `prepare_configured_plugins` dispatches on the configured runtime; the component
   deployment is discovered through the *same* `component_deployment` constructor
   `--check` uses (strict/expected/grants unchanged), the host starts **before**
   the world is opened because that is where the rules come from, a refused plan or
   two packages that each declare rules stops startup with the host already
   stopped, `bind_with_scripts` binds the host's own boundary, and the server
   registers the session handle after binding.

**Evidence on this tree.** New mc-server cases: the deployment prepares, starts and
really claims its command (`boundary().player_command_roots() == ["hello"]`); the
rules carry the package's own values; a package with no plan contributes none; two
declarers refuse the deployment naming both. Existing suites: `mc-plugin-host`
49/0 (10 binaries), `mc-script` 130/0 and 267/0 with `lua-runtime`, `mc-net`
session-filtered 605/0 plus the 2 new lookup cases, `mc-server` 237/0 + the 3 new
cases. `harness run fmt` PASS, `harness run code-health` PASS (`0 fail`,
`verdict: KEEP`), workspace `clippy -D warnings` clean (it was **not** clean before
this wave: four findings in this route's crate, three in `mc-plugin-host` and one
in `startup_contribution`'s test binary, all fixed).

**Named limits of this stage, stated rather than implied.** A component deployment
declares no ore profile, settlement plan or client bundle — the WIT contract has no
record for them — so the world contract records what a server with no plugin
directory records and `serve()` logs that fact; SIGHUP reload remains Luau-only
until P6, and a component deployment logs that the reload was ignored rather than
reporting one; `PluginLimits`/`HostQueues` are still the documented defaults (P7
measures them). The fixture-build recipe for the SDK guest now exists in two places
(`crates/mc-plugin-host/tests/fixture/mod.rs` and mc-server's test module); P7 owns
centralizing it in the harness.

**The L2 gate, honestly — red on one pre-existing test, four receipts.** The first
two `harness run correctness` attempts never reached a verdict: `systemd-oomd`
killed the run's scope before its test phase finished (machine protection, not a
test result; see the machine-bounds section). Run with
`SOLARIS_HARNESS_MEMORY_HIGH=off SOLARIS_HARNESS_MEMORY_MAX=10G`, the `test` phase
completed twice — `20260916T084138-test-ct64wtlj` (458.7 s) and
`20260916T085042-test-vz5yi4m0` (241.1 s, scope peak 1.13 GiB) — and both failed on
exactly one test, the same one:
`settlement_pause_repro::settlement_fund_reserves_materials_and_answers_the_player`.
68 of 69 test binaries were green in the second run; `cargo test -p
mc-test-harness --test settlement_pause_repro` passes standalone in 3.83 s, and so
does the same command **inside the identical scope properties** (`systemd-run
--user --scope -p CPUQuota=600% -p MemoryHigh=3G -p MemoryMax=4G -p
MemorySwapMax=1G env RUST_TEST_THREADS=6`), so the quota is not the trigger — the
accumulated load of a full workspace run is. The failure is the signature CP-002
already recorded (`["Unknown command"]`), and its stated residual is the owner:
the metric behind that wall-clock slice. **L2 is therefore not green on this
tree**, nothing in this wave is in that path, and the receipts above are the
evidence rather than a claim that the gate passed.

**Checkpoint state (no commit authorization).**

```yaml
base_tree: b676669c02bcb4b4d5d886aa6f3b19ed2b072f17
diff_hash: aaf3dc156af854655a868cb6814c119b06bc4b0603e7dc155b299d49dbec865a
paths: [crates/mc-plugin-host, crates/mc-script/wit, crates/mc-script/src/gameplay_rules.rs,
        crates/mc-script/Cargo.toml, crates/mc-server, crates/mc-net/src/server.rs,
        sdk/rust, tools/harness/__main__.py, docs/PLUGINS.md, .gitignore]
validation: [mc-plugin-host 49/0, mc-script 130/0 and 267/0, mc-net session 605/0 + 2, mc-server 237/0 + 3 new,
             harness run fmt PASS 20260916T085637, harness run code-health PASS 20260916T085641,
             L2 test phase RED on settlement_pause_repro only]
next: P3, first vertical in flight
```

Recompute it the same way: SHA-256 over `git diff -- <paths>` followed by the path
and bytes of each untracked file under those paths, sorted. `docs/MEMORY.md` is
excluded on purpose (it is this cursor). The P3 wave that starts next moves
`crates/mc-script/wit` and `crates/mc-plugin-host/src/adapter.rs`, so this digest
describes the tree as of this closeout, not the tree in flight.

## Historical checkpoint — WASM audit, zone completion coverage, Loader reconciliation (2026-09-16)

**Route:** `plugins`. Owning plan:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`; owning ADR:
[`0009-regional-plugin-boundary.md`](decisions/0009-regional-plugin-boundary.md).
Core base: `62ea32a37e740f1559355056bde2558d3a6b5c4b`.
Earlier sections are historical evidence, not the current phase acceptance.

### Actual migration state

| Phase | Current result and remaining acceptance |
| --- | --- |
| P0 | Host, SDK component, pre-instantiation limits and hostile-guest tests exist; the earlier safety evidence is recorded above. This checkpoint does not refresh the whole safety/performance baseline. |
| P1 | **Partial, not accepted.** Default `mc-script` is VM-free, but its optional `lua-runtime` still owns `mlua`/`luaur`; `mc-server` and `mc-net` enable it. `cargo tree -p mc-server -i mlua --depth 3` confirms the dependency through `mc-script`. This does not satisfy the plan's requirement to isolate retained Luau outside the runtime-independent contract crate. |
| P2 | Discovery, grants, bounded host and server composition root are implemented. **Graphical hello/join acceptance remains unrecorded**; the prior blanket “P2 done” was too strong. R0's separately deferred acceptance does not waive P2's gate. |
| P3 | Storage batch CAS/operation status (S1), teleport (S3), and zone commands/results (S2) are implemented. This checkpoint adds S2 runtime regression coverage. Menus, inventory transactions, remaining used resident/settlement operations, config/timers and Loader views remain to migrate. |
| P4 | Basic configure/startup rules are wired; ore profile, settlement plan and Loader/client metadata remain incomplete. |
| P5–P8 | Precommit hooks, component reload, first-party package migration and final Luau removal remain open. |

**Correction to the interrupted-S2 note:** the zone answer mapping and the guest's
zone fixture were already in committed `62ea32a`; only the dedicated regression
coverage was missing. No new zone production implementation was needed or added.
The old session-local `local://p3-operation-matrix.md` is not available here;
do not treat that URI as recoverable evidence.

### Completed outcome: zone result delivery

`crates/mc-plugin-host/tests/zone_operations.rs` now drives real Rust components
through the real host and admission boundary:

- Two plugins use the same zone ids. Applied/refused results, including a refused
  removal, reach only the issuing plugin and preserve the owner's one-bit answer.
- An ungranted zone batch publishes no command and retires its package.
- Closing event admission drains queued results and closes the command channel;
  success never depends on a timed silence check.

The test supplies owner completion events through `ScriptPluginTarget`; it is
not a graphical gameplay run or a combined live-server zone-owner test.
The existing zone-owner suite was exercised separately: capacity refusal,
protection, ownership, membership, no-op removal and shutdown behavior.
Zone entry/exit observations are still absent from WIT.

### Completed outcome: Loader local/main conflict

Sibling `../solaris-loader/main` fast-forwarded from `3aaa926` to existing
`origin/main` `0972926e4431ee597cb4903abc43e96872acd732`, without a new commit,
push, stash or reset. The old local wire-3/schema-2 draft overlapped the upstream
implementation. Its full input remains in
`../solaris-loader/.analysis/loader-reconcile/` (snapshot, patch and trace report).
The distinct local `bridge-core/.../ClientCommandsTest.java` class-name correction
is retained and uncommitted; caches and local configuration were untouched.

The old draft's unused preview projection math was not restored. Its input-mode
code only reselected already-open views; the upstream modal screen owns that
interaction. The trace also found a pre-existing gap on both sides: no production
key binding emits `view_request{settlement|army}`. That remains Loader/P4 work,
not functionality completed by this conflict resolution.

### Evidence and boundaries

Receipts: `.analysis/codex-logs/wasm-zones-loader-20260916/`.

- `cargo test -p mc-plugin-host --test zone_operations`: **2 passed**.
- `cargo test -p mc-net --lib script::zone_tests`: **16 passed**.
- Scoped host Clippy with `-D warnings`: **passed**.
- Harness `fmt`: **passed**, `20260916T120851-fmt-m_t5ubqm`.
- Harness `code-health`: **passed**, `20260916T120855-code-health-vq1tkvaz`.
- Harness `java`: **passed**, `20260916T120050-java-r2vs3sah`; all four Loader
  modules compiled/tested (**95 tests, zero failures/errors/skips**);
  bridge/java-agent tasks were up-to-date.
- Harness `fixture-check`: **passed**, `20260916T120130-fixture-check-jv0bzbpo`.
- CodeGraph sync: completed. Independent/negative-code review found one vacuous
  retirement assertion: closing admission already clears routes. Fixed by queuing
  a second callback and proving only one batch is refused; the test no longer
  uses shutdown-cleared routes as evidence. Final 2-case suite, scoped Clippy and
  file-format check passed after this fix. No production/scope defect was found.
- L2 `correctness` and graphical clients **not run**. The previously recorded
  full-workspace `settlement_pause_repro` red remains unresolved; no overall
  green or completed P1/P2/P3 acceptance is claimed.

The initial large generated test suite was replaced with two focused behavioral
regressions; duplicate DTO-forwarding checks and timed-silence assertions were
removed. No production Rust code or Cargo dependency was changed.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
diff_hash: fbf85c7ea34919bf3b4f69fa6ea66e4f6c66b473dd1a55030acc895f290a3e27
changed_files:
  - crates/mc-plugin-host/tests/zone_operations.rs
  - docs/PLUGINS.md
  - docs/decisions/0009-regional-plugin-boundary.md
  - docs/MEMORY.md
validation: [zones_2_passed, zone_owner_16_passed, host_clippy_passed, fmt_passed, code_health_passed, java_passed, fixture_check_passed]
next: Complete P1 runtime separation so mc-script has no VM dependency, preserve retained Luau behavior outside that boundary, and prove both deployment paths still build and run.
```

`diff_hash` hashes `owned.patch` in the receipt directory: the scoped tracked diff
plus the new zone test, excluding this cursor to avoid a self-referential hash.
Unrelated pre-existing analysis-file changes remain untouched. No new commits
were created. Before further P3 API expansion, close the earliest unmet P1
dependency contract; then obtain P2's actual hello/join client evidence.

## Historical checkpoint — P1 contract/runtime separation accepted (2026-09-16)

**Route:** `plugins`. Owning plan:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`; owning ADR:
[`0009-regional-plugin-boundary.md`](decisions/0009-regional-plugin-boundary.md).
Base: `62ea32a37e740f1559355056bde2558d3a6b5c4b`.
The persistent objective remains the entire plan, not just this checkpoint.
All 105 plan items are tracked; this checkpoint closes P1 only.

### Completed outcome

`mc-script` now owns the runtime-independent boundary, contract and shared
deployment/client/startup metadata. It declares no VM feature or VM dependency.
Retained Luau moved into the existing `mc-plugin-host::legacy_luau` module behind
the migration-only `legacy-luau` feature; no second host crate was created.
`mc-server` selects that feature at composition. Production `mc-net` has no host
or VM dependency; its integration tests select the host as a dev-dependency.
All old runtime callers moved; metadata callers remain on `mc-script`.

Reload still shares the original bounded FIFO with events. A narrow
`ScriptHostInputSender` moves a trusted, opaque host-only payload; no Tokio sender,
guest control API, second mailbox or generic runtime provider was added.
`commit_reload` retains admission checks, route replacement, swap and publication
ordering. Component reload remains unimplemented P6 work.

Metadata fields and discovery checks were preserved. Constructors assemble
trusted already-validated metadata; they do not claim to validate arbitrary
input. WASM client/worldgen metadata is still incomplete. WIT, SDK, gameplay
owners, storage formats and runtime budgets were not changed.

### Evidence

Receipts and exact changed-file list:
`.analysis/codex-logs/wasm-p1-separation-20260916/checkpoint.json`.

- Linux x86_64 Cargo graph, all features and normal/build edges: no Wasmtime,
  `mlua` or `luaur` below `mc-script`; no `mc-net` dependency below the host;
  no production VM dependency below `mc-net`. The server retains Luau through
  `mc-plugin-host`, not `mc-script`. `dependency-boundary.json` records the check.
- The standalone Rust SDK guest built successfully for
  `wasm32-unknown-unknown`; a real component was encoded from that artifact.
  The temporary packager source was removed after the smoke.
- Actual `cargo run --locked --bin mc-server -- --check --config ...` passed
  for isolated WASM and Luau deployments. Neither created its world directory.
  This is deployment/check-mode evidence, not client gameplay.
- Workspace strict Clippy passed:
  `20260916T131448-clippy-21rce3dk`.
- Full `cargo test --workspace --all-targets` passed through the canonical
  harness: **5,034 passed, zero failed, 192 ignored**,
  `20260916T132433-test-fsq02t7w`. This includes **138 `mc-script` tests** and
  **135 retained-Luau host unit tests**, plus component integration tests.
- Standalone default-feature `mc-plugin-host` suite: **63 passed, zero failed
  or ignored**. This separately exercises the real component host without
  workspace feature unification selecting Luau.
- Independent static/negative-code review found one lost runtime regression:
  moving the poisoned-ledger test had retained boundary rejection coverage but
  lost the host's once-only `CommandAdmission` disable diagnostic. Restored the
  host test using public APIs to saturate outstanding admissions, with no public
  test hook. Two real Luau ticks produce one diagnostic, zero enabled plugins,
  and normal shutdown. The restored test passed after the workspace run;
  host all-target Clippy with `legacy-luau` and `-D warnings` also passed.
  No production defect or additional negative-code finding was reported.
- Final formatter passed: `20260916T133457-fmt-i9c9lxs1`.
  Final code-health passed: `20260916T133501-code-health-pyo1569o`.
  CodeGraph sync completed; updated documentation link targets exist.

The initial correctness attempt failed on a leaked edit-tool token and an
unused test-only import; both were fixed before the successful workspace Clippy.
The first workspace test attempt then exposed an incorrect new saturation-fence
assertion against a raw command instead of its admitted request. Fixed via the
real admission API; the exact reproduction and full workspace rerun passed.
These failed receipts are retained, not relabeled green.

The historical load-sensitive `settlement_pause_repro` passed in this complete
workspace run. No timeout or VM wall-clock budget was widened, and this is not
evidence that its load sensitivity was fixed. Ignored/local-parity tests and
graphical clients were not run. No performance or fresh AArch64 claim is made.

### Remaining plan state

P0's earlier safety evidence is inherited, not refreshed. P1 is accepted.
P2 has host/server code and headless coverage, but its required real-client
join/`/hello` acceptance is still unrecorded. P3 has storage batch/status,
teleport and zone commands/results; remaining used operations are open.
P4–P8 and the later CP queue remain open. This is still **draft** work.

The preceding checkpoint's zone regression coverage and Loader reconciliation
are preserved. Loader remains at existing upstream
`0972926e4431ee597cb4903abc43e96872acd732`, with the separate uncommitted test
class-name correction retained. No new Loader changes were made here.
Missing key bindings for `view_request{settlement|army}` remain P4 work.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
diff_hash: 6d83507505a17d683ee74cdb90a75b0a4a4d47b6af88e7a2763472691fad03be
changed_files: .analysis/codex-logs/wasm-p1-separation-20260916/checkpoint.json#/changed_files
validation: [workspace_test_passed, workspace_clippy_passed, review_fix_test_passed, host_clippy_passed, default_wasm_host_passed, standalone_sdk_passed, both_cli_checks_passed, fmt_passed, code_health_passed]
next: Close P2 with real graphical vanilla-client join and /hello from external components, strict-startup rejection and no-world check-mode evidence.
```

`diff_hash` covers the saved `owned.patch` against the base, including preserved
prior-checkpoint edits and new files; this cursor is excluded to avoid a
self-referential hash. No commit or push was authorized or performed. Unrelated
dirty analysis files remain untouched.

## Historical checkpoint — P3 durable storage compatibility accepted (2026-09-16)

**Route:** `plugins`; primary document: `docs/PLUGINS.md`. Owning plan:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.
P0–P2 remain accepted. This checkpoint closes **storage compatibility**, not
all P3: the full **106-task** plan still has **3 completed / 103 incomplete**,
with P3 in progress. The persistent goal remains active; maturity is **draft**.

### Completed outcome and evidence

- A real Luau deployment wrote a standalone CAS record at revision 1 and a
  durable batch at revision 2. A WASM deployment with the same plugin id read
  both, replayed the legacy operation under a fresh request id, and received
  `operation-conflict` for substituted content.
- The component committed its own batch at revision 3. Repeating it with fresh
  request IDs and reopening the world preserved that revision and value,
  rather than applying another mutation. Both old/new operation receipts
  remained queryable; another plugin saw neither records nor receipts.
- These are the existing storage owners, namespace, journal and receipt paths.
  No kernel, production-host, WIT or public SDK API change was needed. Added
  a real SDK guest fixture and one real-owner integration regression, not
  hand-built result events or journal bytes. The owning ADR is intentionally
  unchanged.
- Actual CLI smoke used **five separate `mc-server` processes**: legacy seed,
  WASM migrate, restart, foreign-owner probe, and final-component restart
  verification after cleanup. Every process exited 0; the logs contain no
  plugin errors/warnings. Legacy seeding used a real protocol client. No new
  graphical or fault-injected power-loss/DurabilityUnknown gate is claimed.
  Evidence: `.analysis/codex-logs/wasm-p3-storage-20260916/smoke.json`.
- Canonical `correctness` passed formatter, strict workspace Clippy,
  code-health and workspace/all-target tests:
  **5056 passed, 0 failed, 192 ignored**.
  Receipt:
  `.analysis/validation/20260916T180535-correctness-h_teuzib/result.json`.
  After a final guest-only cleanup removed response-vector aggregation and
  asserted its existing single-outstanding-response invariant, the SDK format
  check, real-owner regression and actual-server restart replay all passed.
  A subsequent fail-fast Luau fixture-load prerequisite passed root fmt,
  targeted strict Clippy and the real-owner regression again. Production
  host/kernel and public APIs did not change; CodeGraph synced.
- Independent read-only review: **pass, no actionable findings**, including the
  negative-code pass. The narrow final fixture cleanup was self-checked and
  exercised as above; no second reviewer wave.

### Preserved boundaries and failure history

The first strict CLI check failed because the prepared server configuration
omitted operator grants. Explicit `[plugins.grants.<id>]` entries corrected it;
strict discovery stayed enabled. Both denied and granted checks left world
state absent. Component manifests omit the Luau-only `required_features`
field; the operator instructions now show the actual grant shape.

The earlier timer checkpoint's catalog-wire timeout remains unexplained,
**not fixed by this work**. This checkpoint's workspace run passed that test;
the earlier failed receipt remains preserved in the timer checkpoint.

No commit or push was authorized or performed. Inherited P1/P2/timer changes
and prior Loader reconciliation are preserved. No new Loader or sibling source
changes. Temporary smoke-client source was moved into ignored evidence, its
temporary executables removed, and all owned processes/agents closed.

### Remaining work and next bounded outcome

P3 still needs the remaining used inventory, query, entity/world and
resident/settlement/order verticals and their acceptance boundaries. Next:
**WASM inventory/storage purchase/refund transactions through the existing
session/coordinator/storage owners**, preserving typed outcomes, inventory
revision, item-component conservation and existing receipts/outbox/fences,
without a new journal. Current consumer evidence is
`../solaris-default-plugins/basic-economy/main.lua:516`.
Use a real WASM caller and actual-client evidence for inventory-visible effects.

The historical `local://p3-operation-matrix.md` is unavailable; do not chase it
or invent its rows. Start with bounded discovery of that current consumer and
the existing transaction/result/durability path. Storage scan/delete were not
found in `solaris-settlements`; they were not added speculatively.

The queued **TUI dashboard** remains pending: TPS/MSPT/percentiles, CPU/memory,
players/entities/chunks, **worldgen chunks/s, ms/chunk and backlog**, and useful
chunk I/O, persistence and plugin metrics; readable layout and keyboard
navigation. Verify the actual running TUI with active world generation and
mark unavailable metrics explicitly. The storage smoke records an existing
`calls=0` / `events_delivered=8` diagnostic observation: establish counter
semantics before using it as guest-call telemetry.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
diff_hash: 7f85bad032f9f63460ae7549fd257acd3464b7be1cfba2ab0e03ab1649c9f628
changed_files: .analysis/codex-logs/wasm-p3-storage-20260916/checkpoint.json#/changed_files
validation: [correctness_passed, final_component_regression_passed, sdk_fmt_passed, actual_server_migration_restart_isolation_passed, strict_grants_checked, independent_review_passed, codegraph_synced]
next: P3 inventory/storage transaction vertical through the existing owners, with real WASM and actual-client acceptance.
```

[Checkpoint evidence](../.analysis/codex-logs/wasm-p3-storage-20260916/checkpoint.json)
contains commands, snapshots and the complete plan state.
`owned.patch` is this storage delta against saved post-timer working-tree
copies, excluding this self-referential cursor. `lock-baseline.json` records
the derived baseline for the one newly added test dependency edge.
The [timer checkpoint](../.analysis/codex-logs/wasm-p3-timers-20260916/checkpoint.json)
retains the preceding capability and its inherited evidence.

## Historical checkpoint — P3 inventory/storage transactions accepted (2026-09-16)

**Route:** `plugins`; primary document: `docs/PLUGINS.md`. Owning plan:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.
This checkpoint closes the inventory/storage vertical, **not all P3**.
The full 106-task plan remains **3 completed / 103 incomplete**, with P3 in
progress. The persistent goal remains active; maturity is **draft**.

### Completed outcome

- WIT and the Rust SDK expose a session-bound inventory/storage command and
  typed committed/refused answer through the existing capability, DTO validators,
  session gate and durable coordinator. This compound operation has **no durable
  operation-id receipt**: `request` is correlation only and may be reused after
  completion. Uncertain durability produces no result for the current command;
  silence is not a refusal or permission to retry.
- Three real-component regressions cover owner/disk purchase and refund,
  insufficient inventory, stale CAS/session, reconnect, item-component
  conservation, missing capability and malformed whole-batch refusal. A legal
  multi-key transaction reproduces and guards the fixed **per-string versus
  aggregate text-bound** error; the pre-lift whole-answer budget remains intact.
- The canonical graphical regression passed with a real no-Solaris-Loader
  26.1.2 client under Xvfb: two purchases, insufficient-inventory refusal, refund,
  stale-CAS refusal, reconnect, old-session refusal and a successful purchase
  from the new live session. Ledger markers and exact inventory counts agree.
  A pickaxe damaged by actual survival interaction retains its damage and
  observable components. Main visually inspected all six inventory screenshots.
  Setup used explicit operator commands; this is not no-debug survival.
- The actual client exposed native debug stack exhaustion on ordinary
  `select_hotbar_item`/container-click ingress. A GDB trace localized it to the
  large click future; boxing that future in `play.rs` fixes the reproduced path
  without increasing thread stacks or changing inventory authority.
- Harness stdout retains its original **INFO-only** contract; `main.rs` is
  restored to its checkpoint baseline. Only the existing `play session
  unregistered` event is promoted to INFO, so reconnect does not require DEBUG
  capture or the removed save-log string. Unregister does **not** assert
  asynchronous save completion; reconnect and native disk checks provide
  separate persistence evidence. Two mocked reconnect tests that did not defend
  waiting behavior remain removed; the actual INFO-only scenario supplies proof.

### Verification and preserved failures

- Real graphical pass:
  `.analysis/validation/20260916T212003-regression-7bycs290/result.json`.
  Structured phase outcomes, inventory/component observations, adversarial
  refusals and screenshots are indexed by
  `.analysis/codex-logs/wasm-p3-inventory-20260916/graphical-acceptance.json`.
- Final logging revision passed canonical **`run correctness`**:
  `.analysis/validation/20260916T212046-correctness-2g_2o0mw/result.json`.
  Formatter, strict workspace Clippy, code-health and workspace/all-target tests
  passed: **5057 passed, 0 failed, 192 ignored**. Harness-check and SDK checks
  also passed; CodeGraph synced. The final graphical run used `RUST_LOG=info`,
  captured both actual session-release events at INFO and no DEBUG stdout rows.
  The earlier `correctness` attempt stopped on Clippy's nested-if diagnostic;
  an earlier workspace run caught an incorrect test control assertion
  (`Broadcast` versus the real guest's `SendMessage`). Those receipts remain
  failed, not relabeled. The intermediate individual-gate pass and its source
  snapshot are retained as prior-revision evidence.
- Independent read-only review found no proven source defect in the initial
  patch, but its graphical run was **blocked** by the stack overflow. Main
  diagnosed the subsequent failures, made the fixes and ran the final gates.
  The later fixes were not independently re-reviewed; no second reviewer wave.
  Original review and each failed receipt remain in the checkpoint evidence.
- One intermediate run timed out on chunk delivery; its cause is **unresolved**,
  not fixed by this work. Later runs, including the full accepted scenario,
  loaded the chunks. The earlier timer catalog-wire timeout also remains
  recorded, not claimed fixed.
- Graphical observation does not expose arbitrary item components or storage
  revisions; native coverage supplies those checks. This checkpoint does not
  claim a server-process restart, crash-injected or durability-unknown recovery
  test for the compound operation. Existing storage restart evidence is retained.

### Next bounded outcome

**WASM market inventory-menu open/action/close through the existing owners**,
using the current `basic-economy` consumer
(`../solaris-default-plugins/basic-economy/main.lua:306,381,515–516`).
Acceptance needs a real component and actual-client purchase/refund through
the menu, component conservation, and stale-menu/session refusals. Do not add a
second transaction authority or journal. Other used inventory/query,
entity/world and resident/settlement/order verticals still keep P3 open.

The queued **TUI dashboard** remains pending: TPS/MSPT/percentiles, CPU/memory,
players/entities/chunks, **worldgen chunks/s, ms/chunk and backlog**, useful
chunk I/O, persistence and plugin metrics, readable keyboard navigation and
actual-runtime verification. Mark unavailable metrics explicitly; establish
guest-call counter semantics before displaying them.

No commit or push was authorized or performed. Inherited work and Loader
reconciliation remain intact; no sibling source changed. There was no dependency
or lockfile change in this checkpoint. Temporary diagnostic scripts were removed
after archiving their source, and all owned processes/agents are closed.
The owning ADR is intentionally unchanged: existing domain owners, persistence
ordering and transaction policy remain authoritative.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
diff_hash: 830d7c417501b119543c6a8cc0ef1b4b88ddbc8175ce64bd3846add2bd46f2bf
changed_files: .analysis/codex-logs/wasm-p3-inventory-20260916/checkpoint.json#/changed_files
validation: [info_only_graphical_regression_passed, real_component_native_passed, correctness_passed, harness_check_passed, sdk_fmt_passed, initial_independent_review_blocked_then_main_fixed_and_verified, codegraph_synced]
next: P3 WASM market menu open/action/close through existing owners, with real-component and actual-client purchase/refund and stale-context refusal evidence.
```

[Checkpoint evidence](../.analysis/codex-logs/wasm-p3-inventory-20260916/checkpoint.json)
contains commands, source fingerprints, review, failure history and the next
cursor. `owned.patch` is the inventory/storage delta against saved post-storage
working-tree copies, excluding this self-referential cursor.
The [storage checkpoint](../.analysis/codex-logs/wasm-p3-storage-20260916/checkpoint.json)
retains the previous capability and its inherited evidence.

## Historical checkpoint — P3 WASM inventory menus accepted (2026-09-16)

**Route:** `plugins`; primary document: `docs/PLUGINS.md`. Owning plan:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.
This closes the menu open/action/close vertical, **not all P3**. The full plan
remains **3 completed / 103 incomplete**, P3 in progress; the persistent goal
remains active and maturity is **draft**.

### Completed outcome

- WIT/SDK expose session-bound menu open/close and owner-targeted typed clicks.
  Clicks carry the original session separately from the player's UUID.
  Existing capability checks, DTO validators, session/menu owners and compound
  inventory/storage authority remain in charge. Open/close are fire-and-forget;
  request markers are not successful-effect acknowledgements.
- The real component fixture opens a market, buys/refunds through its existing
  ledger transaction, and closes via either a button or command. Native wire
  coverage exercises all four click kinds, exact title/button content, a second
  subscribed component proving owner-only delivery, missing grants, malformed
  batches, stale window/revision/menu/session contexts, reconnect and persisted
  named/enchanted/damaged item conservation.
- Final real 26.1.2 no-Solaris-Loader graphical runs passed for both the new menu
  route and the existing command-only trade route. Menu coverage includes two
  purchases, insufficient-inventory refusal, refund, both close paths, reconnect,
  stale requests against a kept live window and a subsequent successful click.
  Every real click was confirmed; no timeout or bridge error is treated as a
  pass. Main visually reviewed all **25 final UI captures** through an indexed
  contact sheet, plus initial full-frame evidence.
- Main removed speculative tolerance of unconfirmed bridge clicks, scoped the
  observer fixture to its own mode, and changed stale wire probes to the
  opposite trade action so a rejected click cannot be falsely “proved” by an
  indistinguishable accepted purchase.
- Independent read-only review found one fixture defect: a stale-session
  transaction forgot the live menu while sending its close to the dead session.
  Main reproduced the missing close frame despite a real refusal marker, fixed
  closing to use the tracked menu's own session, and reran the exact regression
  successfully. The regression also reopens and trades afterwards. No second
  reviewer wave; the final fix and private test-helper cleanup are Main-verified.

### Verification and boundaries

- Final menu graphical receipt:
  `.analysis/validation/20260916T224734-regression-b6qptme7/result.json`.
- Final existing command-route receipt:
  `.analysis/validation/20260916T224817-regression-ptyy1v9e/result.json`.
- Full **L2 scope passed through named gates**: formatter, strict workspace
  Clippy, code-health and workspace/all-target tests. Test receipt:
  `.analysis/validation/20260916T223647-test-5yw6tdti/result.json` —
  **5060 passed, 0 failed, 192 ignored**. Harness-check and SDK formatting passed;
  CodeGraph was synchronized.
- The initial canonical `correctness` attempt remains **failed** at Clippy's
  nine-argument private click helper. Main removed redundant window claims and
  grouped existing verification inputs without new state or allocation, then
  ran the failed and remaining gates individually. This is not a claim that the
  failed canonical receipt turned green.
- The review reproduction remains **failed** before the fix; its exact-command
  post-fix run passed. Both are preserved in
  `.analysis/codex-logs/wasm-p3-menu-20260916/review.json`.
- GUI setup used explicit operator commands with seed 81, not no-debug survival.
  Native tests, not MCP, cover modified clicks and forged packets. GUI checks
  actual windows, exact counts and exposed tool components; native checks cover
  richer components. Neither menu commands nor the older compound transaction
  gain a durable operation-id receipt, new journal or crash-recovery guarantee.
- The earlier chunk-delivery and timer catalog-wire timeouts remain unresolved,
  not fixed by this checkpoint. Prior blocked owner/manual gameplay observations
  are not downgraded by these gates.

### Next bounded outcome

**WASM zone entry/exit drives market open/close through the existing owners.**
The live consumer is
`../solaris-default-plugins/basic-economy/main.lua:358–381`; it requests the
catalog on entry and clears pending state/closes its menu on exit. WIT zone
mutations exist, but those observations are still missing. Acceptance needs a
real component and actual client crossing zone boundaries, owner-only event
delivery with original sessions, menu effects, and existing dimension/disconnect
behavior. Do not introduce a second zone observer or menu authority.

The queued **TUI dashboard** remains pending: TPS/MSPT/percentiles, CPU/memory,
players/entities/chunks, **worldgen chunks/s, ms/chunk and backlog**, useful
chunk I/O, persistence and plugin metrics, readable keyboard navigation and
actual-runtime verification. Mark unavailable metrics explicitly; establish
guest-call counter semantics before displaying them.

No commit or push was authorized or performed. Inherited work and Loader
reconciliation remain intact; no sibling source was changed. No dependency or
lockfile change was needed. Owned agents are closed; canonical harness runs
completed their process cleanup. The owning ADR is intentionally unchanged:
existing domain owners, persistence ordering and transaction policy are retained.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
diff_hash: 19c13dda7da3bf5bbe229d96af072a6cee48563714e478b72c182dce83eee28c
changed_files: .analysis/codex-logs/wasm-p3-menu-20260916/checkpoint.json#/changed_files
validation: [final_menu_graphical_passed, final_command_graphical_passed, real_component_native_passed, l2_named_gates_passed, harness_check_passed, sdk_fmt_passed, independent_finding_reproduced_and_fixed, codegraph_synced]
next: P3 WASM zone entry/exit-driven market open/close, with real-component and actual-client boundary-crossing evidence through existing owners.
```

[Checkpoint evidence](../.analysis/codex-logs/wasm-p3-menu-20260916/checkpoint.json)
indexes exact commands, receipts, source fingerprints and review. `owned.patch`
is the menu delta against saved post-inventory working-tree copies, excluding
this self-referential cursor; `after/` holds the accepted source snapshot.

## Historical checkpoint — P4 accepted (2026-09-17)

**Route:** `plugins`; primary document: `docs/PLUGINS.md`. Owning plan:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.
P0–P4 are accepted: **5 completed / 101 incomplete** in the full TODO.
The persistent goal remains active; the owner's full-plan instruction and
predominantly Task-agent implementation remain in force. Maturity: **draft**.

### Completed outcome and exact evidence

- `configure` runs in a disposable Store; `init` creates runtime state in a
  separate Store. Both frontends share strict feature, client-bundle and
  worldgen metadata parsing. Existing native startup validators and effective
  world identity remain authoritative; changing guest bytes or runtime alone
  does not invalidate the same world.
- Typed WASM view open/present/close, owned sounds and block-item grants use
  existing native owner/session/permission/hash paths. Loader wire **3** and
  artifact schema **2** are unchanged. HUDs coexist without replacing modals;
  input bindings and cleanup stay tied to their original connection.
- Real configure/init refusals were exercised in both `--check` and serving
  paths: world storage remained absent and the intended listener ports remained
  bindable. Receipts live under the phase's `startup-refusal-probes/`.
- Final L2 constituent gates passed: formatter, strict workspace Clippy,
  code-health and workspace/all-target tests. Recorded test summaries:
  **5115 passed, 0 failed, 192 ignored**. The initial `correctness` aggregate
  stopped at Clippy; its failed receipt is retained. After fixing the case-table
  annotation and large startup enum, only the failed/remaining gates were run.
  `final-native-validation.json` indexes these receipts and their exact scope.
- Final Java and harness-check profiles passed. All three real graphical
  Loader platform scenarios passed after the reviewed HUD fix:
  - Fabric: `.analysis/validation/20260917T054239-loader-live-kq5_gytm/result.json`
  - NeoForge: `.analysis/validation/20260917T054452-loader-live-37y73l4_/result.json`
  - Forge: `.analysis/validation/20260917T054713-loader-live-lsfkb8_7/result.json`
- The final no-Loader core-client scenario passed:
  `.analysis/validation/20260917T054914-core-client-_h7zhj10/result.json`.
  A real WASM greeting and `/hello` reply reached the client, its ordinary
  inventory opened, and deliberately invalid `rules.lua` was ignored. The
  existing Luau and required-client rejection scenarios were retained.
- Clients were agent-run through the canonical harness under Xvfb/MCP.
  Main inspected actual HUD/update/reconnect and no-Loader inventory images.
  Bounded adversarial coverage includes invalid input batches, owner-local
  sound stop, HUD isolation and reconnect cleanup.
- Independent read-only review found duplicate HUD opens and a lost hide while
  an open was pending. Both real-component regressions failed on the preserved
  old artifact and passed on the fixed source-built artifact. One outstanding
  open now coalesces the latest model/visibility; refusal and reconnect are
  covered too. No permanent replay override or second reviewer wave remains.
- Negative-code cleanup removed root wiring-only tests and redundant assertions;
  four world-compatibility/conflict/cleanup tests live in the focused
  `component_startup_tests.rs`. Existing architecture/API/Loader/SDK docs were
  updated; 25 relative documentation links resolve. CodeGraph is synchronized.

### Boundaries retained

R0 and full survival acceptance remain deferred; Loader compatibility is not a
survival or release claim. At 854×480 transient vanilla login toasts overlap the
HUD state column; later panels are readable. First-party package migration,
WASM reload and precommit hooks remain later phases. The stop-log `calls=0`
observation was independently confirmed pre-existing, not fixed by P4.
Earlier blocked owner/manual scenarios retain their status.

No new commit, staging or push was authorized or performed. The Loader base
remains the already reconciled `0972926`; inherited unrelated changes were
preserved. P4 writers/reviewer are closed; harness runs completed their owned
process lifecycles. The queued TUI dashboard remains in the full TODO.

### Immediate next outcome

**P5: bounded precommit hooks**, first before-build, then before-damage through
native owner tickets. Acceptance: ordered handlers, cancellation without item
or damage effects, stale/expired/duplicate rejection, queue-wide deadline,
revoked permission and fail-closed protection, plus client-visible latency
and zero-subscriber behavior. No guest execution under world/region locks.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
sibling_base_tree: 0972926e4431ee597cb4903abc43e96872acd732
diff_hash: fad77696185d5cb266825075d3185e33011cd3f0bfb933a99e330ce1bfe99caa
changed_files: .analysis/codex-logs/wasm-p4-20260917/phase-evidence.json#/changed_files
validation: [final_L2_constituents_passed, java_passed, harness_check_passed, fabric_passed, neoforge_passed, forge_passed, no_loader_core_client_passed, review_regressions_failed_before_passed_after, documentation_links_passed, codegraph_synced]
next: Complete P5 bounded before-build and before-damage hooks with native ticket, protection, cancellation and client-latency evidence.
```

[P4 phase evidence](../.analysis/codex-logs/wasm-p4-20260917/phase-evidence.json)
indexes exact receipts, review resolution, accepted component SHA-256 and
99 owned core/Loader file deltas. The hash covers canonical sorted path and
before/after hashes, excluding this self-referential cursor. `before/` and
`after/` retain exact source/binary snapshots; `phase-review.patch` records the
text delta. P3 evidence remains in
`.analysis/codex-logs/wasm-p3-20260916/phase-evidence.json`.

## Historical checkpoint — P5 blocked, partial source preserved (2026-09-17)

**Route:** `plugins`; primary document: `docs/PLUGINS.md`. Owning plan:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.
P0–P4 remain accepted: **5 completed / 101 incomplete**. P5 is **not accepted**;
P6 must not start. The TODO tool auto-promoted P6 when P5 was blocked; that is
not a routing decision. The persistent goal remains active. Maturity: **draft**.

### Concrete external blocker

The configured Task provider, `opencode-go/deepseek-v4.1-flash`, returns
HTTP **401**, `CreditsError`, **Insufficient balance**. Five initial writers
stopped after partial edits without an error diagnostic. Explicit resume
messages did not restart execution. After cancelling those sessions, a fresh
four-task recovery batch failed before work with the same explicit billing
error. All P5 writer/recovery/scout/configuration agents were then closed.

The owner requested roughly 80–85% Task delegation. The next decision is to
restore Task availability (balance or provider/model configuration), or
explicitly authorize Main-only completion instead. Do not silently replace
that execution contract or repeatedly retry the same billing failure.

### Exact source state — not a runnable deliverable

- New precommit WIT exports, SDK glue, fixture controls and nonce-fenced status
  reports are written but unbuilt. Main reduced the fixture report to 31
  records plus its summary to respect the host's 32-command bound.
- Native DTO/ticket/approval/FIFO and package/discovery registration changes
  are partial and unverified. Actual Wasmtime hook callbacks, phase enforcement
  and ordered host dispatch remain unfinished.
- Operator `[[plugins.hooks]]` configuration/check/serve source and focused tests
  are written, not run.
- Build plan fields and native helper/channel wiring are partial. Actual
  admission, current-permission checks, final consume and programmatic/settlement
  no-bypass integration are unfinished.
- Damage ingress changes reference a **not-yet-implemented**
  `session/damage_precommit.rs`; authoritative player/entity/projectile/effect
  continuation and commit migration remain unfinished.
- `tools/harness/precommit.py` and its `core-client` stage are written, **not run**.
  They target ordinary placement/mining, ordered raw damage, cancellation,
  fail-closed behavior after a trap, and a zero-subscriber comparison.
- Current WIT does **not** expose the native `SetWorldBlock`/`DamageEntity`
  commands. No new WIT mutation API was added merely for QA. Existing native
  programmatic producers still require no-bypass coverage.

No P5 build, test, formatter, graphical gate, L2 validation or independent review
passed or was claimed. Do not reuse P4's green receipts as evidence for this tree.
No commit, staging, push or source rollback occurred; inherited changes remain.

### Resume outcome and evidence

After resolving the execution choice, revalidate the partial snapshot and
finish **the full P5 before-build/before-damage outcome**, including ordered
handlers, native conservation/revision/session/permission fences, one-use and
deadline rejection, bounded outstanding tickets, programmatic coverage,
real-client latency/direct-path evidence, L2 and independent review.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
diff_hash: df308a5673c3159bb8d6172a64a74affa3e7b24f62c81e8ee57401fde564256b
changed_files: .analysis/codex-logs/wasm-p5-20260917/partial-source-snapshot.json#/changed_files
validation: [partial_source_snapshot_preserved, P5_runtime_gates_not_run]
next: Resolve Task availability or Main-only authorization, then complete and verify P5 before-build and before-damage without bypassing native owners.
```

[P5 blocker and handoff](../.analysis/codex-logs/wasm-p5-20260917/blocker.json)
records the exact failure, attempts and unfinished acceptance. The phase directory
holds the shared implementation contract, pre-P5 `before/` snapshots, and exact
29-file `partial/` source snapshots. The delta hash excludes this self-referential
cursor; `source-baseline.json` inventories the preserved starting tree.

## Checkpoint — P5 accepted (2026-09-18)

**Route:** `plugins`; primary document: `docs/PLUGINS.md`. Owning plan:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.

P5's before-build and before-damage WASM hook outcome is accepted. The current
implementation has a single native admission/commit path, ordered handlers,
replacement/cancellation propagation, deadline/reentrancy/failure denial, and
public non-exhaustive ABI types with consumers handling future variants
fail-closed. The final cleanup keeps player-damage admission owned by the
session boundary and reduces `SimulationOwner::process_batch` below the
code-health gateway budget.

The direct P5 rerun first reproduced an intermittent client-mining stall:
`start_sent=true`, with 399 accepted client continuations but no observed block
change. The client-mining result now retains that diagnostic for a future
failure. The next exact real-client rerun passed the complete canonical
`core-client` profile, including server-only natural dirt pickup and direct plus
hooked precommit paths.

P0–P4 audit: P1–P4 retain their historical acceptance evidence. P0's
plan-required Linux AArch64 proof remains unavailable in this workspace and is
not claimed as freshly verified.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
diff_hash: b5f6656c89caa19da814edccbfa114244d187e6d5fa1bf2c71d707eab7cfa7bb
changed_files:
  - crates/mc-script/src/precommit.rs
  - crates/mc-script/src/precommit_tests.rs
  - crates/mc-script/wit/precommit.wit
  - crates/mc-plugin-host/src/discovery.rs
  - crates/mc-plugin-host/src/host.rs
  - crates/mc-plugin-host/src/instance.rs
  - crates/mc-plugin-host/src/lib.rs
  - crates/mc-plugin-host/tests/precommit_host.rs
  - crates/mc-net/src/play.rs
  - crates/mc-net/src/play/bucket_precommit_tests.rs
  - crates/mc-net/src/play/command_execution.rs
  - crates/mc-net/src/play/player_damage_adapter.rs
  - crates/mc-net/src/play/simulation.rs
  - crates/mc-net/src/play/simulation/precommit.rs
  - crates/mc-net/src/play/simulation/queue.rs
  - crates/mc-net/src/play/simulation/tests/precommit_tests.rs
  - crates/mc-net/src/play/tests/player_damage.rs
  - crates/mc-net/src/play/use_item_on_adapter.rs
  - crates/mc-net/src/play/session/damage_precommit.rs
  - crates/mc-net/src/play/session/entity_combat.rs
  - crates/mc-net/src/play/session/explosion_authority.rs
  - crates/mc-net/src/play/session/player_combat.rs
  - crates/mc-net/src/play/session/player_combat/tests/precommit_tests.rs
  - crates/mc-net/src/play/session/player_effects.rs
  - crates/mc-net/src/play/session/player_state.rs
  - crates/mc-net/src/play/session/projectiles.rs
  - crates/mc-net/src/play/session/survival_action_authority.rs
  - crates/mc-net/src/play/session/transactions.rs
  - crates/mc-net/src/play/session/transactions_precommit_tests.rs
  - crates/mc-net/src/play/session/village_defense.rs
  - sdk/rust/examples/hello/src/precommit.rs
  - tools/harness/precommit.py
validation:
  - .analysis/validation/20260918T041516-core-client-o204mnk0/result.json:
      passed complete real-client core-client profile, including P5 direct/hooked
  - .analysis/validation/20260918T023332-core-client-3vk_ache/scenario.json:
      historical first P5 direct/hooked pass; superseded by the final full pass
  - .analysis/validation/20260918T033535-correctness-bze01xms/result.json:
      passed (fmt, code-health, strict workspace Clippy, all workspace targets)
  - P5FinalReview: pass; no findings
next: Assess P6 scope against the owning WASM plan.
```

## Checkpoint — P6 code complete, full validation interrupted (2026-09-18)

**Route:** `plugins`; primary document: `docs/PLUGINS.md`. Owning plan:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.

P6 implements strict component deployment reload without replacing the server's
`ScriptBoundary`, player-session handle, network, or world. A candidate is
re-discovered from re-read strict WASM configuration, built in separate stores,
and staged behind the host FIFO. It compares ordered identities, package catalog,
player/operator command roots, payload channels, effective grants, pre-commit
roster, deployment surface, and startup contribution. A static bound rejects
combined old/candidate guest memory before candidate construction. Commit
replaces routes and registrations atomically; old timers begin no new callbacks,
and targeted results admitted under an old registration cannot reach a same-id
replacement. Buffered targeted results still drain during graceful shutdown.

The independent P6 review found two concrete issues: candidate init timers used
tick zero after a live reload, and the new targeted-event fence discarded
already-admitted results during graceful shutdown. Both are fixed: candidates
now use the active simulation tick, while closed admission retains registration
fences until the FIFO drains. The reviewer was not re-run, per the one-review
rule.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
diff_hash: 13386dd30f74a6c47f2de1ed6bcd76538fb2ef8cb10e351963c4067bf6ae9ebc
changed_files:
  - crates/mc-plugin-host/Cargo.toml
  - crates/mc-plugin-host/src/host.rs
  - crates/mc-plugin-host/src/limits.rs
  - crates/mc-plugin-host/tests/host_deployment.rs
  - crates/mc-script/src/lib.rs
  - crates/mc-server/src/component_startup_tests.rs
  - crates/mc-server/src/main.rs
  - docs/PLUGINS.md
  - docs/OPERATING.md
  - docs/decisions/0009-regional-plugin-boundary.md
  - docs/MEMORY.md
validation:
  - cargo test -p mc-script targeted_event_admitted_before_replacement_is_not_delivered_afterwards:
      passed before the final shutdown-drain correction
  - cargo test -p mc-script closing_event_admission_drains_buffered_targeted_events:
      passed
  - cargo test -p mc-plugin-host --test host_deployment:
      passed (8 lifecycle/reload cases)
  - cargo test -p mc-server component_reload_from_config_replaces_the_live_generation:
      passed
  - .analysis/validation/20260918T044847-fmt-qfingqa5/result.json:
      passed before the final mechanical reload-context refactor; affected Rust source
      was subsequently formatted directly with rustfmt --edition 2024
  - .analysis/validation/20260918T044854-code-health-p04k8y6j/result.json:
      passed before the final mechanical reload-context refactor
  - .analysis/validation/20260918T044948-clippy-787y668_/result.json:
      passed after the final refactor
  - P6ReloadReview:
      changes requested: timer origin and shutdown drain; both source-backed findings fixed
  - .analysis/validation/20260918T045014-correctness-woiq81bf:
      interrupted during workspace test compilation with exit 241; no result.json written
  - .analysis/validation/20260918T045131-test-c8sb7lsk:
      interrupted during workspace test compilation with exit 241; no result.json written
status: validation-blocked
next: Restore a harness workspace-test run that does not terminate under its 4G scope and writes a receipt, then rerun correctness before closing P6 and starting P7.
```

## Checkpoint — P7/P8 component-only cutover (2026-09-18)

**Route:** `plugins`; primary document: `docs/PLUGINS.md`. Owning plan:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.

P7 now deploys five real first-party Rust components: permissions, essentials,
economy, towns, and audit. They run through strict package discovery with the
ordinary component host, retain session-scoped replies and exact TPA correlation,
and have aggregate standard-pack acceptance coverage.

P8 removes the retired Luau path end-to-end: `mc-server` always discovers
components; the `runtime` selector, host module, manifest dual-version default,
Lua dependencies, source fixtures, and current documentation are gone. API
0.7 is the sole package contract. Component migration tests now register a
receiving plugin route before asserting targeted answers. The zone precommit
fixture builds its component before opening the bounded native request, so guest
build work cannot consume the request deadline.

The independent P8 review found four concrete test-cutover defects: unregistered
query result delivery, an obsolete CLI runtime selector, obsolete CLI JSON
expectations, and 0.6 API test expectations. All were fixed and the exact
affected suites passed. The reviewer was not re-run under the one-review rule.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
diff_hash: df99c2a8a71ee354150f14481dc3ca6aa578ec5688f8b070b1dcae10738c0aae
changed_files:
  - Cargo.toml
  - Cargo.lock
  - README.md
  - example.toml
  - docs/PLUGINS.md
  - docs/OPERATING.md
  - docs/ARCHITECTURE.md
  - crates/mc-script/src/lib.rs
  - crates/mc-script/src/gameplay_rules.rs
  - crates/mc-script/src/plugin_metadata.rs
  - crates/mc-script/src/client_view.rs
  - crates/mc-script/wit/events.wit
  - crates/mc-script/wit/world-events.wit
  - crates/mc-script/wit/commands.wit
  - crates/mc-script/wit/lifecycle.wit
  - crates/mc-plugin-host/Cargo.toml
  - crates/mc-plugin-host/src/legacy_luau/
  - crates/mc-plugin-host/src/{adapter,client_bundle,host,lib,limits,package,required_features,startup,timers,world_events,worldgen}.rs
  - crates/mc-plugin-host/tests/{audit_component,component_roundtrip,deployment_discovery,economy_component,essentials_component,first_party_component_pack,host_deployment,host_runtime,loader_operations,package_contract,permissions_component,player_operations,startup_contribution,towns_component}.rs
  - crates/mc-net/Cargo.toml
  - crates/mc-net/src/{play.rs,script/mod.rs,script/router.rs,script/storage.rs,script/zone.rs}
  - crates/mc-net/src/play/{persistence/inventory_recovery_tests.rs,session/entity_lifecycle.rs,session/script_client_sound_endpoint_tests.rs,session/script_client_view_endpoint_tests.rs,session/script_menu_endpoint_tests.rs,simulation/tests/precommit_tests.rs}
  - crates/mc-net/src/script/{inventory_tests,player_query_tests,teleport_tests}.rs
  - crates/mc-server/Cargo.toml
  - crates/mc-server/src/{component_startup_tests,lib,main,structure_rules_tests}.rs
  - crates/mc-server/tests/{cli,play}.rs
  - crates/mc-test-harness/Cargo.toml
  - crates/mc-test-harness/tests/commands.rs
  - sdk/rust/Cargo.toml
  - sdk/rust/solaris-plugin-sdk/src/lib.rs
  - sdk/rust/packages/solaris-{permissions,essentials,economy,towns,audit}/
validation:
  - cargo check -p mc-server -p mc-plugin-host -p mc-net:
      passed
  - cargo test -p mc-plugin-host --test permissions_component --test essentials_component --test economy_component --test towns_component --test audit_component --test first_party_component_pack --test component_roundtrip --test deployment_discovery --test host_deployment --test startup_contribution:
      passed 34 tests
  - cargo test -p mc-plugin-host --test player_operations:
      passed 6 tests
  - cargo test -p mc-server --test play component_plugin_loaded_from_disk_replies_to_join_and_command_over_the_wire:
      passed
  - cargo test -p mc-server --test cli check_:
      passed 43 tests
  - cargo test -p mc-net teleport_adapter_publishes_exact_unavailable_result:
      passed
  - cargo test -p mc-net player_query_adapter_publishes_authoritative_targeted_snapshot:
      passed
  - cargo test -p mc-net changed_zone_snapshot_refuses_kept_placement_without_mutation:
      passed
  - .analysis/validation/20260918T073558-fmt-g6ehae19/result.json:
      passed
  - .analysis/validation/20260918T073602-code-health-xg0kf2_f/result.json:
      passed
  - P8ComponentOnlyReview:
      changes requested; all four source-backed findings fixed
status: P7/P8 complete; P6 and P0 remain externally blocked
next: Implement CP-001 planned implementation and evidence.
```

## Checkpoint — CP-001 component warehouse hauling (2026-09-18)

**Route:** `plugins`; primary document:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.

CP-001 ships `solaris-settlements` as a Rust/WASM component. It persists its
operation counter before issuing work, restores the counter on guest restart,
and validates player input before it can reach host DTO admission. A bind stores
only a matching committed opaque warehouse binding. A haul takes the caller's
exact current resident revision and emits the existing directed
`ResidentCarry -> Warehouse` order; it never accepts a chest coordinate or
warehouse handle from a player.

The focused component scenario builds the real guest, deploys its shipped
manifest, restores the durable counter through host storage callbacks, binds a
real core warehouse, sends its admitted typed haul through the inventory
runtime, and observes the chest moving from 12 to 15 logs while carry empties.
The scenario runs with `NoSessions`, so no online player principal is invented.
Existing core cases prove the same atomic path retains cargo on a full chest,
continues after a rejected mixed stack, and replays one transfer after reopening
durable storage. A refused or unavailable binding remains a typed core refusal;
the guest retains no synthetic empty warehouse and cannot accept a destination
handle or chest coordinate from a player.

The independent CP-001 review found four defects: unbounded player DTO input,
operation-id reuse after guest restart, reanimated completed binding
correlations, and silently upgraded freshness fences. All are fixed. The review
was not rerun under the one-review rule.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
diff_hash: 3f437625fdfe3807e464f028abf6b68b24ee0a78a08d0758f841f78ca639c449
changed_files:
  - sdk/rust/Cargo.toml
  - sdk/rust/packages/solaris-settlements/Cargo.toml
  - sdk/rust/packages/solaris-settlements/plugin.toml
  - sdk/rust/packages/solaris-settlements/config.toml
  - sdk/rust/packages/solaris-settlements/src/lib.rs
  - crates/mc-net/src/script/storage/resident_settlement_tests.rs
  - crates/mc-plugin-host/tests/first_party_component_pack.rs
  - docs/MEMORY.md
validation:
  - cargo check -p solaris-settlements-plugin --target wasm32-unknown-unknown:
      passed
  - cargo test -p mc-net --lib settlement_component_binds_warehouse_and_delivers_resident_carry:
      passed
  - cargo test -p mc-net --lib worker_haul_deposits_its_cargo_into_the_bound_warehouse:
      passed
  - cargo test -p mc-net --lib a_full_container_leaves_the_cargo_with_the_worker:
      passed
  - cargo test -p mc-net --lib a_replayed_haul_deposits_once:
      passed after reopening durable storage
  - cargo test -p mc-net --lib a_haul_deposits_past_a_stack_the_container_refuses:
      passed
  - cargo test -p mc-plugin-host --test first_party_component_pack:
      passed
  - .analysis/validation/20260918T081018-fmt-b8eu1_nb/result.json:
      passed
  - .analysis/validation/20260918T081235-code-health-kw0k4bdj/result.json:
      passed
  - CP001SettlementReview:
      changes requested; all four source-backed findings fixed
  - python3 -m tools.harness run correctness:
      not rerun; the active P6 4G workspace-test blocker exits 241 without a
      receipt, so no unavailable L2 pass is claimed
status: CP-001 complete under the existing P6 L2 blocker; P0 and P6 remain externally blocked
next: Verify CP-002 WASM settlement readiness.
```

## Checkpoint — CP-002 WASM settlement readiness (2026-09-18)

**Route:** `plugins`; primary document:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.

The historical `settlement_pause_repro` was a removed Lua-host scenario. Its
old standalone pass is not treated as a fix. The current Rust/WASM
`solaris-settlements` guest instead has one bounded real-component readiness
route: strict package discovery, public deployment through compile,
startup-instantiation/configure/runtime-instantiation/init, the durable
counter callback, then bind and haul completion callbacks. Every command
receipt now has a ten-second failure boundary which identifies the stalled
phase; elapsed phase lines are evidence, not performance limits.

The route passed at rest and under four owned CPU-spinner processes. It reached
its boot and all queue receipts under the latter without changing limits,
epochs, fuel, retries, queue capacity, or a wait budget. The ported real-world
WASM settlement fixture also passed two scenarios concurrently: its full
server/world owner flow and its capability refusal. Separate host controls
passed actual configure/init refusal, queue backpressure, guest CPU exhaustion,
and a guest blocked in a host-native import. Thus current WASM readiness is
observable and did not reproduce the historic failure signature; this is not a
claim that the historical Lua load sensitivity was repaired.

Raw receipts and the independent review are
`.analysis/codex-logs/cp002-wasm-readiness-20260918/receipt.json`.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
source_hash: 69ac0a676f073ae3426f0b37dd622861b70e6e4c8f78d7b64647124d22e3157a
changed_files:
  - crates/mc-net/src/script/storage/resident_settlement_tests.rs
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net --lib settlement_component_binds_warehouse_and_delivers_resident_carry -- --nocapture:
      passed; actual component readiness receipts recorded
  - four owned CPU spinners + current component scenario:
      passed; raw log recorded
  - RUST_TEST_THREADS=2 cargo test -p mc-test-harness --test wasm_settlement_operations -- --nocapture:
      passed; 2 current WASM settlement scenarios
  - cargo test -p mc-plugin-host --test host_runtime --test startup_lifecycle -- --nocapture:
      passed; 16 readiness-control tests
  - .analysis/validation/20260918T082351-fmt-2vesexe3/result.json:
      passed
  - .analysis/validation/20260918T082400-code-health-gqsmljxh/result.json:
      passed
  - CP002ReadinessReview:
      pass; no source-backed findings
  - python3 -m tools.harness run correctness:
      not rerun; active P6 4G workspace-test blocker remains unresolved
status: CP-002 complete under the existing P6 L2 blocker; P0 and P6 remain externally blocked
next: Implement CP-003 core-issued resident tool supply.
```

## Checkpoint — CP-003 WASM warehouse issuance (2026-09-18)

**Route:** `plugins`; primary document:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.

The native warehouse-to-resident composite already owned binding resolution,
real container slots, revision/capacity/item validation, one journal decision,
and no player principal. CP-003 exposes that existing authority to the shipped
Rust/WASM settlements guest without a new WIT endpoint or inventory authority:
`/settlement issue <resident> <revision> <carry|equipment> [item]` emits only
`Warehouse(binding) -> ResidentCarry/ResidentEquipment` work after durable
operation-counter reservation.

The actual compiled component binds its core-owned warehouse, deposits three
logs, then issues all fifteen logs into resident carry and the resident's iron
axe into equipment. The core chest ends empty; every move has no player
participant and all four component operations have durable receipts. Existing
native checks also pass named carry/equipment withdrawals, missing input, full
carry, unknown warehouse, and exact replay. The guest keeps the caller-supplied
revision; it cannot choose a warehouse handle or a player inventory endpoint.

Raw receipt and independent review:
`.analysis/codex-logs/cp003-wasm-warehouse-issue-20260918/receipt.json`.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
source_hashes:
  sdk/rust/packages/solaris-settlements/src/lib.rs: 5f53b4d45a0db52ae8d6cab1262d351c9bfc4e3d121cddb3c620086c63fec67e
  crates/mc-net/src/script/storage/resident_settlement_tests.rs: bcdfd6d8813a4531cba495018e929cd89365f0fd32e730c08cef268cf9742d6f
changed_files:
  - sdk/rust/packages/solaris-settlements/src/lib.rs
  - crates/mc-net/src/script/storage/resident_settlement_tests.rs
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net --lib settlement_component_binds_warehouse_and_delivers_resident_carry -- --nocapture:
      passed; real carry and equipment issuance
  - cargo test -p mc-net --lib worker_withdraws -- --nocapture:
      passed; 2 native endpoint tests
  - cargo test -p mc-net --lib a_withdrawal -- --nocapture:
      passed; 3 native refusal tests
  - cargo test -p mc-net --lib a_replayed_withdrawal -- --nocapture:
      passed; one-transfer replay
  - .analysis/validation/20260918T083940-fmt-0ceq9upi/result.json:
      passed
  - .analysis/validation/20260918T083949-code-health-ck8ei13h/result.json:
      passed
  - CP003IssuanceReview:
      pass; no source-backed findings
  - python3 -m tools.harness run correctness:
      not rerun; active P6 4G workspace-test blocker remains unresolved
status: CP-003 warehouse issuance complete; village container-source scope remains open; P0 and P6 remain externally blocked
next: Complete CP-003 village container source before CP-004.
```

## Checkpoint — CP-003 village container source (2026-09-18)

The settlements component now binds a warehouse to one loaded vanilla village
container with `settlement bind-village <site-id> <container-ordinal>`. The
core resolves the selection through the existing generator village inventory
path, verifies it is loaded, and persists the resolved block position as the
binding identity. The ordinal therefore selects a source only at bind time:
later container enumeration changes cannot make the original chest available
to another settlement. Foreign claims, unknown/unloaded sources, and missing
sources are refused. Existing flat authored warehouse receipts deserialize
into the explicit authored source form, so the WIT/source-model migration
preserves durable settlement bindings.

The independent review found and prompted fixes for ordinal-shift duplicate
binding and legacy-receipt decoding. The focused tests exercise both fixes.
Raw evidence: `.analysis/codex-logs/cp003-village-warehouse-20260918/receipt.json`.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
implementation_diff_hash: 8e0932c7e7caf65a843393d78e64a7aaed699d01311768ed3fdd3c7f9ca014fb
source_hashes:
  crates/mc-script/wit/settlements.wit: ff44fd25124f878e23d410a246b22dd540b6178dec42ceee5981e1b7ec72fecf
  crates/mc-script/src/settlement_operations.rs: a2bfe19733b7a81c4df95bbc06b209e572a8b40d366059e183a42649bc0d93a0
  crates/mc-plugin-host/src/domain_settlements.rs: 81c94f1ecc1126c2cc7d2c7645ebcce318063f16e8bffe61cdbb2a0f3a066b96
  crates/mc-net/src/script/storage/settlement.rs: f58f15e91310f5d0e2714f1f634014ab96bf47f5b29121194f33b0e6fa090329
  crates/mc-net/src/script/storage/settlement_tests.rs: 3ba1058f297c74555dab366599c6c4b6e204d6062d85630d2f83d43fc5dc5989
  crates/mc-net/src/settlement.rs: 937c449ce88f372e0dd1a98023297e5b74b6aad7b10859374be42c59ad6bd0e6
  sdk/rust/solaris-plugin-sdk/src/settlement_ops.rs: fee463c46fe40b2e0c77949c5e8b2e32563d5494f01d973d0d359085fc12b248
  sdk/rust/packages/solaris-settlements/src/lib.rs: 5036f847eb0f3883a816bbcaa8de83ab47b0db26dc4caa24e02be0eb729a66b9
changed_files:
  - crates/mc-script/wit/settlements.wit
  - crates/mc-script/src/settlement_operations.rs
  - crates/mc-script/src/lib.rs
  - crates/mc-plugin-host/src/domain_settlements.rs
  - crates/mc-net/src/script/storage/settlement.rs
  - crates/mc-net/src/script/storage/settlement_tests.rs
  - crates/mc-net/src/settlement.rs
  - sdk/rust/solaris-plugin-sdk/src/settlement_ops.rs
  - sdk/rust/packages/solaris-settlements/src/lib.rs
  - docs/MEMORY.md
validation:
  - cargo test -p mc-script warehouse_binding_and_handle_are_bounded_and_owner_scoped -- --nocapture:
      passed; legacy flat receipt migration
  - cargo test -p mc-net --lib village_warehouse_binds_one_materialized_position_without_rebinding_its_ordinal -- --nocapture:
      passed; exact source identity across ordinal shifts
  - cargo test -p mc-net --lib settlement_component_binds_warehouse_and_delivers_resident_carry -- --nocapture:
      passed; compiled component compatibility
  - .analysis/validation/20260918T091802-fmt-0kfrdzhp/result.json:
      passed
  - .analysis/validation/20260918T091810-code-health-xfjenx9c/result.json:
      passed
  - CP003VillageSourceReview:
      changes applied; no second review per repository policy
  - python3 -m tools.harness run correctness:
      not run; active P6 4G workspace-test blocker remains unresolved
status: CP-003 complete; P0 and P6 remain externally blocked
next: Begin CP-004 against the existing warehouse reservation authority.
```

## Checkpoint — CP-004 warehouse stock reservation (2026-09-18)

**Route:** `plugins`; primary document:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.

Construction now reserves quantities from the exact bound vanilla warehouse
container rather than promising a material plan from stale snapshot data. The
physical reservation floor is shared by all three chest mutation authorities:
the ordinary regional client route, the fallback route, and server-owned
resident warehouse transfers. A player or resident transfer that would reduce
a reserved item below its aggregate outstanding quantity is rejected and
resynchronized. Reservation admission and every chest mutation use one
non-blocking gate, so a successful reservation cannot race a previously
checked chest withdrawal.

Reservation recovery rebuilds floors from durable reservations. An unloaded
but otherwise valid bound source retains its durable position floor until it
can be resolved again; missing, invalid, or destroyed sources contribute no
new live floor and further reservation requests refuse. The focused recovery
test closes two projects against twelve logs, rejects an overpromise and a
destroyed source, restarts while the source is unloaded, then confirms the
recovered floor still rejects an eleven-log after-image.

Raw receipt and independent review:
`.analysis/codex-logs/cp004-warehouse-reservations-20260918/receipt.json`.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
implementation_diff_hash: 9fa847e3dcd485f155eccc88ce9525805e4517215f08d9e023e1cd40a3d30728
source_hashes:
  crates/mc-net/src/play/session.rs: 70e5a69b7ab22edc16eef6a116e220f87cdd4b185aadda20f9888ad57e653a6b
  crates/mc-net/src/play/session/container_views.rs: 602f01952d2e1606e66fc37edac1089eda0722e27dae6718f3fdfde97586416b
  crates/mc-net/src/play/session/transactions.rs: d05345faf8645f9f222c48bb5c5cd7ddaf9c8ee1380a0fef9ebedad0052800b8
  crates/mc-net/src/play/session/tests.rs: 0e699dea8e462932bcb226acd139890b912eb92fd4c8e3fbb21ef16a8ddb09aa
  crates/mc-net/src/play/simulation.rs: 73910f5359f650e90be078f0025c8238155c4a990b984319a445f4127a63ca7d
  crates/mc-net/src/script/storage.rs: 7eeef7f9dc6eb4f920c78379ecf4f3de5a03d202608fdb4141f9fe392ec98e38
  crates/mc-net/src/script/storage/world_inventory.rs: 1e13993c36470870bf2cb70d093e83fe12855894fc5e6953af7b22040c4eecb7
  crates/mc-net/src/script/storage/settlement.rs: 301b1e638e6edb7a0b8f5e40269841b988b88a041b49374cef979436d13f16dc
  crates/mc-net/src/script/storage/settlement_tests.rs: acd84769c6a02fac7a16cd5205afc965e107e0890d127d7318d8fd2125de92c9
changed_files:
  - crates/mc-net/src/play/session.rs
  - crates/mc-net/src/play/session/container_views.rs
  - crates/mc-net/src/play/session/transactions.rs
  - crates/mc-net/src/play/session/tests.rs
  - crates/mc-net/src/play/simulation.rs
  - crates/mc-net/src/script/storage.rs
  - crates/mc-net/src/script/storage/settlement.rs
  - crates/mc-net/src/script/storage/settlement_tests.rs
  - crates/mc-net/src/script/storage/world_inventory.rs
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net --lib warehouse_reservation -- --nocapture:
      passed; five reservation admission, physical floor, unload/restart, fallback, and server-owned tests
  - cargo test -p mc-net --lib regional_chest_commit_resyncs_when_a_warehouse_floor_would_be_consumed -- --nocapture:
      passed; ordinary regional client chest route
  - cargo test -p mc-net --lib owned_reservation_blocks_a_drawdown_of_the_reserved_quantity -- --nocapture:
      passed; durable reservation projection
  - cargo test -p mc-net --lib warehouse_transfer_commits_both_participants_under_one_decision -- --nocapture:
      passed; existing receipt-backed composite path
  - .analysis/validation/20260918T095754-fmt-o75at0xe/result.json:
      passed
  - .analysis/validation/20260918T095759-code-health-axcu3j07/result.json:
      passed
  - CP004ReservationReview:
      changes fixed; no second review per repository policy
  - python3 -m tools.harness run correctness:
      not run; active P6 4G workspace-test blocker remains unresolved
status: CP-004 complete; P0 and P6 remain externally blocked
next: Begin CP-005 from the active plugin route.
```

## Checkpoint — CP-005 atomic construction portions (2026-09-18)

**Route:** `plugins`; primary document:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.

Construction no longer applies blocks before separately committing progress. One
receipt-bearing `ApplyBlockEdits` regional decision conditionally changes and
stamps the finite portion, persists its chunk after-images with the encoded
prepared settlement batch, then publishes. The storage projection and
`mark_inventory_projected` run only after that decision id returns. Recovery
decodes that same generic decision batch before admitting new work.

Receipt portions always acquire exact block mutation preconditions even without
a `before-build` handler. Immediately before regional mutation, the worker
rechecks its zone fence and consumes any current build approval. Ordinary
server-owned block edits retain the canonical staged lane. The structure's next
footprint revision is predicted from the post-portion image, so its own commit
does not pause subsequent portions while a foreign in-footprint edit still does.

The new recovery test injects a storage append fault after the world decision,
reopens the world journal, projects the receipt once, and proves replay submits
no second portion. A competing advance against the consumed structure revision
is refused before a second block or reservation consumption. The regional-path
precommit test cancels a receipt portion, leaves the block unchanged, and closes
its reserved journal id without an after-image or recoverable receipt.

ADR authority/persistence ordering:
`docs/decisions/0004-staged-single-writer-simulation.md`.
Raw receipt and review evidence:
`.analysis/codex-logs/cp005-atomic-structure-portions-20260918/receipt.json`.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
implementation_diff_hash: abbc21e65d81a1cb454a5bd9522594073761344a613f6f1ee0ac8081b1b6b99d
source_hashes:
  crates/mc-net/src/settlement.rs: 26a6caf59c608ed98cb7f2d57c30492965d409d745421c72c02f54b0ec2275fd
  crates/mc-net/src/play/simulation.rs: b71618e7a522552c093dea0a7671ebb08c3ba3409ddd70cd25604bf9ffc61a1c
  crates/mc-net/src/play/simulation/regional_mutation.rs: e4f3c03b06cd341c6e51f69d2b7fc0f028962d6e188a9c2ccb883905d9f07426
  crates/mc-net/src/play/simulation/tests/precommit_tests.rs: 3d60b39d786047ede41c2dd30664994177e307fab86a0c123f11d3e28f240dd2
  crates/mc-net/src/play/world_inventory_journal.rs: 21edf2987b46d3ec0b740a7175796c33909e5f85074787a43a6f909f0c89c33e
  crates/mc-net/src/script/storage/settlement.rs: b81b2df1559a167a5a70aa60b908baf5d2974b7cbcadb19f139a48bb9289ecd8
  crates/mc-net/src/script/storage/world_inventory.rs: 6b0d6a37723113740e5af6ee9f57581432d0f0f88d1a71bfd6c0f73fa517d529
  crates/mc-net/src/script/storage/settlement_tests.rs: c47938e7d82cbdbdd48928dae2b4dc27226166ff2a1b7a4b210eb201ad7fc128
  crates/mc-net/src/script/storage/resident_settlement_tests.rs: 04e066aba548f5b038fcb10d6071310d02e8a1ed9d5ca2b28e5e004abb645681
  docs/decisions/0004-staged-single-writer-simulation.md: 4860c3456c06579f52fdc14a7c05e5ccc85a0430787199cb751d8a83d136b47b
changed_files:
  - crates/mc-net/src/settlement.rs
  - crates/mc-net/src/play/simulation.rs
  - crates/mc-net/src/play/simulation/regional_mutation.rs
  - crates/mc-net/src/play/simulation/tests/precommit_tests.rs
  - crates/mc-net/src/play/world_inventory_journal.rs
  - crates/mc-net/src/script/storage/settlement.rs
  - crates/mc-net/src/script/storage/world_inventory.rs
  - crates/mc-net/src/script/storage/settlement_tests.rs
  - crates/mc-net/src/script/storage/resident_settlement_tests.rs
  - docs/decisions/0004-staged-single-writer-simulation.md
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net --lib cancelled_journaled_structure_portion_keeps_blocks_and_receipt_uncommitted -- --nocapture:
      passed; regional worker consumes the cancelled build approval before mutation and closes an empty decision
  - cargo test -p mc-net --lib structure_portion -- --nocapture:
      passed; regional receipt decision and append-failure recovery
  - cargo test -p mc-net --lib competing_structure_portions_admit_only_one_revision -- --nocapture:
      passed; competing builders share one structure revision fence
  - cargo test -p mc-net --lib advance_ -- --nocapture:
      passed; advance, replay, foreign footprint, and reservation-plan coverage
  - cargo test -p mc-net --lib construct_work_consumes_the_reserved_portion_and_commits_the_stage -- --nocapture:
      passed; resident construction consumes one durable reservation portion
  - .analysis/validation/20260918T103644-fmt-_0o21unu/result.json:
      passed
  - .analysis/validation/20260918T103648-code-health-1purrfh7/result.json:
      passed
  - CP005Advisor and CP005SecondPOV:
      material routing finding fixed and refusal branch covered; reservation-fold concern disproved by storage receipt indexing and focused tests; no second review per repository policy
  - python3 -m tools.harness run correctness:
      not run; active P6 4G workspace-test blocker remains unresolved
status: CP-005 complete; P0 and P6 remain externally blocked
next: Begin CP-006 from the active plugins route.
```

## Checkpoint — CP-006 safe construction lifecycle (2026-09-18)

**Route:** `plugins`; primary document:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.

`resume-structure` is now a durable WIT/DTO/host/SDK operation. It only accepts
the owner’s exact paused revision after the authoritative footprint still
matches its last accepted image and any bound reservation remains compatible and
unreleased. It restores `Prepared` before a first portion or `Running` after a
partial build. A changed or unavailable footprint refuses without recording a
new operation; callers must reread authoritative status and do not recreate an
already accepted portion.

Cancellation remains terminal. It never removes built blocks, retained receipt
consumption stays spent, and the reservation fold returns only its unspent
quantities. The lifecycle test covers pause/resume before and after a portion,
foreign-footprint refusal, partial cancellation, and repeated cancellation.

Raw receipt and review evidence:
`.analysis/codex-logs/cp006-construction-lifecycle-20260918/receipt.json`.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
implementation_diff_hash: d93b19916a0256ee9211363d882080f161e690ad3c9c3f3ed7a1d67b563946d4
source_hashes:
  crates/mc-script/wit/settlements.wit: cdea625d81f2a9f1a4da5e464498a18873a4f964d056e03480d0942d6a1f8ecf
  crates/mc-script/src/settlement_operations.rs: f54c869bcc8f2fa60f3b9285b62d808f157f50af2920c46998cdfefae8aa2308
  crates/mc-script/src/lib.rs: 44397c2f2fb52e3d2b0509717f9f95ccf4a7763c870806d92c7c4748f4059b87
  crates/mc-plugin-host/src/domain_settlements.rs: 2df81af87fefd6d43d018ce5cf991f4c22c79321a57936715f434070a874ae77
  sdk/rust/solaris-plugin-sdk/src/settlement_ops.rs: 8312eaec0204551f726b301bdd2eae006ae365e5529466c445e4a5f667741592
  crates/mc-net/src/script/storage/settlement.rs: 7e951cf2ae40e00cc6b7619b0560f0408ed430095136b1e35822df6c13b86ff6
  crates/mc-net/src/script/storage/settlement_tests.rs: 8a695a432fcfe73bb401e1853acd53c4b4ff7c18dd1408d29071f9cce0e897cc
changed_files:
  - crates/mc-script/wit/settlements.wit
  - crates/mc-script/src/settlement_operations.rs
  - crates/mc-script/src/lib.rs
  - crates/mc-plugin-host/src/domain_settlements.rs
  - sdk/rust/solaris-plugin-sdk/src/settlement_ops.rs
  - crates/mc-net/src/script/storage/settlement.rs
  - crates/mc-net/src/script/storage/settlement_tests.rs
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net --lib pause_resume_requires_the_accepted_footprint_and_releases_only_unspent_materials -- --nocapture:
      passed; prepared/running resume, foreign footprint, partial cancel, and repeat-cancel coverage
  - cargo test -p mc-net --lib prepare_builds_nothing_and_cancel_preserves_built_blocks -- --nocapture:
      passed; cancel before first and after partial portion
  - cargo test -p mc-net --lib advance_replays_after_reopen_without_a_second_portion -- --nocapture:
      passed; restart/replay between accepted portion and caller replay
  - cargo test -p mc-script --lib operation_id_is_exposed_only_for_idempotent_mutations -- --nocapture:
      passed; durable resume DTO identity
  - cargo test --manifest-path sdk/rust/solaris-plugin-sdk/Cargo.toml --lib -- --nocapture:
      passed; updated guest contract compiles
  - .analysis/validation/20260918T104617-fmt-pjh_t_3b/result.json:
      passed
  - .analysis/validation/20260918T104621-code-health-imgrvfsq/result.json:
      passed
  - CP006LifecycleReview:
      pass; no concrete lifecycle or conversion findings
  - python3 -m tools.harness run correctness:
      not run; active P6 4G workspace-test blocker remains unresolved
status: CP-006 complete; P0 and P6 remain externally blocked
next: Begin CP-007 from the active plugins route.
```

## Checkpoint — CP-007 atomic resident actions (2026-09-18)

**Route:** `plugins`; primary document:
`/home/kaiserroman/Downloads/SOLARIS_LONG_TERM_PLAN_WASM.md`.

Harvest, mine and cut-tree work now preview its exact source state/token and
canonical loot, proves cargo capacity before a world change, then makes the
block after-image and prepared resident cargo/tool/progress batch one
receipt-bearing regional decision. Storage projection follows the decision id;
recovery installs its single receipt before replay, so an append failure cannot
lose or duplicate resident loot.

A stale-replacement adapter changes a crop after preview and proves the
conditional decision leaves the new block, carry and work watermark untouched.
One work decision remains in one regional owner lane. Crop and ore breaks do
not schedule leaf propagation and therefore admit a valid edge cell through the
journaled path; logs retain their leaf scheduling.

The shared structure and resident commit helpers now size/compact the projection
before asking the world to commit. A quota refusal therefore happens while the
source blocks and prepared resident receipt are both still uncommitted.

ADR authority/persistence ordering:
`docs/decisions/0004-staged-single-writer-simulation.md`.
Raw receipt and review evidence:
`.analysis/codex-logs/cp007-atomic-resident-actions-20260918/receipt.json`.

```yaml
base_tree: 62ea32a37e740f1559355056bde2558d3a6b5c4b
implementation_diff_hash: 6eefda196ca21adabd8a7e11dbd3164716fea45d92b1802d7c650c56b811332d
changed_files:
  - crates/mc-net/src/play/resident_work.rs
  - crates/mc-net/src/play/simulation.rs
  - crates/mc-net/src/play/simulation/regional_mutation.rs
  - crates/mc-net/src/script/storage/resident_order_execution.rs
  - crates/mc-net/src/script/storage/resident_order_tests.rs
  - crates/mc-net/src/script/storage/resident_settlement_tests.rs
  - crates/mc-net/src/script/storage/world_inventory.rs
  - crates/mc-net/src/server.rs
  - crates/mc-script/src/lib.rs
  - crates/mc-script/src/settlement_operations.rs
  - docs/decisions/0004-staged-single-writer-simulation.md
  - docs/MEMORY.md
validation:
  - cargo test -p mc-net --lib replaced_crop_after_preview_does_not_create_cargo_or_work_progress -- --nocapture:
      passed; replacement between preview and decision preserves the replacement and creates neither cargo nor work progress
  - cargo test -p mc-net --lib harvest_ -- --nocapture:
      passed; canonical crop cargo, storage-fault recovery, and owner-edge harvest
  - cargo test -p mc-net --lib mined_ore_reaches_the_worker_cargo -- --nocapture:
      passed
  - cargo test -p mc-net --lib cut_tree_commits_the_log_and_worker_cargo_together -- --nocapture:
      passed
  - cargo test -p mc-net --lib a_full_worker_reports_no_storage_and_leaves_the_crop_standing -- --nocapture:
      passed
  - cargo test -p mc-net --lib structure_portion -- --nocapture:
      passed; construction composite regression after shared projection-admission ordering
  - cargo test -p mc-net --lib resident -- --nocapture:
      passed; 75 resident-focused tests
  - .analysis/validation/20260918T115820-fmt-v03_h9s8/result.json:
      passed
  - .analysis/validation/20260918T115824-code-health-_8llogyb/result.json:
      passed
  - CP007AtomicReview:
      two material findings fixed; no second review per repository policy
  - python3 -m tools.harness run correctness:
      not run; active P6 4G workspace-test blocker remains unresolved
status: CP-007 complete; P0 and P6 remain externally blocked
next: Begin CP-008 from the active plugins route.
```

## Checkpoint — architecture-review comprehensibility (2026-09-20)

Accepted boundaries from
`.rpiv/artifacts/architecture-reviews/2026-09-20_18-44-21_solaris-model-comprehensibility.md`
are now localized without changing their public contracts: server configuration
and startup composition, supervisor checkpoint/physics paths, named play
ingress handlers, simulation/storage/entity test contracts, and the native
script manifest/capability domain. `CommandBatch` remains in the root
admission boundary; `manifest.rs` owns capability declarations, manifest DTOs,
validation and capability matching.

```yaml
base_tree: 1d39e864b291eb3e8c45ba5081b6ce10d40e4c46
implementation_diff_hash: c5c6c2d59ca57c265ab3036dce118d3fa0e38e6d450714a36e15fe877db20b28
changed_files:
  - AGENTS.md
  - README.md
  - crates/mc-net/src/{lib.rs,play.rs,play/session.rs,play/simulation.rs,play/simulation/,play/ingress.rs,play/ingress/,server.rs,server/checkpoint.rs,server/entity_physics.rs,server/tests.rs,server/tests/}
  - crates/mc-server/src/{lib.rs,main.rs,config/,startup.rs,startup/,main_tests.rs,main_tests/,access_control_file_tests.rs}
  - crates/mc-world/src/{storage.rs,storage/tests.rs,storage/tests/}
  - crates/mc-entity/src/{lib.rs,tests.rs}
  - crates/mc-script/src/{lib.rs,manifest.rs}
  - docs/MEMORY.md
validation:
  - .analysis/validation/20260920T151433-test-l2f2ivvk/result.json: mc-script passed
  - .analysis/validation/20260920T151212-test-poeyxry7/result.json: mc-entity passed
  - .analysis/validation/20260920T150433-test-g6e_mxch/result.json: mc-world passed
  - .analysis/validation/20260920T151456-test-h25y1lui/result.json: mc-server passed
  - .analysis/validation/20260920T152602-fmt-108inf37/result.json: formatter passed
  - .analysis/validation/20260920T153324-clippy-biaqd8wa/result.json: strict workspace Clippy passed
  - .analysis/validation/20260920T153411-code-health-o5ty7qc6/result.json: code-health passed
  - .analysis/validation/20260920T154043-correctness-7bw83l4v/result.json:
      formatter, strict workspace Clippy, and code-health passed; workspace tests failed only at the recorded precommit assertion (2298 passed, 8 ignored)
  - codegraph sync .: up to date
  - ArchitectureExtractionReview: pass; no extraction-attributable regression found
status: implementation checkpoint complete; L2 test stage blocked by the recorded mc-net precommit assertion; no commit authorized
next: triage the recorded mc-net precommit failure before treating the full workspace gate as green
```
