# Archived checkpoint log — part 5 of 8

Chronological checkpoint history moved out of `docs/MEMORY.md` so the live cursor
stays small. **Not startup context** (see `AGENTS.md`); read it when a question is
about this era, not to learn the current state.

## Sections in this part

- Previous checkpoint: compact chunk storage and actionable profiles
- Standing architecture objective
- Local C1 work: reapplied over updated origin/main
- Previous checkpoint: published alpha-4 and verified installation
- Previous checkpoint: downloadable Loader preview
- Previous checkpoint: explicit plugin installation and author workflow
- Previous checkpoint: alpha-4 field findings and startup rules
- Baseline and evidence
- Current verified improvement: compact vehicle reads
- Previous verified improvement: shared collision classification
- Previous verified improvement: background CPU headroom
- Previous checkpoint: idle grazing snapshot construction avoided
- Previous checkpoint: duplicated grazing batch validation removed
- Previous checkpoint: dense natural despawn reads reduced
- Previous checkpoint: populated habitat traversal verified
- Previous checkpoint: coastal climate classification verified
- Previous checkpoint: natural riparian wetlands verified
- Previous checkpoint: natural pig population and swimming verified
- Previous checkpoint: raised coastal sand strips removed
- Previous checkpoint: curved rivers and natural inland banks

---

## Previous checkpoint: compact chunk storage and actionable profiles

The owner-approved uniform/shared lighting, 1–3-bit in-memory block palettes and
memory/CPU profile breakdown are implemented. Source remains uncommitted.
No installed binary, owner process, world or configuration was changed.

Light arrays use inline repeated bytes or shared copy-on-write 2,048-byte
payloads. Unknown and computed-zero light remain distinct. Small palettes use
1/2/3 bits in RAM, with valid minimum-four-bit Anvil/wire encoding.
On the same isolated copy of 1,811 stored owner chunks, requested live heap fell
100,825,603→42,859,987 bytes (96.2→40.9 MiB, 57.5% less). Both probes released to
244 bytes after world drop; RSS remained retained. The probe's printed checksum
is an estimated-byte sum, not a semantic content hash.

`profile` now captures actual process RSS/Linux mappings/I/O/thread CPU,
requested live Rust allocations and churn, sorted owner estimates, chunk
categories with shared-payload deduplication, reusable lighting scratch,
prepared/session/entity capacities, and exclusive CPU by subsystem.
Async polling excludes suspension; lock/runnable wait and inclusive wall time
remain separate. Run profile before and after the workload for interval deltas.
The system allocator is unchanged; no trimming or arena tuning was introduced.
The unclassified heap/CPU remainder and non-transactional capture limits are
explicit, not labeled a leak or exact allocator retention.

Final isolated debug-server smoke passed four 1,089-chunk initial streams,
128 unloads and regrowths each, zero retained resends and normal chat.
After explicit `save-all` acknowledgement, profile measured 235.1 MiB RSS and
48.2 MiB requested Rust live bytes: estimated registry 21.4 MiB, light scratch
6.8 MiB, published chunks 1.5 MiB, prepared frames 0.17 MiB and 18.4 MiB outside
classified owners. Capture took 12 ms. The exercised interval attributed
43.5 of 49.4 CPU seconds; other preparation, disk decode and saving dominated.
These are a native debug workload and capacity estimates, not graphical
acceptance, a TPS target or exact RSS ownership. The combined changes did not
regress the measured warmed stream timings; profiler overhead was not isolated
from the storage savings.

Validation: complete workspace 4,500 passed/194 ignored; formatter, strict
workspace Clippy and code-health passed. Final registry/CPU attribution
refinements additionally passed strict workspace Clippy, mc-world 280/15 and
mc-server 76/0, debug build and the native profile/save/stream scenario.
One independent read-only review found no blockers in compact storage and the
initial profiler; Main verified the final attribution refinements.
The existing Lua gameplay-event fixture now moves horizontally off its supporting
block before descending to the item; all event assertions remain and both its
focused test and complete workspace rerun pass. An old telemetry fixture that
only asserted defaults/copied values was removed, not re-pinned.

```yaml
base_tree: 8414d7e05b379b0d5b78e9506036d14bc7f98378
snapshot: .analysis/codex-logs/compact-profile/receipt.json
previous_snapshot: .analysis/codex-logs/river-profile/receipt.json
validation:
  - .analysis/validation/20260910T075629-test-bec1lr2p/result.json
  - .analysis/validation/20260910T081626-code-health-bqumxzwb/result.json
  - .analysis/codex-logs/compact-profile/final-validation.json
  - .analysis/codex-logs/compact-profile/final-after-save.json
resume:
  next: Owner graphical field test of target/debug/mc-server with a fresh world_dir, collecting profile before and after exploration; keep the current sarvar world and installed process unchanged until separately authorized.
```

Earlier aquatic movement, region-reader cache and wider/deeper variable rivers
remain implemented; their evidence is in the previous snapshot and
`.analysis/codex-logs/aquatic-ram/receipt.json`. Worldgen revision remains 20:
the prior river change requires a fresh world rather than rewriting the owner's
revision-19 chunks. This compact-storage/profile checkpoint did not bump it.
`target/debug/mc-server` includes all these changes. `target/release/mc-server`
remains the earlier aquatic/RAM revision-19 build. Subsequently, on explicit owner
request, the current debug build was installed atomically as
`~/.local/bin/solaris`; the previous executable was backed up. SHA-256 matches
the tested source binary and installed `--version` reports `mc-server 0.0.5`.
No running server was restarted and no world/configuration was modified.
Installation receipt and backup location:
`.analysis/codex-logs/compact-profile/installation/receipt.json`.
Published version remains v0.0.5 and
maturity `draft`. Graphical movement/parity and historical failed/manual-pending
scenarios remain unverified; native/Cargo passes do not close them.

## Standing architecture objective

Full core redesign: fewer mechanisms and lines, more reuse and clarity, preserved
performance and vanilla invariants. One broad, uniform, stable addon API must
serve server logic and Loader-backed client features. Replace the documentation
and memory; delete obsolete material rather than preserve legacy guides.

The new target contract is [ARCHITECTURE.md](../ARCHITECTURE.md). It is not a claim
that the runtime has already been migrated. Current plugin API: `0.6.0`;
[current reference](../PLUGINS.md). Loader has one common implementation and
Fabric/NeoForge/Forge adapters.

Owner-approved overload policy: inside declared capacity, vanilla semantics;
outside it, explicit pre-mutation rejection or local delay of expensive work is
allowed to protect the kernel and healthy players. Never discard accepted
mutations, delete items, or partially commit a transaction to relieve pressure.

Owner correction: finish one bounded area through design, implementation and
verification before selecting the next. Do not reopen a whole-core design survey.

Current owner priority: finish RAM-backed asynchronous WAL, then the alpha-3
worldgen and water-walking findings; reduce oversized files, duplication and
one-use helpers; run varied load scenarios and fix measured hot paths. Loader
feature development remains frozen. The owner now authorizes committing and
pushing the entire accumulated core change, with local artifacts excluded, and
creating/publishing the public `solaris-loader` and `solaris-default-plugins`
repositories first so hosted CI can resolve them. The owner subsequently
authorized tagging and publishing `v0.0.3-alpha.1` for curl installation and
manual play, superseding the CI-wait interruption. The existing frozen local
archive remains untouched; tag CI builds new public assets from current source.

Publication completed on `main`: core `f6e9426b`, Loader `6a383c2d`, default
plugins `d71ce84d`. The owner then prioritized the hosted CI failures in
[run 34353147695](https://github.com/kaiserproger/solaris/actions/runs/34353147695).
The Loader fixture required missing `ffmpeg`; a fresh Java build also lacked
the external Minecraft client jar. CI now provisions both, verifies Mojang's
client checksum, and retains failed test/Loader receipts. Three fresh public
checkouts pass 4,462 Rust tests and the full Java profile after prerequisites.
The original hosted Cargo failure's detailed log was not retained, so its cause
is not established and no test or assertion was weakened.
Exact reproduction, correction and hosted follow-up evidence:
`.analysis/codex-logs/ci-fixture-34353147695/receipt.json`.

The owner-authorized public prerelease `v0.0.3-alpha.1` is now published from
`571f53834f33a4f60c430b293dc901e9b1f4f5f7`.
[Tag workflow 34356580571](https://github.com/kaiserproger/solaris/actions/runs/34356580571)
passed all required gates and published Linux x86_64/AArch64 archives and SHA-256
files. The pinned public curl installer, version/config checks, fresh standalone
startup on `127.0.0.1:25565` and clean SIGINT shutdown passed on x86_64.
One independent release review passed; full survival acceptance remains open.
Receipt: `.analysis/releases/public-v0.0.3-alpha.1/receipt.json`.

Owner explicitly requires continuous autonomous execution, without mandatory
checkpoint stops. Evidence snapshots and validation are internal milestones,
not permission gates or reasons to yield. Continue into the next bounded area
unless a real blocker or material owner decision prevents progress.

## Local C1 work: reapplied over updated origin/main

The owner requested an upstream update and explicitly chose to reapply local
C1 rather than park it. Core is based on `98eefb5f`; Loader and default-plugins
already matched their current `origin/main`. Upstream field-test handoff and
release notes above remain intact; this local work does not close those issues.

C1 storage batches/scans/receipts are implemented. Compound inventory/storage
uses one world-journal decision, with durable storage/playerdata projections
before live publication. Recovery includes cursor and open-container inputs,
runs without Lua, and restores checkpoint eligibility. Failed projection
signals world fail-stop while the player inventory remains fenced.

Before this upstream integration, 105 inventory tests passed, including process
crashes and startup without Lua (`artifact://1770`). The broader correctness
run passed formatting but failed Clippy on an unused import and unwired owned
inventory helpers; the import was subsequently fixed. Those earlier results
are not validation of the rebased tree.

Verified pre-update backup, stash identity and synchronization evidence:
`.analysis/codex-logs/upstream-sync-20260910T113724Z/receipt.json`.
Prior implementation evidence:
`.analysis/codex-logs/settlement-c1/owned-inventory-progress.json`.

**C1 remains in progress.** Physical transfers/reservations still need runtime
integration with canonical POI and resident ownership. The owner chose to allow
destruction of containers holding reserved materials, with explicit loss
accounting and no automatic compensation; do not make them indestructible.
Contract: `../solaris-default-plugins/SETTLEMENT_OVERHAUL_CONTRACT.md`.

## Previous checkpoint: published alpha-4 and verified installation

[Solaris v0.0.4-alpha.1](https://github.com/kaiserproger/solaris/releases/tag/v0.0.4-alpha.1)
is published from `60be6039bfc63e97a40299a378f3e66e6a7cac17`.
[Tag workflow 34417692138](https://github.com/kaiserproger/solaris/actions/runs/34417692138)
passed and published Linux x86_64/AArch64 archives and SHA-256 files.
The final local correctness gate passed: 4,471 Rust tests, zero failures,
194 ignored, plus formatter, strict Clippy and code-health. Installer and
harness-check gates passed; the independent release review found no blockers.

The pinned public installer downloaded the x86_64 release without a local
asset override. The installed binary reports `mc-server 0.0.4-alpha.1`;
strict configuration admission loaded all five explicitly installed standard
packages from public package commit `2d51ae5559cdd5b7cbab32888ef2b11d931e3e6e`.
An isolated fresh world started, accepted `status`, `save-all` and `stop`,
then reopened its saved metadata and exited cleanly on SIGINT. Both exits
were zero. The pipe driver required an explicit final LF; its initial wait
timeout is retained in the receipt, not counted as a successful gate.

World identity is schema 4 / worldgen revision 19. Use a fresh alpha-4 world;
do not hand-edit older world metadata past the startup compatibility fence.
These installation and diagnostic graphical checks are not full survival
acceptance. The earlier no-debug survival scenario remains blocked; maturity
remains **draft**. No local AArch64 runtime or packaged-JAR launcher-matrix
claim is made.

Evidence: `.analysis/releases/public-v0.0.4-alpha.1/receipt.json`.
Next outcome: implement and verify C1 durable storage and inventory operations
from `../solaris-default-plugins/SETTLEMENT_OVERHAUL_CONTRACT.md`, followed
by the remaining owner-requested settlement contract and acceptance scenarios.

## Previous checkpoint: downloadable Loader preview

[Solaris Loader v0.1.0](https://github.com/kaiserproger/solaris-loader/releases/tag/v0.1.0)
is published as a prerelease from
`3aaa92663ce8bc3e7de2859ad40ed357aacf3382`, with the three player adapter
JARs and `SHA256SUMS`. Downloaded release assets passed checksum verification.
The README now starts with player installation and permissions, including
instance-local instructions for common launchers; developer/MCP material is
retained below. Forge metadata now fences Minecraft exactly to 26.1.2.

Java tests and real graphical Fabric, NeoForge and Forge Loader gates passed.
Visual review confirmed visible modal assets/buttons, two-owner HUD updates,
owner-local hiding and reconnect cleanup. Vanilla notification overlays
obscure some modal description text. These gates use Gradle client adapters,
not packaged-JAR installation in every launcher; no broader claim is made.

Evidence and release source/assets:
`.analysis/releases/loader-v0.1.0/receipt.json`.

## Previous checkpoint: explicit plugin installation and author workflow

The owner selected explicit opt-in installation. The package repository now
owns `install.sh`: it installs the standard five packages or named selections,
refuses existing package paths, and does not edit server configuration. Core
builds and ordinary core installation remain independent and plugin-free.

The plugin guide now indexes all 32 registered host functions, includes an
executable `/hello` author workflow, and distinguishes dynamic host argument
validation from strict checking of Luau itself. Isolation documentation records
separate VMs on one host thread, 16 MiB per-VM memory, aggregate 50 ms event
budget and maximum 10 ms plugin slices; none is a process-isolation or
whole-server performance claim.

Real-package installation and strict server admission passed. Refused updates
preserved operator configuration; invalid batches, duplicate names, traversal
and dangling destination symlinks were rejected. A real graphical client
received the documented greeting and sandbox checks before and after an
infinite handler disabled only its owning plugin. One independent read-only
review returned no findings.

Evidence: `.analysis/codex-logs/alpha4-plugin-install/receipt.json` and
`.analysis/validation/20260909T230228-regression-w8ebweft/result.json`.
No runtime Rust changed in this slice; prior native gates were not rerun.
Next outcome: player-first Loader documentation and downloadable platform
JARs, verified through the supported launch/install paths.

## Previous checkpoint: alpha-4 field findings and startup rules

On seed `1785772562805887200`, native herd placement now separates pack
members, suitable water supports fish and squid, and generated shallow
sediments include clay. The field probe found a nearby plains tree; a real
graphical client visited that tree, birch forest, and a snowy mountain.
The stricter graphical survey also observed natural cod and squid close up
and raycast natural clay at both the spawn-side pool and river.

Startup `rules.lua` compiles bounded, validated gameplay data once into native
spawn and terrain rules. Its effective identity is persisted with the world;
changed rules reject a restart before listening. Broken manifests cannot hide
a rules owner in permissive discovery. Weighted monster plans consume the
declared entries and counts within native admission caps.

The server now has a typed interactive console and an explicit `--no-console`
mode. Normal logs omit repeated profiling detail. TUI/plain-console and
startup/restart-rejection smokes passed. Workspace tests, formatter,
code-health and strict workspace Clippy passed; one independent read-only
review found two issues, both corrected and covered by focused regressions.

Exact receipts, source snapshot and the failed diagnostic driver attempts:
`.analysis/codex-logs/alpha4-field-1785772562805887200/receipt.json`.
Final graphical gate:
`.analysis/validation/20260909T162959-regression-_fak5g95/result.json`.
This is operator-assisted field diagnosis, not no-debug survival or release
acceptance. Maturity remains `draft`; alpha-4 has not been published.

Next outcome: install selected default plugin packages without bundling their
source into core, with a documented, executable author/deployment workflow.
The supplied settlement overhaul contract remains a proposal; its upstream
capabilities and acceptance matrix are not implemented by this checkpoint.

## Baseline and evidence

- Base HEAD before the authorized publication:
  `638543ab4771f7db9aef93cfaeed7e2fae832312`. Earlier dirty work is included by
  explicit owner choice; local caches, world data and evidence remain excluded.
  Historical no-commit notes below describe their original checkpoints, not the
  current authorization.
- Previous cutover: net −1,198 Rust lines, including tests. Core tests, Clippy,
  formatting, code-health, and independent review passed. This did not close
  broader acceptance. All eleven recorded workspace failures are now closed;
  the full correctness gate and its final affected-target follow-up passed.
- Frozen debug load matrix: 42 runs on one Ryzen 5 7535HS, CPU affinities 1/4/12;
  **20 PASS / 22 FAIL**. Original assertions and workload sizes remain intact.
  Both soaks fail setup; the twelve-CPU 40k/60-client run retains only 7 sessions.
- Exact prior receipt, snapshots, and logs:
  `.analysis/codex-logs/core-overhaul-2026-09-05/receipt.json` and `rnd/` beside it.
  Graphical join smoke passed; full survival/multiplayer and owner terrain
  acceptance were not established. Alpha is still draft, not release-ready.
- Vanilla oracle: `.analysis/server.jar`, verified Minecraft 26.1.2. Reuse it.

## Current verified improvement: compact vehicle reads

- Vehicle graph validation and passenger lookup now read live identity,
  lifecycle and vehicle components instead of cloning every complete entity
  snapshot. The existing vehicle API lives in `entity_vehicle.rs`; no new cache,
  topology authority, scheduling policy or reduced entity selection was added.
- Entity removal uses the canonical ECS passenger unlinking once, without the
  former redundant full-population scan. Atomic graph rejection, lifecycle,
  duplicate-passenger, cycle, rollback and publication rules are preserved.
- In the same instrumented seeded route, unfenced owner-apply median/p95 fell
  from 10.152/18.321 to 4.373/10.872 ms. All 3,738 baseline and 2,575 candidate
  unfenced inputs committed. Work counts differ: these are elapsed distributions,
  not equal-throughput, process CPU or whole-server claims.
- A separate compact fenced batch applied 92 of 130 inputs. Its receipt remains
  explicit; unchanged fence policy and the fully committed unfenced cohort do
  not justify claiming universal input acceptance.
- The external smoke checked 45 graph cases, 18 removals and 360 paired passenger
  lookups. Existing vehicle/kinematics/rollback checks passed, and a permanent
  chain-extension/atomic-cycle regression protects the uncertain graph edge.
- Final correctness passes: 4,462 tests passed, zero failed, 194 ignored;
  formatting, strict Clippy, code-health, debug build and three graphical seeds
  pass. Each route observed land/water/bank transitions, with zero unsupported
  samples in this run. Earlier isolated water evidence remains unresolved.
- Native warning-only maxima remain 119.981 ms whole tick on the owner seed and
  98.495 ms dispatch on seed -17711. This does not close the 50/60 ms gate.
- One independent read-only optimization review passed. All ten temporary
  profiling/configuration/launcher/smoke files were removed after verification;
  their sources and exact evidence remain under
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/entity-dispatch-followup/receipt.json`.
  Publication evidence and exact repository revisions are recorded separately
  in `publication.json` beside it. Alpha remains draft.

## Previous verified improvement: shared collision classification

- Owner-local physics and pathing now reuse `vanilla_collision_class`, already
  used by fallback physics. Empty and full-cube cells avoid repeated binary
  shape decoding. Startup warms this existing immutable table before serving.
- Block reads, snapshot/completeness checks, powder-snow context, complex boxes,
  unknown-state fallbacks and publication fences are unchanged. No per-entity
  block cache, duplicate classification table or reduced simulation work remains.
- On 3,693,246 identical query/snapshot pairs, sampler construction plus physics
  integration averaged 12.330 → 7.448 µs, a 39.6% elapsed reduction. Both execution
  orders improved; the separate identical-implementation control had 0.90%
  aggregate label bias. This is not process CPU or whole-server throughput.
- The final pathing cutover preserved geometry for 29,873 catalog states and two
  out-of-range cases. Existing publication and chicken-AABB checks passed.
  The catalog smoke was temporary, not a new permanent test.
- Final correctness passes: 4,461 tests passed, 194 ignored, formatting, strict
  Clippy and code-health. Debug build and all three graphical routes pass.
  Each observed land-to-water and water-to-bank transitions; sustained
  unsupported samples were zero. Earlier isolated water evidence and broad
  terrain acceptance remain unresolved; still images do not prove support.
- Final owner warning-only tick maximum is 148.525 ms. Dispatch, preparation and
  scheduled-block costs remain open; this is not a whole-tick latency win.
- One independent read-only review passed. Production Rust grew by ten lines;
  all probes and nine temporary launch/config/smoke files were removed.
  Exact comparisons, controls, receipts, source delta and visual limitations:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/physics-sampling-followup/receipt.json`.
  Alpha remains draft; no staging, commit, push, tag or archive replacement.
- Prior publication-tail, periodic-planning and narrow-despawn evidence remains
  in `regional-commit-followup/receipt.json`, `periodic-planning-followup/receipt.json`
  and `unattributed-tick-followup/receipt.json` beside this receipt's parent.

## Previous verified improvement: background CPU headroom

- Background preparation starts and recovers at `max(cpu_capacity - 1, 1)`.
  The shared foreground ceiling, selected simulation work and job retention
  are unchanged. Other foreground users can occupy the headroom; this is not
  a planning deadline guarantee. Single-worker configurations remain serial.
- A forwarding-waker probe attributed 98.516 ms of a 98.597 ms admission sample
  to waiting for a permit, versus 0.081 ms to resume after notification.
  In the matched route, admission p99/max fell from 2.084/98.597 ms to
  0.016/3.020 ms; no candidate acquisition polled pending. Combined
  admission/dispatch/planning p99 fell from 4.956 to 3.205 ms.
- Realized work counts and finished stream windows differ between runs.
  Do not claim equal chunk throughput or an across-the-board speedup.
- Final workspace tests pass: 4,462 passed, 194 ignored. Final formatting,
  code-health, strict Clippy, debug build and three graphical habitat routes
  pass. One independent review accepted the production policy. Later test
  migrations replace call-counter/default assertions with admission/drain
  behavior; obsolete test-only counters were removed.
- Original failed correctness/test receipts remain failed and retained.
  An intermittent roster timeout did not recur in diagnostic runs; the wire
  scenario now waits for the final menu's close, not any buffered close.
  Stale-close causation of that timeout is not established.
- Ten temporary files and the planning probe were removed. Exact source,
  measurements, failures, final gates and review scope:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/scheduled-stall-followup/receipt.json`.
- The owner route still has warning-only maxima of 155.537 ms whole tick,
  118.228 ms unattributed time, 56.793 ms entity dispatch and 23.620 ms
  scheduled-block work. The broad scheduled-block task and 50/60 ms gate remain
  open. Alpha remains draft; no staging, commit, push, tag or archive replacement.
- Previous coastal and session evidence remains in `coastal-boundaries/receipt.json`
  and `session-lock-phases/receipt.json` beside this receipt's parent directory.

## Previous checkpoint: idle grazing snapshot construction avoided

- Every loaded sheep ID still reaches an owner batch. Owners read the current
  ECS timer and construct full snapshots only for active timers or possible
  idle starts. The existing baby/adult phase relationship preserves both start
  schedules; `Some(0)` cleanup and full-state atomic timer CAS remain intact.
- No population cap, cadence change, persistent cache or journal bypass.
  Filtered reads use the coordinator; complete reads retain their direct path
  and do not receive partial route-cache publications.
- Nine controlled debug A/B cases retain exact timers and start sets. At 1024
  sheep, idle median falls 15.785→3.708 ms and mixed 19.439→13.691 ms; all-active
  median rises 41.866→43.339 ms (+3.5%). This is a measured tradeoff, not a
  whole-server latency claim.
- Full L2, debug build and one independent read-only review pass. All three
  unchanged graphical habitat routes pass their existing acceptance rule;
  sampled dry pig movement and complete censuses remain, without reliable
  overflow log lines. One nonconsecutive grounded-over-water observation on
  the owner route remains unclassified; the gate rejects consecutive samples,
  not every isolated observation. Do not claim clean water-surface parity.
- Live grazing still reaches a 51.527 ms owner-route warning; the jungle frame
  shows 14 FPS. Broader grazing, terrain/render and survival acceptance remain
  open. Source, receipts, the surface observation and next outcome:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/sheep-grazing-idle/checkpoint.json`.
- Temporary probe and replay tools are removed with their sources retained.
  No staging, commit, push or tag; frozen archive unchanged. Alpha stays draft.

## Previous checkpoint: duplicated grazing batch validation removed

- In-place conditional replacement batches with unchanged passenger links now
  use owner preparation for full-state comparison, without another coordinator
  snapshot read. Structural, claim, routing, committed-state and atomic
  rollback/journal fences remain. Topology-changing batches retain preflight.
- Removed the redundant grazing-action ID filter; every action belongs to the
  same atomic timer batch. The borrowed planner remains: an ownership cutover
  increased measured planning cost and was rejected.
- Nine matched debug workloads retain every loaded sheep and exact timer
  decrements. At 1024 active sheep, median/p95 fell from 52.756/54.448 ms to
  42.323/46.913 ms on the same four CPUs. Idle reads did not improve consistently.
- Final full L2, debug build and independent read-only review pass. The unchanged
  three-seed graphical habitat routes pass with complete natural censuses,
  sampled dry pig movement and no reliable queue overflow.
- The broader sheep-grazing bottleneck task stays open: idle snapshot reads
  remain material, and the owner route still warns about expensive grazing.
  Dense-canopy client FPS remains low. Warning-only samples and still images
  do not prove full-server latency, seamless distant terrain or survival parity.
- Complete evidence, rejected trial, source and next action:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/sheep-grazing/checkpoint.json`.
  Temporary profiling/replay tools are removed, with sources retained. No
  staging, commit, push or tag; frozen archive unchanged. Alpha remains draft.

## Previous checkpoint: dense natural despawn reads reduced

- Every tracked natural entity now uses the existing compact owner projection;
  only conditional removals need full retained snapshots. Population, cadence,
  distances, idle/damage rules and UUID-based rolls remain unchanged.
- Fifteen unchanged debug cases cover five population mixes and three sizes.
  Homogeneous 1024-entity median read/scan costs fell by 20.3–27.0%. A two-pass
  ground-animal trial was rejected because it regressed other ground species.
- Independent review identified a vanished-snapshot idle-clock leak. Its
  deterministic reproduction fails before and passes after the cleanup; the
  regression remains in a focused sibling test module.
- Full L2 passed before that one-line review correction. Final affected-scope
  formatting, code-health, strict Clippy, 21 despawn tests, debug build and the
  unchanged three-seed graphical habitat routes pass after it. Reliable unload
  batching remains intact; final client logs have no reliable queue overflows.
- The session lock still spans owner reads. Sheep grazing is now the larger
  warned cost on the retained owner route; dense-jungle client FPS remains low.
  Warning samples are not unbiased percentiles or throughput evidence. Neither
  distant render/fog acceptance nor full survival acceptance is closed.
- Receipt, exact measurements, source, review resolution and next action:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/despawn-cost/checkpoint.json`.
  Temporary profiling/replay tooling is removed, with sources retained. No
  staging, commit, push or tag; frozen archive unchanged. Alpha remains draft.

## Previous checkpoint: populated habitat traversal verified

- Fresh revision-18 agent-run graphical Minecraft 26.1.2 routes pass on
  `5617830`, `712816` and `-17711`, with unchanged natural-spawn intervals,
  work budgets and observation deadlines. No animals were summoned or moved,
  and no terrain was edited. Both retained owner barren sites now show jungle
  trees, undergrowth, grass and flowers; river samples include natural mangrove
  habitat, while other seeds retain intentionally open grassy lowlands.
- The final complete 60-block censuses at the two owner sites contain 190 and
  209 land animals. Sampled dry pig movement is observed for 146, 58 and 32
  distinct pigs across the three seeds; no unsupported dry-ground water-footprint
  samples were recorded. This is not full movement parity or survival acceptance.
- The first owner route failed during its third-site view change. Server evidence
  identifies reliable-queue overflow (321 dropped commands) before the secondary
  client-thread timeout. Chunk unload emitted a separate ordered command and full
  snapshot per entity, unlike already batched chunk-load publication.
- Unloading a dense chunk now publishes compact removal IDs in the existing
  `RemoveEntities` packet and ordered reliable lane. Singleton lifecycle
  commands retain their behavior; queue limits and authoritative entities do not
  change. A 400-entity/one-slot receiver regression fails before, then passes
  with exact-once removal and all authoritative entities retained. The unchanged
  real-client route subsequently passes; all three final logs have zero reliable
  backlog overflows.
- Final L2, debug build and independent read-only review pass. Existing uncapped
  spawning, independent frequency/disable controls, collision admission and
  movement/refill tests also pass; the population-policy task is verified.
- Performance remains open: the owner screenshot shows 16 FPS, and warning-only
  samples reach 202.351 ms per tick, 150.476 ms holding the distant-despawn session
  lock and 40.498 ms in sheep grazing. These are not unbiased percentiles or a
  controlled throughput comparison. Distant render/fog rectangles remain visible.
- Full receipts, source, review, failing/passing reproduction and next action:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/habitat-population/checkpoint.json`.
  Temporary executable tooling is removed; replayable sources remain. No staging,
  commit, push or tag; frozen archive unchanged. Alpha remains draft.

## Previous checkpoint: coastal climate classification verified

- Worldgen revision 18 routes ocean temperature variants and snowy beaches
  through the shared inland/riparian climate field in both modes. The unrelated
  deep-ocean noise picker is removed; no extra noise field or height adjustment.
- Existing erosion selects rocky shoreline within the existing two-block band.
  Stony shore now produces gravel over stone rather than the generic beach sand.
  The retained raised bank stays grass. Cold and warm variants remain reachable.
- Both new coastal regressions fail against the inherited routing and pass after
  the fix. Final L2 passes formatter, strict workspace Clippy, code-health and
  all-target workspace tests; the debug server builds. Independent read-only
  review passed without findings.
- Fresh agent-run graphical Minecraft 26.1.2 routes pass on `5617830`, `712816`
  and `-17711`: nine coastal/bank samples retain their expected ground after
  120 live ticks, with connected clients. Inspected aerial and ground images
  show warm jungle/mangrove coasts, snowy inland/coast adjacency, rocky shores
  and the retained grass bank. Before/after height images are byte-identical
  across all three sampled mosaics; this is not exhaustive terrain parity.
- Distant aerial images still show rectangular render/fog boundaries whose
  cause is unestablished. This checkpoint proves nearby coastal classification
  and ground, not seamless distant rendering or vanilla frozen-water features.
  Broader visual acceptance remains open; retained barren habitat is verified above.
- Receipts, images, review, source snapshot and next outcome:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/coastal-climate/checkpoint.json`.
  Temporary executable tooling is removed, with reproducible source retained.
  No staging or commit; frozen archive untouched. Fresh generated worlds are
  required by the revision fence. Alpha remains draft.

## Previous checkpoint: natural riparian wetlands verified

- Worldgen revision 17 replaces blanket humid-lowland swamp coloring with
  river-connected warm/moist shoulders, shallow pockets and dry hummocks.
  Weak dry reaches retain their surrounding climate; no fixed-width ring or
  seed-specific override. River and wetland variants now follow that climate.
- Mangroves have mud, native logs/leaves and soil-anchored, waterlogged roots.
  Temperate wetlands have oaks, grass and blue orchids. Tree placement is shared
  in `terrain/trees.rs`; chunk orchestration shrank from 2,495 to 2,353 lines.
- Fresh agent-run graphical Minecraft 26.1.2 habitat and dry-inland checks pass
  on `5617830`, `712816` and `-17711`. Inspected aerial and ground views show the
  natural transitions. Native trunks remain after 120 live ticks; sampled
  mangrove roots retain source water before and after. Dry inland columns retain
  grass and their surrounding vegetation.
- The first `-17711` run exposed a real disconnect during the second-site stream:
  optimistic cache admission missed retained clean entries and actual pressure
  escaped the storage `try_` APIs as an error. Both try-publication paths now
  return their existing backpressure outcomes for that typed condition, while
  preserving other errors. Cache limits and scenario requirements are unchanged.
  The deterministic reproduction fails before and passes after; the unchanged
  graphical route then passes with 12 deferrals and no disconnect or abandoned
  delivery.
- Final L2 passes formatter, strict workspace Clippy, code-health and all-target
  workspace tests; the revised debug server builds. One independent read-only
  wetland review passed. The later pressure correction has its retained failing/
  passing regression, non-pressure-error coverage and real-client reproduction.
- Evidence, final source snapshot and next outcome:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/habitat/checkpoint.json`.
  Active throwaway tooling is removed; reproducible source and receipts remain.
  No staging or commit; the frozen archive is unchanged. Revision fencing
  requires fresh generated worlds. Alpha remains draft; broader population,
  coastal climate coherence and scheduled-block performance are not closed.

## Previous checkpoint: natural pig population and swimming verified

- Fresh agent-run graphical Minecraft 26.1.2 worlds pass on `5617830`, `712816`
  and `-17711`. Natural pigs populate the retained owner sites and actual river
  neighborhoods; no summons, animal interactions, attacks or animal teleports.
- Same-UUID client samples establish dry movement, immersed movement and later
  grounded dry-bank exits with matching solid footprint scans on all three seeds.
  This is sampled behavior, not full movement parity or a continuous path capture.
- The complete census covers 60 blocks. Wider 128-block queries that reach the
  bridge's 512-entry maximum are explicitly lower bounds, not population totals.
  Spawning remains at 400 ticks / 48 chunks, with view and simulation distance 8;
  no population caps or reduced simulation work were introduced.
- The old water coordinate is now dry ground at `y=81` after the terrain changes.
  New water observations use client-confirmed open rivers, not the obsolete site.
- Inspected images show immersed natural pigs on the owner and `712816` seeds
  and a dry-land pig on `-17711`. The owner land close-up is tree-occluded; exact
  submersion and bank support are established by the retained client state.
- No production changes were needed. The unchanged Rust scope was not rerun;
  one independent read-only evidence review passed. Dense-view screenshots show
  15 FPS, so this does not close the separate performance/load queue.
- Evidence, review, source snapshot and next outcome:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/pigs/checkpoint.json`.
- Active throwaway drivers removed; reproducible source and receipts retained.
  No staging or commit, frozen archive unchanged, alpha remains draft.

## Previous checkpoint: raised coastal sand strips removed

- The remaining strip was a beach, not a desert-climate boundary. Revision 16
  removes the Tellus-only six-block beach height and uses the existing two-block
  shoreline band in both modes. Climate fields and river geometry are unchanged.
- Altitude/climate routing now lives in `terrain/biome_routing.rs`; assembly and
  structure placement consume its shared shoreline bound. The assembly file
  shrank from 2,679 to 2,495 lines without duplicating the moved decisions.
- The retained generated-block regression now keeps raised land grassy while
  preserving sand at the true shoreline. In the 16,384-sample diagnostic window,
  only 2,682 beach samples change to grassland; the height image is byte-identical.
- Final L2 passed: **4,455 passed / 194 ignored**, formatter, strict Clippy and
  code-health. One independent read-only review passed; its source files remain
  unchanged and CodeGraph is synced.
- Fresh agent-run graphical worlds pass on `5617830`, `712816` and `-17711`.
  Inspected images remove the retained raised sand strip. The real client checks
  both the new grassy column and preserved shoreline sand.
- The reported river/raised-beach defects are closed. The swamp-colored lowland
  mismatch left by this checkpoint is addressed by revision 17 above; broader
  population, full survival and general terrain parity are not established here.
- Evidence, final source snapshot and next outcome:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/biome-coherence/checkpoint.json`.
- Temporary client tooling removed; evidence snapshots remain. No staging or
  commit, frozen archive unchanged, alpha still draft.

## Previous checkpoint: curved rivers and natural inland banks

- Revision 15 replaces the two straight halves of a river reach with a smooth
  parabolic bend. Shared endpoints, downstream topology and runoff remain.
  Search bounds include the complete curved reach and its relief-dependent
  width; the old small-scale cell-boundary clipping regression now passes.
- Taller banks widen instead of cutting steeper cliffs. The existing three-block
  terrain-step check passes without relaxing it. Both biome routes now reserve
  beaches for the continental coast; inland banks retain local ground and life.
- Final L2 passed: **4,454 passed / 194 ignored**, formatter, strict Clippy and
  code-health. One independent read-only review passed the geometry change;
  those reviewed files are unchanged. The later coastal-biome correction passed
  its generated-block regression, biome reachability, final L2 and client checks.
- Fresh agent-run graphical worlds pass on `5617830`, `712816` and `-17711`.
  Inspected images show curved reaches and grassy inland banks; the real client
  confirms the formerly sandy regression column is now grass.
- **The broader artificial-terrain item remains open.** The final `-17711`
  view still shows an abrupt sandy biome strip. Biome transitions, population,
  full survival and general performance acceptance are not established here.
- Evidence, before/after regressions, final snapshot and next outcome:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/rivers/checkpoint.json`.
- Temporary probes and client tooling removed; their evidence snapshots remain.
  No staging or commit; the frozen archive is unchanged. Alpha remains draft.
