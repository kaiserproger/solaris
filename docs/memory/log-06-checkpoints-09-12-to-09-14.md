# Archived checkpoint log — part 6 of 8

Chronological checkpoint history moved out of `docs/MEMORY.md` so the live cursor
stays small. **Not startup context** (see `AGENTS.md`); read it when a question is
about this era, not to learn the current state.

## Sections in this part

- Previous checkpoint: full-volume skylight boundary scan removed
- Previous checkpoint: duplicate scheduled-plan admission removed
- Previous checkpoint: resident-accounting lock stalls removed
- Previous checkpoint: jungle undergrowth and leaf initialization
- Previous verified slice: sheep/pig swimming and shore exit
- Previous verified slice: RAM write-behind WAL
- Previous verified slice: local inventory candidates
- Previous verified slice: owner-selected held-item gameplay
- Previous verified slice: core startup data
- Previous verified slice: Loader sounds
- Previous verified slice: unified client UI
- Next outcome
- Owner followup 2026-09-11 (evening, pushed unverified — internet cutoff)
- Tab-list checkpoint 2026-09-12 (uncommitted, L1 green)
- Biome-river checkpoint 2026-09-12 (uncommitted, L1 green)
- Beach-shore checkpoint 2026-09-12 (uncommitted, L1 green)
- Live operator/whitelist checkpoint 2026-09-12 (uncommitted, L1 green)
- River/beach realism checkpoint 2026-09-12 (uncommitted, L1 green)
- Villages + redstone checkpoint 2026-09-12 (uncommitted, L1 green)
- LOC unification and plugin authoring audit 2026-09-13 (uncommitted, draft)
- Shutdown signals and import visibility (landed + verified locally 2026-09-14, uncommitted)
- Vanilla feature executor A1 (partial, internal only — landed uncommitted 2026-09-14)

---

## Previous checkpoint: full-volume skylight boundary scan removed

- Profiling isolated sky-boundary seeding as the dominant chunk-light cost.
  It now derives the same boundary from adjacent columns' open-sky bottoms,
  rather than scanning every volume cell and its six neighbours. Removed the
  one-use dark-neighbour helper; no new cache, executor, limit or bypass.
- Instrumented mean seeding wall time fell from 169.7 ms across 589 calls to
  12.5 ms across 578 calls. Calls are not paired; this is not a whole-server
  throughput ratio. Profiles and original source are retained in the receipt.
- The new sibling regression compares all light values with independently
  seeded full-source propagation across uneven, open, closed and low-opacity
  columns. Existing incremental checks pass, including the explicitly enabled
  `incremental_relight_wire_matches_full_recompute` wire test.
- Full L2 passed: **4,452 passed / 194 ignored**, formatter, strict Clippy and
  code-health. One independent read-only review passed; CodeGraph is synced.
- Both final graphical jungle workloads pass unchanged on `5617830` and
  `-17711`. Inspected images retain trees, undergrowth and lighting. No lock-wait
  warnings or CPU-limit changes occurred. Maximum block ticks are 59.3/39.7 ms,
  down from the preceding 119.6/135.7 ms; slow-tick warning counts are 5/1.
- Occasional tick-budget overruns remain; general performance acceptance and the
  broader scheduled-stalls task are still open. With destination streaming green,
  return to the owner's remaining ordinary terrain/habitat findings before
  chasing the residual performance tail.
- Evidence, source snapshot and next outcome:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/streaming-light/checkpoint.json`.
- Temporary profiling and client driver removed. No staging or commit; the
  frozen archive is unchanged. Alpha remains draft.

## Previous checkpoint: duplicate scheduled-plan admission removed

- The unchanged jungle workload produced 169 sampled slow, nonempty planning
  calls: 5.93 seconds waiting for CPU admission versus 15.8 milliseconds doing
  planning. These are sampled wall spans, not a normalized CPU benchmark.
- The serial path now consumes its first existing plan instead of discarding it
  and entering CPU admission again. Later groups still re-snapshot and replan;
  state/token and due-prefix fences still retain stale work.
- Removed the one-use single-region wrapper. Plan types and synchronous planning
  stay in `scheduled_blocks.rs`; CPU admission and blocking-worker dispatch stay
  in existing play orchestration. The architecture gate caught and rejected the
  initial async-wrapper relocation; it was corrected without a guard exception.
- The strengthened one-permit FIFO button regression fails with the duplicate
  admission and passes without it. Repeated-region and ABA coverage also passes.
  Final L2 scope passed through the individual formatter, code-health, strict
  Clippy and workspace-test profiles: **4,451 passed / 194 ignored**.
  One independent read-only review passed the semantic change; the later
  ownership correction restores the original orchestration boundary.
- Final agent-run graphical harnesses pass on `5617830` and `-17711`, with the
  unchanged 9x9 destination window, 30-second deadline and warning checks.
  Both inspected images show trees and undergrowth; neither run has lock-wait
  warnings. Maximum observed block ticks are 119.6 and 135.7 ms, respectively,
  versus 179.9 and 198.8 ms in the preceding checkpoint's runs.
- **Scheduled-block stalls remain open.** These samples do not establish general
  performance acceptance. The owner-seed run still triggered TickTime CPU
  scaling from 6 to 3 and back to 6; that existing policy was not changed.
- Profiling, before/after regression, final source snapshot, gates, review and
  next outcome:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/scheduled-admission/checkpoint.json`.
- Temporary profiler and client driver removed. No staging or commit; the frozen
  archive is unchanged. Alpha remains draft.

## Previous checkpoint: resident-accounting lock stalls removed

- Profiling the unchanged jungle workload attributed the dominant measured
  chunk-publication lock holds to repeated resident heap-accounting scans.
  Receipts contain the wall-time samples; kernel `perf` was unavailable under
  `perf_event_paranoid=4`, so temporary in-process timing was used and removed.
- Resident/dirty byte totals now update with the existing publication counters,
  using the unchanged heap estimator and before/after mutation footprints.
  Admission no longer clones and scans every resident chunk. Budgets, save-health
  checks, clean eviction and cross-region publication fences remain unchanged.
- Counter storage shares one `Arc`; no additional executor, resident authority,
  cache limit or lock. This also avoids enlarging resident transaction values.
- The lifecycle regression covers admission through growth, shrinkage, dirty
  flush finalization and clean eviction. **4,451 tests passed / 194 ignored**.
  Final formatter, strict Clippy and code-health gates passed. The initial L2
  attempt exposed the enlarged enum; after compacting counter storage, the
  remaining L2 scope completed through the individual harness profiles.
  One independent read-only source review passed.
- **Both full graphical harnesses now pass** on `5617830` and `-17711`, preserving
  the exact 9x9 destination window, 30-second deadline and warning checks.
  Neither run logged a lock-wait warning; inspected images still show jungle
  trees and undergrowth. The prior chunk-publication lock-wait failure is closed.
- **Scheduled-block stalls remain unresolved.** Both runs still logged long
  block ticks. These graphical passes are not general performance acceptance.
- Source snapshot, profiling, final gate receipts, review and next outcome:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/streaming-accounting/checkpoint.json`.
- The preceding queue-backpressure correction and closure of the water-walking/
  sparse-jungle finding remain recorded in `streaming/checkpoint.json` beside it.
- Temporary profiling and client driver removed. No staging or commit; frozen
  archive unchanged. Alpha remains draft.

## Previous checkpoint: jungle undergrowth and leaf initialization

- Revision 14 mixes low jungle-log/oak-leaf bushes with ordinary jungle trunks
  of 4–12 blocks. Existing density, exact-surface, stable-5x5 and chunk-margin
  fences remain. Mega trees, vines and full vanilla jungle parity are not claimed.
- Natural leaves are initialized to their nearest in-chunk supporting logs
  before publication, with `persistent=false`. Generation and scheduled runtime
  updates share the existing `mc-world` plant support rule. Removed the one-use
  leaf-state helper. There is no separate CPU admission or permanent-leaf bypass.
- Full L2 passed: **4,449 tests passed / 194 ignored**, strict workspace Clippy,
  formatting and code-health. The nearest-log/unsupported-leaf boundary test,
  three-seed bush/tree test and existing scheduled log-removal propagation test
  all passed. One independent read-only source review passed; CodeGraph is synced.
- Agent-run graphical seed `712816` shows taller trees and low bushes in a
  natural valley. Seed `5617830` client block observations contain natural leaf
  distances 1–3, and its pre-generated startup terrain renders.
- At that checkpoint, destination streaming remained blocked: `5617830`
  showed all-sky views and failed the stronger 9x9/30-second gate; `-17711`
  failed even the original four-corner gate. The current checkpoint above
  supersedes those destination failures, not the remaining performance failures.
- Profiling measured shared CPU admission dominating slow scheduled ticks;
  CPU capacity fell to 1 and view distance changed 8→7→6. The leaf correction
  storm was reduced. All temporary runtime profiling was removed.
- L2 receipt:
  `.analysis/validation/20260906T184530-correctness-pr61axb6/result.json`.
  Source snapshot, review, visual evidence, failed gates and next reproduction:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/jungle/checkpoint.json`.

This closes the source capability, not the complete jungle finding or alpha
acceptance. The old alpha archive is unchanged; no staging or commit is authorized.

## Previous verified slice: sheep/pig swimming and shore exit

- Agent-run graphical Minecraft 26.1.2 verified immersed ascent and actual
  grounded dry-bank landings for both sheep and pigs. The final fixture uses
  existing water corridors with two-block dry banks; support is checked across
  the entity footprint, not only its center. Both landed at `y=95`,
  `on_ground=true`, `in_water=false`.
- Inspected screenshots show immersed bodies and subsequent dry-stone landings.
  Exact underwater foot depth is established by client state, not guessed from
  the water-obscured images. A cod control remained in water for 20 samples;
  its screenshot does not resolve the fish, so that control is state evidence.
- Local vanilla bytecode confirms depth-gated float jumps and the `0.3/tick`
  bank impulse. Solaris uses bounded deterministic lift, not full vanilla
  probabilistic movement parity. Corrected the stale head-submersion comment.
- One independent read-only review passed. Formatter and code-health passed;
  CodeGraph is synchronized. The preceding full workspace gate already covers
  the unchanged physics behavior; this checkpoint adds real-client evidence.
- Final graphical receipt:
  `.analysis/validation/20260906T164401-regression-oopflzwf/result.json`.
  Snapshot, state measurements, images, review and rejected probe assumptions:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/swimming/checkpoint.json`.

This closes the sheep/pig buoyancy findings, not jungle density, natural-world
population, river/biome appearance, full movement parity or load acceptance.

## Previous verified slice: RAM write-behind WAL

Chunk and regional-entity decisions share one world-owned writer, bounded queue,
grouped disk sync and failure signal. Gameplay acknowledges RAM acceptance;
the owner explicitly accepts losing the unflushed tail on a crash. Save-all,
background/pressure dirty flush and clean shutdown fence accepted work before
acknowledging durable storage. The writer owns the world lease until shutdown.
The contract is in [ADR 0005](../decisions/0005-regional-simulation.md#journal-durability).

- Removed synchronous reservation rewrites, separate journal workers and obsolete
  persistence bypasses. Moved the substantial journal tests to a sibling file;
  removed the unused `fd-lock` dependency.
- Final workspace tests: **4,448 passed / 194 ignored**. Workspace strict Clippy
  passed; later worldgen/xtask edits passed focused strict Clippy. Final formatter
  and code-health passed. Original failed receipts and successful follow-ups
  remain preserved; the original composite receipt was not rewritten as green.
- The full test run caught a tree-placement regression. Restored the existing
  surrounding-terrain stability guard; the original assertion and exact failing
  test remain intact and pass. Removed three stale code-health anchors that
  required deleted passive-spawn gating and adapter wrappers.
- Agent-run graphical Minecraft: mining changed a block to air; clean shutdown,
  restart/rejoin, recovered block state and manual save-all passed. The actual
  in-game screenshot was inspected. Survival pickup was interrupted by player
  death; crowded-world performance remains unresolved. Neither is called green.
- One executing independent WAL reviewer found no evidence-backed corruption
  or false durable-save acknowledgement. The old field reviewer could not be
  revived; its earlier verdict was not reused as WAL evidence.
- Receipt, owned snapshot, exact gates, review and runtime evidence:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/wal-write-behind/checkpoint.json`.

This closes WAL delivery, not the full owner request. Alpha remains **draft**.
The delivered local alpha archive remains frozen and was not overwritten.

## Previous verified slice: local inventory candidates

Window-0 clicks, recipe-book crafting, offhand swaps and normal/debug grants
prepare local inventory/cursor candidates. The connection baseline stays
unchanged until the existing owner returns a committed or rejected snapshot.
Removed speculative projection writes, rollback branches and redundant clones.
Other menu handlers and broader core ownership remain unfinished.

- L2 passed: **4,449 tests passed / 194 ignored**, code-health, workspace Clippy
  `-D warnings` and formatting. Java bridge-core/java-agent tests passed.
- Real Minecraft passed natural mining/pickup/economy, one oak log → four planks,
  cursor pickup/return of two apples, and selection of the crafted stack:
  `.analysis/plugin-client-compat/20260906T145930/result.json`.
  Crafting setup used an isolated operator fixture; this is not full survival,
  multiplayer, performance or owner terrain acceptance.
- The old crafting scenario used fixed recipe ID 697; the server advertises 18.
  It now uses existing recipe-book lookup. Two tests pinning mocked call order
  and the fixed ID were removed rather than repinned. Actual count assertions
  remain in the real-client scenario.
- One independent read-only review passed. The throwaway smoke launcher was
  removed; its source and observations remain with the evidence.
- Receipt: `.analysis/codex-logs/core-inventory-candidates-2026-09-06/checkpoint.json`.
  No commit, staging, push or tag.

## Previous verified slice: owner-selected held-item gameplay

Route: `architecture`; contract: `docs/ARCHITECTURE.md` and ADR 0006.
Removed `InteractionState.selected_hotbar_slot` and its constructor/update paths.
Mining, combat, item use, shields, arrows and drops now read the exact owner's
shared selected slot. No replacement cache or compatibility path was introduced.

- Reproduced a stale gameplay read: after owner selection of an enchanted weapon,
  connection gameplay still computed **5 damage instead of 7**. The regression
  now passes and also covers invalid selection preserving the weapon and return
  to the original slot. Existing arrow-selection coverage remains.
- L2 passed: **4,449 tests passed / 194 ignored**, code-health, workspace Clippy
  `-D warnings` and formatting. One independent read-only review passed.
- Agent-run graphical Minecraft gate passed without changing this checkpoint's
  scenario: `.analysis/plugin-client-compat/20260906T142622/result.json`.
  Natural mining/pickup, economy purchase and ledger persistence passed.
- Receipt, checkpoint-local diff and validation logs:
  `.analysis/codex-logs/core-player-selection-2026-09-06/checkpoint.json`.
  No commit, staging, push or tag.

This closes held-item selection authority, not all player/session mirrors or the
whole core redesign. Inventory/cursor projections and broader ownership remain.

## Previous verified slice: core startup data

Route: `architecture`; contract: `docs/ARCHITECTURE.md` and ADR 0006.
`mc_server::startup_data::StartupData` assembles and validates immutable gameplay
tables before terrain/world preparation. The CLI orchestrates the resulting
bundle; per-table `Effective<T>` wrappers are removed.

- Runtime smoke: malformed recipe output is rejected before creating a new
  world or changing an existing sentinel world. Configuration-only `--check`
  still emits valid JSON and does not create a world.
- Agent-run graphical Minecraft compatibility gate passed:
  `.analysis/plugin-client-compat/20260906T140305/result.json`.
  Natural dirt mining/pickup, economy purchase and persisted ledger assertions
  passed. The scenario now clears natural snow cover before harvesting dirt;
  positive pickup and purchase assertions remain intact. This is not an
  unchanged harness run or a full survival/multiplayer acceptance gate.
- L2 passed: **4,448 tests passed / 194 ignored**, code-health, workspace
  Clippy `-D warnings`, formatting; Java bridge-core and java-agent tests passed.
- One independent read-only startup-cutover review passed without findings.
  This review predates the subsequent snow-clearing harness adjustment.
- Evidence and owned-change receipt:
  `.analysis/codex-logs/core-startup-2026-09-06/checkpoint.json`.
  No commit, staging, push or tag.

## Previous verified slice: Loader sounds

Route: `plugins`; current contract: `docs/PLUGINS.md` and ADR 0010.
`play_client_sound`/`stop_client_sound` use owner `sounds` definitions,
`play_sounds`, verified mono OGG assets and the existing resource pack.
One shared native presenter supports personal and fixed-position one-shots,
volume, pitch, vanilla attenuation, owner-local stop and disconnect cleanup.
Loader wire protocol remains **2**; plugin API remains `0.6.0`.

- Final agent-run real Minecraft 26.1.2 MCP/Xvfb audio gates passed:
  `20260906T131829-fabric`, `20260906T131958-neoforge`, and
  `20260906T132202-forge`, under `.analysis/loader-live-gate/runs/`.
  Recorded actual audio proves quarter-volume scaling, 440→660 Hz pitch,
  near/mid/far attenuation, concurrent owners, owner-local/foreign stop,
  disconnect silence, reconnect silence and newly requested playback.
  Both owner frequencies are zero after the final stop in all three runs.
  Post-play and clean-reconnect screenshots were also inspected.
- One independent read-only reviewer found one P2 gap in the audio gate:
  Sapphire's final stop was recorded but not asserted. Both owner frequencies
  are now asserted silent. A real-client dropped-Sapphire-stop probe correctly
  failed with `stop_client_sound left owner audio audible`
  (`20260906T131622-fabric`); the expected-rejection wrapper exited successfully.
  No second reviewer was spawned.
- Initial failures are preserved: Rust's closed artifact index lacked `sounds`;
  the audio capture initially listened to a different sink; an adversarial run
  then exposed background-music interference. The isolated client now selects
  its private sink and disables vanilla music, without changing the desktop sink
  or relaxing spectral thresholds.
- L2 passed once: code-health, **4,450 tests passed / 194 ignored**, workspace
  Clippy `-D warnings`, and formatting. Focused sound-boundary tests, all four
  Java Loader test tasks, three distributable jars and reproducible fixtures
  passed. Only owned Rust files were formatted.
- No loops, moving sources, completion events or arbitrary client scripts.
  Forge retains owner-approved isolated `earlyWindowControl=false`; these runs
  are not owner Prism/terrain acceptance, performance or full-alpha evidence.
- No commit/staging/push/tag. Preserve inherited dirty work.

Exact receipt, review resolution, source snapshots and audio/visual evidence:
`.analysis/codex-logs/loader-sound-2026-09-06/checkpoint.json`.
The preceding input slice is closed; its full evidence remains at
`.analysis/codex-logs/loader-input-2026-09-06/checkpoint.json`.

## Previous verified slice: unified client UI

- One `solaris.present_client_ui(player_id, ui_id, options)` command handles
  `screen`, `hud`, and `hidden` through `ScriptBoundary`. The `ui` resource
  schema, `present_ui` permission, and `solaris:loader/ui` transport replace the
  screen-only API without aliases. Exact-session and owner fences remain.
- One shared Minecraft presenter owns modal item/block/action views and
  non-interactive HUD panels. Text overrides are bounded; omitted values come
  from the verified resource, not earlier dynamic state. Hiding is id-local;
  activation/disconnect clears HUD state.
- Agent-run real Minecraft 26.1.2 MCP/Xvfb gates passed on Fabric, NeoForge and
  Forge. Captured images confirm both owner HUDs, text updates, owner-local
  hiding and no stale HUD after reconnect; ordinary jump input remained live.
  Capture now waits for the actual loading overlay to disappear, not merely Play.
- Forge's default FML early-window GL-context handoff fails on this Xvfb host,
  before Solaris initializes. The owner approved `earlyWindowControl=false` in
  the isolated QA profile. The normal game renderer/HUD passed; early-window
  GL features are not covered. This is not owner-run Prism/terrain acceptance.
- Full L2 passed: **4,449 tests passed, 194 ignored**, code-health, workspace
  Clippy and formatting. Java core and all three adapter test tasks passed.
  The missing non-exhaustive API marker was fixed; a runner deadline interrupted
  Clippy, which then completed separately without replaying successful tests.
- One independent read-only review passed without findings. The later
  non-exhaustive invariant and approved QA-profile setting are recorded in the
  receipt. No load/performance or full-alpha readiness claim; no commit,
  staging, push or tag.

UI receipt:
`.analysis/codex-logs/unified-client-ui-2026-09-05/checkpoint.json`.
The preceding configuration/workspace receipt remains
`.analysis/codex-logs/configuration-workspace-2026-09-05/checkpoint.json` and links
the earlier NPC, movement/pickup, inventory and world-commit evidence.

## Next outcome

First require the correction's hosted `test` and `loader` jobs to pass; local
clean-checkout passes do not clear a hosted failure. Then attribute and reduce the
remaining dispatch tail on retained seed -17711, with the owner jungle route
as a comparison. Separate computation, admission and owner-response waits before
choosing another narrow change; do not reduce selected entities or reorder
goal/gameplay/physics phases.

The final native negative-seed trace reaches 98.495 ms entity dispatch and
41.896 ms physics preparation; the owner trace retains 24.754 ms block work.
These are warning-only maxima, not whole-run percentiles. Acceptance: lower
matched attributed cost with unchanged selection, transaction and publication
fences, plus the retained three-seed graphical routes. Evidence:
`entity-dispatch-followup/native-stage-maxima.json` beside the owner evidence root.
Primary route: `docs/decisions/0005-regional-simulation.md`.
Continue autonomously after recording evidence; this is not a checkpoint stop.

Keep broader visual acceptance open; the radius repair does not establish full
terrain, frozen-water or client-frame parity.
The latest owner route also retains one unclassified, nonconsecutive
grounded-over-water pig observation in the idle-grazing receipt. Preserve that
evidence; a passing sustained-surface gate does not clear shorter artifacts.

Keep residual scheduled-block latency in the measured performance queue, not
marked complete. Grid population was the largest light stage in the retained
streaming-light profile. The final session-contention route also retained a
28.305 ms apply-despawn session hold; do not claim all session stalls eliminated.
Preserve settled accounting, queue-backpressure, first-plan reuse and
leaf-initialization fixes.

Then finish the bounded refactoring and varied load matrix. The original
sheep-grazing bottleneck is resolved, not a reason to repeat its completed work.
Do not restore population caps or reduce selected simulation work to hide costs.

Broader core ownership, integration finalization and full gameplay/load acceptance
remain open. The old frozen load matrix and missing owner terrain acceptance
remain unresolved. Do not return to Loader feature development.

## Owner followup 2026-09-11 (evening, pushed unverified — internet cutoff)

Done and tested: squid first-hit regression
(`squid_first_melee_hit_damages_and_notifies_observers`), panic multipliers
from decompiled 26.1.2 (cow 2.0, sheep/pig 1.25, chicken 1.4), zombie pursuit
from attribute (`ZombieAttackGoal 1.0` over 0.23 → 2.3).
schedule removed. New `skeleton_bow_draw_cycle` passes; `hostile_commit_releases`
and `skeleton_shoots_a_real_arrow` pass. `skeleton_volley` FAILS on the owner-request
count (9 vs old 5: gameplay `attacks == 2` passes, only the lock-count expectation
UNVERIFIED: `skeleton_volley` owner-request count re-baselined 5 → 9 without
a green run (no time before disconnect). First act on reconnect: run the four
bow tests + `cargo fmt --all` + full `mc-net`/`mc-entity`/`mc-data` lib suites,
then L2 `correctness`. `SKELETON_SHOT_PERIOD_TICKS` const removed; do not
re-add. `.analysis/server.jar` (official Mojang download, ignored) + vineflower
decomp under /tmp/decomp (Cow/Sheep/Pig/Chicken/PanicGoal/Zombie/Skeleton/
AbstractSkeleton/RangedBowAttackGoal) back further vanilla checks.

## Tab-list checkpoint 2026-09-12 (uncommitted, L1 green)
Config-driven tab list: `ClientboundTabList` (0x7A, header/footer NBT),
`[tab_list]` in `example.toml`, login burst sends full roster + header/footer
(skipped when both empty), join broadcasts add, leave broadcasts remove,
game-mode changes broadcast `UPDATE_GAME_MODE`. Roster entries carry live
`game_mode` (was hardcoded 0). `OutboundCommand::PlayerInfo/PlayerInfoRemove`
dispatched via trailing `Some(cmd)` arm + `write_player_info` helper
(play_loop_inner gateway budget 731 holds). Fail-closed codec kept: test
profile renamed `InitialRecipeSync` (17) -> `InitialRecipe` (16) instead of
truncating production names. Removed dead `OutboundCommand::TabList` variant.
Gates: mc-net lib 2090/0, mc-protocol 321/0, fmt clean, code-health PASS,
clippy clean except pre-existing furnace collapsible-if (FurnaceBoats-owned).
NOT committed (no authorization). Biome blotches seed 9700063978612627 still open.

## Biome-river checkpoint 2026-09-12 (uncommitted, L1 green)
Seed 9700063978612627 diagnosis (TellusLike): macro continents healthy (2km+
masses, smooth J/G/D borders on 8km map); sampled lowland river textbook
(sand bed, water 2-5, clean banks). Real defect: weak upland carves routed
as warm_ocean stripes (river field 0.03 at w~0.7 missed the 0.016 band that
only covered the w>0.84 core) with grassy savanna shallows at y62-63.
Fix: TELLUS_RIVER_BIOME_WIDTH 0.016->0.04 (covers carve to w~0.6, open water
stays 1.0->ocean; 0.05 tried first but ate the riparian-wetland shoulders
pinned by generated_riparian_wetlands test). No river/swamp reorder, no
field rescale (both wider blast radius). WORLDGEN_REVISION 20->21 (biome
layout change; old worlds must wipe per startup gate). Regression:
tellus_carved_channel_routes_river_wall_to_wall. Gates: mc-worldgen
138+1+12/0, fmt clean, code-health PASS. Straightness: mapped reaches
meander; residual downhill-chain coherence is a known accepted tradeoff
(branch jitter 0.22, see drainage comment). NOT committed.

## Beach-shore checkpoint 2026-09-12 (uncommitted, L1 green)
Seed 1785772562805887200 field report (screenshots): grass underwater at the
waterline + beach sheets across flats. Transect (1283,1740-1832) reproduced
plains y62 + water over grass. Fix 1 (kept): below-sea non-river/swamp/ocean
land routes to shore in Tellus (`tellus_sub_sea_land_routes_shore_not_grass`).
Fix 2 (kept): Tellus shore capped at sea+1 (`tellus_beach_stays_near_waterline`;
y65+ sheets -> climate, y63-64 fringe stays). sea+0 tried, reverted (killed
y64 berm, broke 2 coastal tests). Rivers on this seed verified healthy
(sand beds, water 2-5, meandering with anabranching knots at confluences).
Revision stays 21. Release binary rebuilt+reinstalled with all fixes.
Queued tuning (owner): rougher meanders, softer biome transitions, more land
share. (Live operator/whitelist landed 2026-09-12.)

## Live operator/whitelist checkpoint 2026-09-12 (uncommitted, L1 green)
Console `operator add|remove` and new `whitelist add|remove|list` now take effect
without a restart. Single live source: `CommandPermissionConfig.operators` and
`LoginAccessConfig.whitelist` are `Arc<ArcSwap<BTreeSet<String>>>`; login reads
the whitelist at the check, and chat-command ingress re-resolves op via
`live_permissions_for` (loopback dev fallback stays valid only while no operator
is configured) and re-sends the command tree when it changed. `operator list`
and `whitelist list` report the effective live set.
Persistence mirrors the file manager: `manage_access_file` with
`AccessControlTarget::{Operators,Whitelist}`, console defaults `ops.json` /
`whitelist.json` beside the config, and startup auto-loads `whitelist.json` when
`auth.whitelist_file` is unset (as it already did for `ops.json`).
`operator_file_tests.rs` renamed `access_control_file_tests.rs`.
Known limit: enforcement toggle (`whitelist_enabled`) and online clients'
initial command tree still need a config edit/reconnect; the F3+F4 game-mode
packet path uses login-time authority until reconnect.
Gates: mc-net lib 2093/0, mc-server 78+40+39+14/0, fmt clean, code-health PASS,
`clippy -D warnings` clean (furnace collapsible-if collapsed). Release binary
rebuilt + reinstalled.

## River/beach realism checkpoint 2026-09-12 (uncommitted, L1 green)
Owner field reports on seed 1785772562805887200 (screenshots): grass under
water, beach sand cutting dry valleys, rivers starting at full width out of
nowhere, no gravel in river beds, no savanna found.
- Beaches now require adjacent water: `tellus_biome_for` probes one step (6
  blocks) for a sub-sea neighbour, so a y64 flat 140 blocks from water keeps
  its climate instead of a sand band (`tellus_beach_never_cuts_dry_ground_inland`).
  The two coastal design tests now pin a shoreline column (seed 712816 at
  -448,-32) because synthetic inland samples no longer qualify.
- River beds mix sand with gravel bars: two-octave field, scale 34, threshold
  0.30 (~16% of bed measured), `river_beds_carry_gravel_bars_between_sand`.
- Headwaters taper: Tellus only, `drainage::sample(..., taper_headwaters)`
  scales the minimum channel width by reach strength. VanillaLike keeps the
  flat 20-block minimum so its terrain and the 3-block step budget stay
  byte-identical (measured: Tellus worst step 3, 0 violations; VanillaLike
  continuity test green again).
- Savanna exists on that seed (hot_dry is ~4% of land, vanilla-like):
  first at 1840,-3072, also 2176,-2928, 2560,-2656, 2176,-2544, 2576,-2544;
  a plains at 2848,-2464. No code change.
- Villages: diagnosed as unreachable by default. `structure_rules_for_startup`
  only builds village rules from a Luau settlement plan
  (`PreparedLuaPlugins::worldgen_settlement_plan`) plus `data.vanilla_data_dir`;
  `example.toml` sets neither, so a stock server generates zero villages.
- WORLDGEN_REVISION stays 21 (still unpublished). Binary rebuilt+reinstalled
  at ~/.local/bin/solaris (17:28).
- Queued owner asks: tectonic-plate canyons/mountains, Chunky-style bounded
  pregeneration, more realistic mob spawning, village availability, redstone
  and pistons (delegated to the `RedstonePistons` subagent, uncommitted).

## Villages + redstone checkpoint 2026-09-12 (uncommitted, L1 green)
Villages are reachable without a plugin: `[data] settlement_profile =
"plains_village_prototype"` builds the vanilla plains prototype from
`vanilla_data_dir` (still required; Mojang NBT never enters Git). A deployed
plugin settlement plan still wins and stays the recorded identity, otherwise the
built-in profile name is recorded. Verified end-to-end locally: startup logs
`materialized built-in settlement prototype profile="plains_village_prototype"`
and the sidecar tests now skip-when-absent instead of `#[ignore]`, asserting the
built-in profile changes >200 generated blocks around the fixed centre.
Redstone/pistons landed by the `RedstonePistons` subagent (uncommitted):
`crates/mc-net/src/play/redstone/` (power model, event-driven settle,
atomic piston moves, 17 new tests) plus the old one-hop power fanout deleted
from `toggles.rs`; fence = 256 positions/settle, 1024/tick, 64 ticks/commit,
work beyond a cap is dropped and counted (`budget_drops`) rather than deferred.
Deviations are documented in the module header (no piston animation packet,
no strong-power relaying, one scheduled tick of latency for non-interactive
edits, no quasi-connectivity).
Known flake, not fixed here: `plugin_owned_command_argument_limits_do_not_
terminate_play_ingress` (2s script-event budget; 1 pass / 3 fail under load).
A/B evidence: it still flakes with the tab-list burst disabled and with the
live-permission refresh disabled, so it is load-sensitive, not a feature
regression. `crates/mc-server/tests/play.rs` is unmodified apart from the
required `tab_list` struct-literal lines.
Gates: mc-net lib 2110/0, mc-server 79/43/2/39/14/0/12/1 (+the flake),
mc-worldgen 141+1+12/0, fmt clean, `clippy -D warnings` clean for the three
crates, code-health PASS. Binary rebuilt + reinstalled (~/.local/bin/solaris).

## LOC unification and plugin authoring audit 2026-09-13 (uncommitted, draft)

Whole tracked source/config inventory: 817 files, 542097 physical lines across
all 13 crates and tooling; exact-clone scan plus targeted semantic audits.
This is not a claim that all similar code is interchangeable. Owned code delta:
**-730 lines**, including new Luau declarations and authoring regression tests.
Shared registry decoding, the common TCP Play handshake (16 test files),
identical two-client harness setup/screenshots, script-host dequeue policy, and
identical noise interpolation formulas now have single implementations.
Different handshake, preflight, floating-point, and authority contracts remain
separate rather than being forced through a generic abstraction.

Plugin discovery now checks 61 real host functions against bundled Luau
declarations instead of typing `solaris` as `any`. All ten first-party package
sources typechecked; an actual strict-loaded command and simulation timer
reached a real TCP client. Advanced result records remain dynamically typed and
runtime-validated; durable async request/result correlation is still explicit.
Plugin docs now use the real completion callback and observed inventory fence,
and no longer advertise the removed ephemeral villager bindings.

Evidence: `.analysis/codex-logs/loc-unification/receipt.json` records
`base_tree`, `diff_hash`, the 31 owned `changed_files`, validation, and next action.
Base: `5ebb33c413d2017f0256f67e5445934a817d7da4`.
Owned patch SHA256:
`b9f55c01c4bc4be3290f0ff3d11aebec461a426032aa79f8b59b8ab8ead87b6e`.
The patch excludes this append-only cursor record and concurrent settlement
changes, which were preserved. No staging, commit, push, or sibling-source edits.

Validation: canonical `correctness` passed (4763 passed, 192 ignored), receipt
`.analysis/validation/20260913T165448-correctness-7hjn9uiy/result.json`;
`harness-check` passed; two normally ignored multiplayer presence scenarios
passed with real TCP clients; 48 old/new Python driver trace comparisons passed
with substituted bridge calls. Independent read-only sonic review passed.
No graphical Minecraft client gate was run (client credentials absent); this
does not establish gameplay parity or close any failed owner scenario.

Owner requested quiet operation during closeout. All owned build/test jobs had
finished; no further heavy work was launched, and power saver was left untouched.
Next: review the scoped uncommitted patch while preserving concurrent settlement
work. This appendix does not advance the existing route cursor.

## Shutdown signals and import visibility (landed + verified locally 2026-09-14, uncommitted)

- SIGTERM is now handled exactly like Ctrl-C. `run_bound_server` waits on
  `tokio::signal::unix::signal(SignalKind::terminate())` in the same `select!`
  as `tokio::signal::ctrl_c()`, so `kill`, systemd and container stops drain
  admitted work and perform the single final save. Before this, SIGTERM killed
  the process by default disposition while Ctrl-C was already correct.
  Evidence (`/tmp/sigverify.sh`; isolated dirs and ports, readiness proven from
  each child's own `Solaris is listening` line before the signal): SIGTERM and
  SIGINT both now exit 0 with `shutdown requested; listener stopping`,
  `save-all complete … context=server run final save`, `Luau plugin host stopped`.
- Automatic content import is legible and no longer rotates the server's own log.
  The import reports one terminal line per phase plus byte-throttled download
  checkpoints (`downloaded 8.0 MiB / 57.6 MiB (13%)` …), driven by the response
  stream rather than a timer; the in-tree extractor and datagen JVMs run with an
  explicit `current_dir`, so vanilla log4j can no longer rotate
  `<server cwd>/logs/latest.log` out from under the launcher (observed before the
  fix: the launcher writing into an unlinked inode, `/proc/<pid>/fd/9` pointing at
  `logs/<date>-1.log.gz (deleted)`, `latest.log` at 0 bytes).
  Evidence: cold default-console run transcript; `/tmp/acc2` contains only
  `logs/latest.log` (42 lines) plus `debug.log`, no `*.log.gz`; cache published
  `registries=28 entries=382 in 30.4s`.
- Still not done: the interactive console does not render tracing records at any
  level (`ConsoleOutput`'s ring is a different object from the ring `init_tracing`
  builds, and `--no-console` only flips `interactive`). The import phase lines are
  a terminal sink; general log records are still file-only.
- Undiagnosed and unchanged: a cold start can still fail on the manifest fetch with
  `client error (Connect): operation timed out`. No retry/backoff/sleep mechanism
  was added. The IPv6-only explanation is NOT established — in this session the
  host resolved `piston-meta.mojang.com` to IPv4 `150.171.109.106` and `curl`
  returned 200 in 0.95s.
- Installed build: `~/.local/bin/solaris` md5 `78a91daf6fd31bceb796cfb8c6451c2d`
  (mc-server 0.0.6, release). `cargo test -p mc-server --bin mc-server --
  content_import::tests` 16 passed / 0 failed; `cargo clippy -p mc-server
  --all-targets -- -D warnings` clean; `run code-health` passed
  `.analysis/validation/20260914T054745-code-health-bivycely`. `run fmt` currently
  fails on another agent's in-flight `crates/mc-data/src/vanilla_feature_closure.rs`,
  not on these changes.

## Vanilla feature executor A1 (partial, internal only — landed uncommitted 2026-09-14)

Data-driven execution of the vanilla placed/configured features reachable from the
village `feature_pool_element` entries, scoped to A1 (no trees yet, no villages).
Reachable closure pinned from the derived data: `minecraft:simple_block` ×3
(flower_plain, berry_bush, taiga_grass), `minecraft:block_pile` ×5 (hay, ice,
melon, pumpkin, snow), `minecraft:block_column` ×1 (cactus), and exactly the
modifiers `count`, `random_offset`, `block_predicate_filter`.

- `mc-data::vanilla_feature_closure` resolves by reference from the named root
  only and fails closed (`UnsupportedType{kind, type_id, referrer}`) inside it;
  entries outside the closure are never read. `minecraft:tree` is NOT accepted.
- `mc-worldgen::vanilla_features` compiles that closure and executes the three
  feature bodies (semantics read from the vineflower decompiles staged under
  `/tmp/a1d`), with the vanilla legacy RNG, Java `String.hashCode`, `NormalNoise`
  and the block-state-provider/int-provider/predicate set the closure reaches.
- Glue: `vanilla_features::compile_pool_features(worldgen_dir, pool_id, &BlockSemantics)`.

Evidence: `cargo test -p mc-data --lib vanilla_feature_closure` 12 passed / 0 failed;
`cargo test -p mc-worldgen --lib vanilla_features` 18 passed / 0 failed (both
reproduced by Main); live proof
`SOLARIS_CONTENT_CACHE=/tmp/jdk-cold2 cargo test -p mc-worldgen --lib village_decor_closure_places_from_the_real_cache -- --nocapture`
places `minecraft:village/desert/decor` (11 hay_block, 2 cactus at quoted
coordinates), deterministically on repeat, and fails closed for
`minecraft:village/taiga/decor` on `minecraft:tree`; loud skip when no cache.
Gates: `run fmt` PASS `.analysis/validation/20260914T060152-fmt-de_ds04g`,
`run code-health` PASS `.analysis/validation/20260914T060155-code-health-zsg0znma`,
clippy `-D warnings` clean for `mc-data` + `mc-worldgen`.

Partial by construction: the only non-test call sites are the two module
registrations, `WORLDGEN_REVISION` is still 21, `PlainsVillagePrototype` and its
tests are intact, no profile/default/config/env/flag reaches the new path. Next:
A2 (`minecraft:tree`, the four trunk/foliage placer pairs), then B (village
assembly, activation, prototype deletion, worldgen identity bump).
`Mth.getSeed(BlockPos)` is now read from the decompile
(`seed = x*3129871 ^ z*116129781L ^ y; seed*seed*42317861L + seed*11 >> 16`) but is
deliberately NOT implemented — it belongs to B's per-position rule-processor RNG.

Review (independent, read-only, `ImportProgress`, 2026-09-14): **pass**, no blocking
or high-severity defect. It re-derived the pile and column shapes in Java against the
decompiled bodies and reproduced this crate's expected positions and axis values,
regenerated the pinned RNG constants with `java.util.Random`, ran the real 26.1.2
`NormalNoise` out of the inner server jar and reproduced all 12 noise values, and
confirmed by search that nothing outside the new modules and their tests references
the layer (`WORLDGEN_REVISION` 21, `PlainsVillagePrototype` intact). Its reproduction
of the two focused suites matched (12/0 and 18/0) plus
`cargo test -p mc-worldgen --lib` 185 passed / 0 failed / 5 ignored, and
`run fmt` PASS `.analysis/validation/20260914T061403-fmt-fikmw34z`.
Seven non-blocking findings, folded into A2/B rather than fixed in isolation:
nested `rule_based` providers return `None` where vanilla resolves the existing state
(`provider.rs:222`, only reachable once trees land, so A2 must fix it before enabling
trees); a non-string block-state property is coerced to `""` instead of failing
(`vanilla_feature_closure.rs:762`); `list_pool_element` containing a feature element is
skipped rather than failing closed (`:378`; no village pool uses the list form today);
the live proof asserts liveness and determinism but no vanilla-derived expected
coordinates and covers 3 of 13 reachable features; `parse_offset` omits vanilla's
±16 bound (`:913-940`); missing `weight`/`prioritize_tip` default instead of being
required (`:389`, `:497`); and `random.rs:10-11`'s comment about `DOUBLE_MULTIPLIER`
misdescribes an exact-2^-53 double as a float literal, which invites a future change
that would silently alter every `nextDouble`. Reviewer's unverified list: the 33-entry
vegetation list beyond three checked blocks, `synth.rs` internals, and placement for
the ten features no test exercises.
