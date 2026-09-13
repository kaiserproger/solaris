# Solaris current cursor
## Handover snapshot (batch commit `5ebb33c4`)

- base_tree: `5ebb33c413d2017f0256f67e5445934a817d7da4` (batch commit of the
  validated tree; parent `f77525da6b1f7c6e460d1ae538aca409ce9b6d6b`)
- checkpoint_closed: settlement commit pipeline through the simulation lane, mob spin
  fix, worldgen (biome/river/beach/villages), live operator+whitelist access control,
  tab list, redstone/pistons, pregeneration, warehouse bind/read (C1a) - landed as one
  local batch commit `5ebb33c4` (210 files, +45618/-7730, no push/tag) after the owner
  sanctioned a whole-tree batch because earlier checkpoint lines share hunks with the
  settlement hunks. Local-only paths stayed out: `dist/` (120 MB), the omp session dump,
  `crates/mc-entity/.analysis/bench/entity-battle-1500x1500.json` and four tracked root
  `.analysis/*` deletions.
- validation_at_close: `run correctness` PASS
  `.analysis/validation/20260913T104202-correctness-41olrp8v`; `--lib warehouse` 8,
  `--lib owned_inventory` 18, `-p mc-script` 129, `--test settlement_lifecycle` 7,
  `--test plugin_examples` 4. Independent reviews: `CutoverReview` and
  `WarehouseReadReview` (both verdict changes, all findings fixed).
- validation_after_limiter_fix: `run correctness` PASS
  `.analysis/validation/20260913T151643-correctness-5wnrl9nr` (after moving the drop-refusal
  out of the play gateway, 745 -> 727 lines against the 731 budget; the first attempt failed
  code-health). Focused: `--lib ingress_rate` 8, `--lib settlement` 55,
  `--test settlement_fund` 1 (new repro, fails before the fix).
- pushed: `main` now carries both live-chain fixes (ingress burst limiter no longer drops a
  serialized command and answers a dropped one; `advance_structure` re-observes the footprint
  after its own portion commit instead of pausing as `site_changed`), together with the
  owner's concurrent cleanup sweep over mc-data/mc-test-harness.
- validation_latest_push: `run correctness` PASS
  `.analysis/validation/20260913T231231-correctness-a7ak29y2` (248.7 s) on the pushed tree.
- next: live acceptance of the `site_changed` fix once the machine and the worldgen sweep are
  free - `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=m94-09-settlement-chain python3 -m tools.harness
  run regression --timeout-seconds 600 --run`. Deferred only for the owner's power-saver/noise
  window and because the sweep's terrain turns the driver's deterministic site pick (3203,60,300)
  into 3995/4096 water, which made `.analysis/validation/20260913T164410-regression-go7um90p`
  invalid as acceptance (project refused and withdrawn). If the terrain stays, the driver needs
  a dry-site preference instead of the first candidate. Receipts:
  `.analysis/codex-logs/live-chain/receipt.md`, `.analysis/codex-logs/site-changed/receipt.md`.
- owned_hash: `52a5e94e417ee168aa1f5aa2abf2c6becd4dd706b23f3ee0d335f8861c9e29b5` (SHA-256 over the 13 checkpoint-owned paths, recomputed after the C1a slice and the C1b revert: `crates/mc-net/src/settlement.rs`, `server.rs`, `play.rs`, `play/simulation.rs`, `play/block_wire.rs`, `play/tests/campfire_cooking.rs`, `script/storage/settlement.rs`, `script/storage/settlement_tests.rs`, `script/storage/resident_settlement_tests.rs`, `crates/mc-test-harness/tests/settlement_lifecycle.rs`, `docs/decisions/0004-staged-single-writer-simulation.md`, `docs/PLUGINS.md`, `../solaris-default-plugins/solaris-settlements/main.lua`; recipe: SHA-256 over `"<sha256>  <path>\n"` lines in that order. Other dirty paths belong to earlier checkpoints)
- changed_files: owned batch 18 paths; the rest of the dirty tree (191 paths) is pre-existing workspace WIP, not this batch
- sibling_batch: `../solaris-default-plugins` base `2d51ae5559cd` SHA-256 `71f40021def7e280b2de94fa3a27d206a2eecf6e2e247d36d2f7bec2e774c041` over the 9 files the package agents changed (root `README.md`/contract/`WATCHDOG.yml` and untouched package files excluded — pre-existing WIP; the package's two older receipts under `server/evidence/` are pre-existing as well)
- classification: `.analysis/handover/2026-09-13-batch-files.txt` (ignored artifact)
- checkpoint_site_vertical: settlements are grounded on the world's own terrain.
  `ChunkGenerator::surface_height` (defaulted `None`) with a `TerrainGenerator`
  override; `SettlementSelector::layout`/`::road` take a
  `&dyn Fn(i32, i32) -> Option<i32>` ground resolver and anchor each building so
  its authored anchor meets the terrain row at the anchor column (flush rule);
  `SettlementRuntime` holds the `WorldStorage` generator (fail-closed when a
  deployment has none); `site_snapshot` reports the grounded candidate;
  `spawn_resident` verifies `ResidentWorld::standable` before materialising.
  Evidence: `correctness` PASS `.analysis/validation/20260913T052447-correctness-_wlwlfgp`;
  receipt `.analysis/codex-logs/site-vertical/receipt.md`; live real-client
  `/settlement site` reports `origin 839,62,296` and `origin 1788,87,58` where the
  previous receipt recorded `origin 1788,0,58`, with
  `adopt`/`survey`/`project`/`fund`/`build`/`info houses=1` completing
  (`.analysis/validation/20260913T053315-regression-vbm3xulo`).
- checkpoint_site_vertical_open: both review findings are closed. (1) The first
  grounding rule (`+1`) was wrong: the shipped data anchors `plaza_well` at
  `[7, 0, 7]` with its layer at local y=0 and `house_small` at `[5, 0, 8]` with
  rows only at y=1..6, so `+1` floated the plaza and put the house floor at
  `surface+2` instead of the accepted `surface+1`. The rule is now
  `origin_y = surface - anchor_local_y`; live re-check confirms it
  (`origin 1788,86,58`, `origin 839,61,296`, no y=0). (2) A deployed catalog
  missing a required role now fails startup (`missing_required_role` +
  `SettlementStartupError::MissingRole`), covered by
  `catalog_violations_fail_loudly_with_a_typed_error`. The single independent
  reviewer re-read the settled tree and returned `overall_correctness: correct`
  with no new findings; the site-origin row is the origin column's first free
  row, `road` kept its original signature (no production caller), and a focused
  `anchor = [1,1,0]` deck test covers the anchor-above-base case under rotation.
- checkpoint_commit_publication: fixed behaviourally, cause not proven. Before the
  change, a player-visible `/settlement build` reported `house_sm_1 committed
  (solaris:house_small)` and `houses=1` while a client scan of the footprint found
  only terrain (QA runs 4/5). After it, run 6 on a fresh world found the whole
  house in the client at `origin 1788,94,58` (oak_planks 628, oak_log 32, glass 20,
  red_bed 20, torch 4, oak_door 4, crafting_table 2, chest 2; dirt 92 → snow 93 →
  air 94 → planks 95 → log 96-99) with the same `committed` answer. What changed in
  code: `play::block_wire::broadcast_applied_edits` publishes a writer-less batch
  (`invalidate_prepared_chunks`, delta broadcast, incremental light + chunk
  invalidation) and evicts session cooking state for an applied campfire →
  non-campfire edit, instead of returning after the storage commit. That eviction
  came from the independent read-only review (verdict `incorrect`, one P2 finding
  with the concrete failure path: the tick loop skips only positions whose current
  block is not a campfire, so a stale entry could be inherited by a later campfire
  and materialise its pending outputs); the review confirmed the rest of the fanout
  as correct, and its two minor notes (call `lighting::light_update_chunks`, revert
  the `invalidate_prepared_chunks` widening) are applied. Two explanations for the
  original invisibility remain open in attribution only: run 6's own saved world
  was decoded read-only and contains the committed house (oak_planks 314, oak_log
  16, glass 10, red_bed 10, torch 2, oak_door 2, crafting_table 1, chest 1 — the
  authored `house_small` totals, `.analysis/real-client-runs/settlement-ground-qa/run6-persistence.json`),
  so that run's placement and persistence are not in question; what is not run is
  an A/B of the pre-change build on one world, which is the only way to attribute
  the earlier invisibility to the missing publication rather than to the grounding
  change from the same window. A live reload of that world was not possible for the
  QA agent (entity-owner journal refuses recovery past the supported
  30,000-decision boundary; setting it aside trips the deliberate
  `world metadata identity mismatch` for the copied path). The reviewed campfire
  gap has a regression:
  `play/tests/campfire_cooking.rs::a_writer_less_commit_evicts_cooking_for_a_replaced_campfire`,
  which fails with the eviction disabled and passes with it.
  `.analysis/codex-logs/placement-visibility/receipt.md`; scan
  `.analysis/real-client-runs/settlement-ground-qa/run6-watchdog-scan.json`.
  Still open from the same run: `/settlement populate` refused with `blocked`
  (residents not materialised).
- checkpoint_commit_publication_residual: run 6 proves in-session client
  visibility and (by read-only region decode) disk persistence. It does not prove
  what a restarted server or a reloaded client serves: the QA's reload attempt
  failed before serving (`regional decision recovery exceeds the supported
  30,000-decision boundary`, then the deliberate `world metadata identity mismatch`
  on the copied path). One scan against run 6's own world directory after a real
  restart is the closure for that, tracked as a todo; do not infer reload
  behaviour from the decode.
- checkpoint_settlement_commit_pipeline: DONE (2026-09-13). A structure portion now
  commits as exactly one awaited `SimulationCommand::ApplyBlockEdits
  { actor_session: None, .. }` on the server-owned `SimulationHandle` bound into
  `LiveSettlementWorld` (`crates/mc-net/src/settlement.rs`), created by `server.rs`
  before the settlement deployment; the direct `WorldStorage` write, the
  `broadcast_applied_edits` fanout and the `OnceLock` late binding are gone. Every
  actor-`None` batch stays off the session fast lanes (`command_can_use_resident_
  mutation`, with `command_can_use_regional_mutation` delegating for that command),
  so the staged path owns cooking eviction, reactivity, owner relighting and
  post-commit publication; ADR 0004 records the invariant. Evidence:
  `.analysis/decomposition/evidence/{P01,P02,P03,P04,P04b,P05}/receipt.json`.
  Validation: `cargo test -p mc-net --lib` 2172 passed; `server_owned_block_edits`
  neighbourhood 5 passed (eviction for cross-region and single-region batches,
  fenced-handle refusal, lane-policy predicate, `BlockDeltas` seen by a loaded
  session); `cargo test -p mc-test-harness --test settlement_lifecycle` 6 passed;
  `plugin_examples` 4 passed; `code-health` PASS
  `.analysis/validation/20260913T085900-code-health-2culc043`; `fmt` PASS
  `.analysis/validation/20260913T085903-fmt-vpq5t0nt`.
  Closed in the same checkpoint: `release_resident_site` refuses a consumed
  reservation (`.analysis/decomposition/evidence/P06`); the plugin releases a
  stranded resident-site reservation only for `blocked`/`unloaded` - the two
  refusals that provably precede any effect in `residents.rs` - and keeps the
  reservation for `runtime_unavailable`/`invalid_request`/`not_found`/`capacity`/
  `busy` (`P08`, plus the missing `continue_write` release-intent branch and the
  release-completion hydration fix, `P10_P11`); a refused `project` is withdrawn by
  a confirmed durable batch and its pending intent survives an unconfirmed cleanup
  (`P10`); batch ids no longer alias names differing only by `-`/`_` (`P11`); the
  native m94-09 loop anchors each stage on chat observed after the submission and
  fails closed on absent feedback (`P13`).
  Residual gaps: batch ids built from two 40-char components can still exceed the
  core's 64-byte `MAX_SCRIPT_ID_BYTES`; live-proof driver steps below are missing.
  (The earlier note about writer-session campfire eviction on an accelerated lane
  was wrong and is withdrawn: the writer path clears replaced campfire cooking in
  `finalize_visible_block_edit_outcome`, `play/block_edit_commit.rs:315-338`.)

- release_installed: `~/.local/bin/solaris` md5 `006e7195848f901918118890709b3d3a`
  (`mc-server 0.0.6`, built from the current tree after the settlement commit
  pipeline cutover and the plugin lifecycle fixes), and
  `$HOME/sarvar/plugins/solaris-settlements/main.lua` is byte-identical to the
  sibling package source; the other five deployed packages are untouched. Plugin
  discovery must be checked from `~/sarvar`, because `[plugins].directory =
  "plugins"` is relative: `cd ~/sarvar && solaris --check --config server.toml`
  exits 0 with `operator_warnings: []` and discovers all six packages (the same
  check from the repository root resolves no plugin directory and reports
  `discovered_plugins: []`).
- validation: `run correctness` PASS `.analysis/validation/20260913T071815-correctness-nt60vl9s`
  (fmt, `code-health`, strict workspace Clippy, workspace tests, including the new
  campfire regression) and `.analysis/validation/20260913T070609-correctness-skxjk7sx`
  on the same revision the installed binary came from (the later addition is
  `#[cfg(test)]` only, so `~/.local/bin/solaris` md5 `c6dc5444d9bc91c058c84877c0521060`
  is unchanged); live real-client QA run 6 verified the committed house in the
  client (`.analysis/real-client-runs/settlement-ground-qa/run6-*`) and its saved
  world was decoded read-only as persisted; a smoke start of the installed release
  bound `127.0.0.1:25565` with `plugins=6` and was stopped again, and
  `solaris --check --config ~/sarvar/server.toml` exits 0 with six discovered
  packages, so the port is free for the owner's test.
- validation_latest: `run correctness` PASS
  `.analysis/validation/20260913T104202-correctness-41olrp8v` on the current tree
  (after the C1a review fixes) and
  `.analysis/validation/20260913T103025-correctness-ry310o95` after the C1b revert;
  earlier: `.analysis/validation/20260913T090714-correctness-jrwjk54y` (settlement
  commit pipeline). Focused: `cargo test -p mc-net --lib warehouse` 8 passed,
  `--lib owned_inventory` 18 passed, `cargo test -p mc-script` 129 passed,
  `cargo test -p mc-test-harness --test settlement_lifecycle` 7 passed,
  `--test plugin_examples` 4 passed; `code-health` and `fmt` PASS on the same tree.
  Independent reviews at this checkpoint: `CutoverReview` (settlement commit
  pipeline, verdict changes -> all fixed) and `WarehouseReadReview` (C1a bind/read,
  verdict changes -> receipt-per-bind + doc corrections fixed).
- visibility_claim (behavioral only, A/B not run by decision): committed settlement
  houses are client-visible and persist to the region file on the current build
  (run 6). No causal attribution is made between the missing publication fanout and
  the structure grounding change from the same window; see
  `.analysis/decomposition/evidence/P22/receipt.json`.
- c1_writable_warehouse (in progress, split): the documented contract is an
  opaque handle for a verified loaded container, never coordinates
  (`docs/PLUGINS.md:1577-1621`); canonical storage stays `Chunk.chests` keyed by
  `BlockPos`, so no second item ledger may appear. Verified today: three refusal
  gates (`play/session/owned_inventory_endpoint.rs:41-43` read, `:93-97` transfer,
  `:398-402` reservation), routing at `script/storage/world_inventory.rs:530-545,609-630,655-672`,
  the prepare boundary at `play/owned_inventory.rs:25-65`, and the issuance
  precedent at `script/storage/residents.rs:875-949`; `commit_prepared` still
  passes `Vec::new()` chunk snapshots (`world_inventory.rs:466-470`), so container
  after-images are not yet journaled. The authored warehouse blueprint exists
  (3 chests + 3 barrels + six `empty_container` entities) and the plugin's
  `/settlement deposit` is an unconditional refusal (`main.lua:2344-2355`).
  Handle invariant: core mints the handle; the plugin never supplies positions. The
  warehouse blueprint's `stores` POI is kind `work`, not a container, so no POI
  receipt can address a chest; the plugin names its durable `structure_id`
  plus the authored container ordinal and core verifies it against the
  blueprint catalog and the placed, loaded block. A plugin-chosen coordinate
  or a second container-address authority stays forbidden.
  Slice 1 DONE (2026-09-13, `.analysis/decomposition/evidence/C1a/receipt.json`): the
  DTO/Lua `bind_warehouse` + `ScriptWarehouseBinding {handle,structure_id,container_id,revision}`,
  a durable `DurableSettlementChange::Warehouse` binding, verification (ownership,
  placed `is_placed() == state != Cancelled`, authored `empty_container` ordinal,
  loaded chunk, container present) and a real READ path resolving a handle to the
  canonical `Chunk.chests` snapshot; `is_active()` is deliberately not used (it
  excludes completed structures, and a warehouse is a completed container).
  Evidence: `cargo test -p mc-net --lib warehouse` 8 passed, `-p mc-script` 129 passed,
  L2 `correctness` PASS `.analysis/validation/20260913T094337-correctness-gstdsd2t`.
  Slice 2 ATTEMPTED AND REVERTED (`.analysis/decomposition/evidence/C1b/receipt.json`,
  status reverted): the write path was built on a raw `WorldMutationView` container
  mutation, bypassing the typed container transaction, so it neither advanced
  `chest_state_ids` nor published `ChestSlots` to viewers and a concurrent menu
  commit could plan from pre-transfer state (ADR 0004 violation, a second
  authority). Nothing of it remains; the transfer endpoint refuses again and
  `commit_prepared` is back to `Vec::new()`.
  Required design for the next checkpoint: ONE server-owned simulation command
  carrying the operation receipt, the player after-image, the container with its
  expected/updated slots and the expected fences, executed entirely inside the
  owner turn - validate fences and current chest state, stage via the chest
  transaction boundary, append the ONE world-journal decision, project both sides,
  advance `chest_state_ids`, publish `ChestSlots`, then respond - so no container
  transaction can interleave. That is a cross-domain ownership migration whose
  recovery/data lifetime must be designed, not a small container fix; it is not to
  be improvised.
  Slice 3 (plugin deposit/withdraw) is BLOCKED on three owner decisions, because the
  package defines none of them: (a) which items deposit, (b) whether quantity is
  user-visible, (c) whether a withdraw surface exists at all. `/settlement deposit`
  stays an honest refusal (`main.lua:2344-2355`); no plugin-side money balance may
  be invented and `record.money` is only a carried-inventory projection.
  Slice 4: restart round-trip evidence, after slice 2. Deferred: multi-container /
  double chest and C4 haul (`resident_order_execution.rs:646-650`) /
  demobilization (`:1063-1065`).
  with container after-images in one recoverable decision. Slice 3: plugin
  deposit/withdraw wiring. Slice 4: restart round-trip evidence. Deferred:
  multi-container/double chest and C4 haul/demobilization.
- next: the native chain is runnable
  (`SOLARIS_REAL_CLIENT_AGENT_SCENARIO=m94-09-settlement-chain python3 -m tools.harness
  run regression --timeout-seconds 600 --run`) but there is no settlement sweep
  profile and the refused-spawn retry, second-create, warehouse round-trip and
  dismiss proofs need driver steps that do not exist yet
  (`.analysis/decomposition/evidence/D5/receipt.json`); after that, the C1 writable
  warehouse endpoint (no handle issuer or ownership binding exists today:
  `.analysis/decomposition/evidence/D4/receipt.json`), then the owner's commit
  decision for the owned batch.

Top changed groups: `crates/mc-net` (70), `crates/mc-test-harness` (57), `crates/mc-script` (20), `crates/mc-worldgen` (13), `crates/mc-server` (10), `examples/loader-live-gate` (10), `crates/mc-entity` (7), `crates/mc-protocol` (2)

## Queued after handover (dependencies, not motion)

Blocked on the owner's field test:
- **Owner manual structure-fit test**: the shipped gate's accepted-anchor path on the release
  binary (`~/.local/bin/solaris`, md5 `d066538256ec088b760ae11f027a9f64`); its live evidence so
  far is the terminal refusal on the previous, looser revision plus the unit test.

Blocked on the site-vertical checkpoint (deterministic `TerrainGenerator::surface_height`
plumbed into `SettlementRuntime`; the live-occupancy attempt was cancelled, reverted and
recorded):
- Settlement residents standing on the ground, the live `hire` + `squad order attack` combat
  proof the package still owes, and the canonical `m94-09-settlement-chain` run (its driver
  also needs paced commands, and the run root must stay short until the world-identity fix is
  in a build the harness uses). `/settlement populate` is refused by the core with `blocked`;
  the cause is diagnosed from run 6's plugin journal below.
- QA finding, real-client run 6: the m94-09 runner stalls when the server's
  command ingress bucket drops a `/give` (`COMMAND_BURST=8`, `class="command"`
  drop in `logs/debug.log`). Its second half is fixed: `await_state` in
  `tools/harness/backends/driver.py` clamped its event timeout to the client's
  accepted 0.1 s floor, so a deadline-driven wait no longer surfaces as
  `IllegalArgumentException: timeout_seconds must be between 0.1 and 120.0`
  instead of a clean timeout. Still open: the dropped `/give` itself — the runner
  should wait for the give to be acknowledged (inventory state or the log drop
  marker) rather than firing the next command, and that pacing decision is not
  made yet.
- The package's ghost `projected` entry after a refused `project`: `S.refuse` does not clear
  the prepare intent/index/record.
- **Why `/settlement populate` is refused with `blocked` (diagnosed from run 6's own plugin
  journal, `.analysis/validation/20260913T063935-regression-958j57r4/regression/20260913T063936Z-m94-regression-pack-w2tEzP/world/solaris/plugin-storage-v1/journal-v1.bin`):**
  the journal holds three resident-site ops — `reserve-regsville-25` (`reserve_poi`,
  `site_3_0_31f075c1.0.home`), then `spawn-regsville-27` (`spawn`, token `1a0807e0…`), then
  `reserve-regsville-29` for the *same* home. So the first reserve committed, the spawn failed
  (`runtime_unavailable`), and nothing ever handed the reservation back; the core refuses a
  second reserve of a live reservation (`script/storage/settlement.rs:1163`), which is the
  observed `Core refused the request: blocked.`. Required fix, in this order: resolve the spawn
  op through the plugin's existing durable intent/receipt lifecycle (`operation_status`), and
  release with `solaris.release_resident_site(request_id, op_id, spawn_site_token)` (core
  handler `release_resident_site`, release DTO `ScriptSettlementOperation::ReleaseResidentSite`)
  **only** on a confirmed non-commit. A fire-and-forget release from the generic refusal branch
  is wrong: an unconfirmed failure can race a committed spawn, and the core marks `released`
  without refusing a consumed reservation, so it would free an occupied home. A core-side guard
  refusing a release whose reservation is already `consumed` is a proposed hardening and belongs
  to the core file the current checkpoint owns. Any release intent must also be its own durable
  transition: `set_pending` overwrites the same `resident-site` slot, so an intent whose bundle
  write fails followed by `clear_pending` would drop the only recovery handle while the core
  reservation stays live — model the cleanup exactly like the package's other resident-site
  intents (including failed-write and restart recovery) and cover it with the package's
  intent-lifecycle tests before deploying it.

Open design decisions, no code yet:
- The base-row rule for the eight blueprints that author cells at their local `y=0`
  (`farm`, `market`, `plaza_well`, `mine_entrance`, `pen`, `palisade_gate`, `stone_wall`,
  `fishing_pier`): re-author their base rows hollow, or gate base-row blueprints strictly
  above the terrain. `mine_entrance` cannot be placed into a hillside under the current rule.
- POI leash for villagers and golems: the global 6..32 wander reach moves idle villagers up to
  32 blocks from home. Needs a measured policy, not a guess.
- A writable `warehouse` inventory endpoint (core C1) and the remaining C4 coverage gaps.

## Handover state (owner manual test)

L2 `python3 -m tools.harness run correctness` PASS on the final tree
(`.analysis/validation/20260913T034637-correctness-ualx2zif`, 333.8s) after one independent
read-only review (`FinalReview`, 9m23s) whose verdict was **`changes`, `overall_correctness:
incorrect`** — four findings, not a pass. Three were claim/evidence defects and are fixed:
the structure-fit and wander receipts now state the shipped boundary (`max_opaque_y > anchor[1]`),
record that the live acceptance run predates that correction and is owner-manual pending, and
drop the imaginary `/fill` platform (the server has no `fill` command, so the wander A/B ran on
natural terrain at 0,0). The review also established that villagers and golems share
`GoalState::Wander`, so the global 6..32 reach moves idle villagers up to 32 blocks from home;
a measured POI-leash policy is a separate queued checkpoint, not folded into this batch.
Its fourth finding stays **open, not fixed**: the eight shipped blueprints that author cells at
their local `y=0` (`farm`, `market`, `plaza_well`, `mine_entrance`, `pen`, `palisade_gate`,
`stone_wall`, `fishing_pier`) do replace the terrain's top ground row when anchored flush with
it, so "a structure is never built into terrain" is literally true only for the other fourteen.
Deciding that rule (re-author their base rows hollow, or gate base-row blueprints to strictly
above the terrain) is its own checkpoint.

One harness test needed a determinism fix, not a product change:
`survival_tnt_explosion_damages_mob_over_wire` raced the new wander — a summoned chicken
walked out of the four-block blast radius while the fuse burned. A/B in
`.analysis/codex-logs/tnt-mob-wander/repro.log`: with reach 3..4 the test passes, with 6..26 it
fails. Fix: the test pins `minecraft:chicken` to `MobMovementPolicy::Immobile` through
`bound.entity_behavior_handle().configure_mob_behavior_table(...)` before serve, which removes
the incidental wander premise without touching explosion geometry.

Delivered for the owner's field test: release binary installed at `~/.local/bin/solaris`
(built from this exact tree) and the six packages deployed to `$HOME/sarvar/plugins`
(`solaris-permissions`, `solaris-essentials`, `solaris-economy`, `solaris-towns`,
`solaris-audit`, `solaris-settlements`), which the owner's existing config already points at
through its relative `[plugins] directory = "plugins"`.

## Buildings buried in terrain: diagnosed, guarded, and the real fix dispatched

Owner report (screenshot + "по полу невозможно ходить, я застреваю в нём"). Evidence
from the live probe world: the committed `solaris:house_small` at probetown has its floor
planks at world y=73, windows at y=76 and roof at y=78 while the surrounding terrain is
y≈79-80, and **the interior columns are solid stone**. The player stands inside terrain.

Causal chain, from code plus world scan:
1. `mc_worldgen::SiteCandidate::origin` always carries `y = 0`; its own doc comment says
   "the y coordinate is resolved by the caller" and nothing resolves it, so
   `/settlement site` reports `origin 1788,0,58` and the plugin's `S.site_anchor`
   fallback (`y = site.min_y`) points at y=0.
2. The structure anchor therefore came from the ordering player's position (the plugin's
   `project ... here`), which stood on a slope, so the house was built inside the hill.
3. `prepare_structure` validated bounds, claims and the survey token but never checked
   that the reserved volume was free, while contract A03 requires invalid sites to be
   refused or re-checked **without overwriting** terrain.

Fixed by Main in core: `SettlementWorld::max_opaque_y(bounds)` (bounded per-column read,
honestly named — it is an occupancy ceiling, not a terrain surface) plus the fit gate in
`prepare_structure` that answers `Blocked` when `max_opaque_y > anchor[1]`: nothing may sit
above the base row, and terrain level with it is the ground the structure stands on. The
first revision of that gate allowed the row above as well and was corrected, because a floor
authored at local `y=1` would then overwrite the terrain top. No
terrain is ever cleared: an air/clearance write path was designed and rejected because it
contradicts A03 and would need new blueprint semantics for water-bearing structures such
as `fishing_pier` (which authors `minecraft:water` at local y=0 with its deck at y=1).
Test `prepare_refuses_a_footprint_the_terrain_rises_into` covers the refusal, the
flush-fit boundary and the unloaded footprint; `cargo test -p mc-net --lib` 2162 passed,
clippy `-D warnings` clean, `run fmt` and `run code-health` PASS. Live proof: an
obstructed anchor answered `Core refused the request: blocked.` in the real client.

Independent confirmation, from agent `P1SquadHandle`: a resident entity sat at
(1793, 2, 64) inside solid stone with terrain at y≈92 there, same cause (site POIs use the
unresolved y=0 origin).

Live proof on a **fresh** world (`.analysis/live-probe/gate_proof.py`, own `world_dir`,
frozen binary `/tmp/mc-server-gate`): an anchor whose footprint contains terrain answers
`Core refused the request: blocked.`, and a house committed and stood on the ground with its
floor planks one row above the terrain top, glass walls and a 99-plank roof
(`house_sm_1 committed (solaris:house_small) at revision 20`, raw column scan in
`.analysis/codex-logs/structure-fit/floorscan.json`). That run predates the boundary
correction — it anchored at `terrain_top - 1`, which the old `anchor + 1` rule accepted and
today's rule refuses — so for the shipped revision the live *refusal* and
`prepare_refuses_a_footprint_the_terrain_rises_into` are the matching evidence, and the
accepted-anchor path is **owner-manual pending** (blocked gate, never green): the shipped
revision's live evidence is the refusal plus the unit test, and the accepted-anchor gameplay
check is the owner's own field test. Details, hashes and the tooling limits (stale
client block reads after a build; `minecraft_press_inputs` not moving the player in this
setup) are in `.analysis/codex-logs/structure-fit/receipt.md`.

The site/POI vertical placement is still unresolved and is the next checkpoint. An agent
grounded it from **live occupancy**; that is rejected and being reverted (`Unground`),
because the canonical layout must stay a pure function of `(seed, revision, cell)` — the
live read made `list`/`query` depend on loaded chunks and mutable blocks, so a distant
site would answer `Unloaded` and the same settlement could report different coordinates.
The deterministic design to implement next is
`mc_worldgen::TerrainGenerator::surface_height(x, z)` (public, pure, "the same function
the generator does") plumbed into `SettlementRuntime`, with the fit gate above still
checking real world contents; the rule itself comes from the blueprint's own data (every
shipped blueprint's `[footprint].anchor` equals its first `[[street_connection]].at`, and
`fishing_pier` authors water at local y=0 with its deck at y=1).

## Long-path world identity fixed (owner-relevant)

Agent `WorldIdentity` (21m38s, pass) removed the 128-byte cap on the world identity input
in `mc-script` (`resident_generation_id`): a deep server directory previously made
`/settlement site` answer `Core refused the request: invalid_request.`. The returned id was
already fixed-width 64-hex, so ids for paths ≤128 bytes are byte-identical (persisted CAS
generation ids stay valid). Live proof at a 179-byte world path: `create` and `site`
accepted. `cargo test -p mc-script` 128 passed, clippy clean, `run code-health` PASS.

## Long-range mob wander (owner: "чтобы мир реально был живым и в движении")

Wander targets were rolled 3..7 blocks from the agent's current position
(`WANDER_MIN_DISTANCE 3.0` + `WANDER_DISTANCE_SPREAD 4.0`, `crates/mc-entity/src/lib.rs`),
so the world read as static. They are now rolled 6..32 blocks. Because a target is
rolled relative to the *current* position there is no home leash, so the wider reach
becomes real roaming rather than a wider idle. Cost stays flat per tick: pathing is a
greedy per-tick step under `PathingBudget`, so a longer walk costs ticks, not work, and
an unreachable target is abandoned by the retained-path no-progress budget and re-rolled
(aquatic agents discard the blocked target and re-roll the same way). Hostile mobs share
the goal, so they roam too; villagers do not use it.

Measured A/B on a clean 129x129 stone platform at y=119 with ten sheep summoned on a
tight ring, sampled every 3 s for 60 s through the real MCP client, same world snapshot
and therefore the same entity ids and the same deterministic angle sequence for both
builds (before = `/tmp/mc-server-prewander`, mtime 09:23:48, the last build preceding the
edit; after = `target/debug/mc-server`, mtime 09:40:51):

| metric (60 s, 10 sheep) | before (3..7) | after (6..32) |
| --- | --- | --- |
| median net displacement | 15.98 | 33.40 |
| max net displacement | 27.12 | 57.71 |
| median travel | 79.85 | 106.39 |
| smallest net displacement | 6.00 | 20.90 |

Tests: `wander_targets_are_multiblock_and_not_synchronized` now samples the real roll
path over 64 entity ids and fails if the reach drops back to a stroll;
`wander_pauses_after_reaching_its_retained_target` derives its tick budget from the reach
instead of a magic 80. `cargo test -p mc-entity --lib` 625 passed,
`cargo test -p mc-net --lib mob_spin` 3 passed, `run fmt` PASS, clippy `-D warnings` clean.
Raw method, binaries and totals: `.analysis/codex-logs/wander-range/receipt.md`.

## Mob spin near leaves fixed and measured (owner bug)

Agent `MobSpinFix` (52m23s) found the real cause, which is **not** leaf passability:
leaves were already solid obstacles (oak_leaves state 279, probe `Blocked` inside),
and no leaf-id special-casing was added. The wander pathfinder accepted a detour
that moved the mob *away* from an unsatisfiable target, then walked it back; the
no-progress guard only watched position deltas, so the ~0.9-block oscillation reset
it, and `face_horizontal_motion` chased the flipping velocity — endless rotation
(`crates/mc-entity/src/lib.rs`, `bounded_pathing_step`). Fixes: a detour is accepted
only when it strictly reduces target distance or the body already overlaps terrain,
otherwise `Blocked` (zero velocity, no rotation) and the existing cadence/backoff
retargets; an overlapping agent gets a bounded cardinal escape (feet level and one
block down) so one spawned *inside* a canopy walks out; the terrain probe now
declares the entity position and escape probes.

Measured live on its own server/ports with the real MCP client: pre-fix 4 sheep
matched the spin (net movement < 1 m, yaw > 1700°/5 s); post-fix 0 matched, and
3 spiders summoned inside the canopy walked out 11.2–26.7 m. Tests:
`cargo test -p mc-entity` 625 passed, `cargo test -p mc-net --lib` 2162 passed with
3 new `mob_spin` tests (one fails pre-fix), fmt/clippy/check clean. Known
limitation, honest and recorded: an agent fully enclosed by a ≥2-block leaf pocket
or embedded in a 1x1 trunk log can stay stationary when no cardinal neighbour is
walkable — it no longer rotates, but it also cannot escape a sealed pocket.

## Plugin ↔ C4 wiring landed; live combat blocked by one plugin bug

Agent `P1C4Wiring` (43m5s) wired the shipped package to the C4 APIs and deleted
every "needs core C4" placeholder: `assign/cancel_resident_work`,
`issue/cancel_resident_order`, `demobilize_resident`, and
`transfer_owned_items` with the `resident_equipment`/`resident_carry` endpoints
(`main.lua` 4224→5468, manifest gained `resident_work`/`resident_orders`). It also
fixed a real plugin bug (`squad <name> list` was unreachable) and reported one it
did not fix: `create`'s durable operation id is not per-settlement, so founding a
second settlement name returns `operation_conflict`.

Main proved two thirds of it live on the running server (creative, then the dry
site coordinate 1789/74/59 used by the earlier chain):
- `hire` reads the employer's real player inventory and refuses with the exact
  missing item — `Cannot equip 29ae7f40 as militia; missing from your inventory:
  minecraft:leather_chestplate. Nothing was equipped.` — then after giving the kit:
  `29ae7f40 serves as militia (core equipment: iron_sword,leather_chestplate).`
  with `residents` reporting `service=military role=militia squad=alpha
  gear=iron_sword,leather_chestplate`, i.e. C1's `resident_equipment` endpoint
  really committed the gear.
- `/summon minecraft:zombie` works and the squad record stores `order=hold`.

**Remaining plugin defect found live**: `squad <name> order <squad> hold` answers
`Squad alpha has no member with a core handle.` even though `residents` shows the
same member with its handle and `squad list` reports `members=1 armed=1`, and
`squad <name> add <squad> <handle>` prints nothing at all. So the squad record does
not retain/resolve the resident's core handle, which blocks the order path (and
therefore the live combat proof). Fix is plugin-side and small; it must be followed
by the live proof: armed militia + summoned hostile → observed committed damage,
then `dismiss` returning the gear.

## Live re-verification on the gate-green build + the last gameplay gap

After `correctness` passed, Main re-ran the live chain against the rebuilt binary
(server + real MCP client, same six-package set and the same world, so persistence
was re-proven too): `/settlement info probetown` still reports `houses=1` from the
house built before the gate, and `/settlement populate probetown` now spawns a real
resident through C3 — `29ae7f40 settled in probetown (alive_loaded), home
site_3_0_31f075c1.0.home. House capacity is tracked by that home POI.` —
`/settlement residents` shows `29ae7f40 family=unassigned job=- service=civilian
squad=- life=alive_loaded`, `info` moves to `pop=1 houses=1`, and
`grep -c "wall-clock budget exceeded\|plugin disabled"` = 0.

**Last gameplay gap found by that probe**: the shipped plugin still answers
`29ae7f40 serves as militia; equipment and orders need core C4.` and `Squad alpha
order hold recorded; physical execution needs core C4 (issue_resident_order).`
because it was written before C4's Lua surface existed and (correctly, per its
brief) recorded the missing call instead of faking it. Core C4 is landed and
gate-green, so the gap is purely plugin-side: agent `P1C4Wiring` is wiring
`hire`/gear (`transfer_owned_items` with `resident_equipment`/`resident_carry`),
`squad order` (`issue_resident_order`/`cancel_resident_order`), `job`
(`assign_resident_work`) and `dismiss` (`demobilize_resident`), and must prove it
live by summoning a hostile next to an armed militia member and showing committed
combat damage with no ally hit.

## Full L2 `correctness` gate PASSES on the whole settlement program

`python3 -m tools.harness run correctness` → **status passed**, artifact
`.analysis/validation/20260913T005844-correctness-8o6u6sbt` (supervised via
`hub start name=correctness`; the earlier attempt died when its foreground job was
lost). That is fmt + `code-health` + workspace clippy `-D warnings` +
`cargo test --workspace --all-targets` green on the tree that now carries C1–C4,
the settlement runtime, the loader protocol-3 cutover and the plugin-set changes.

Two integration defects were found by the gate itself and fixed by Main before it
went green:
1. `code-health`: the new public plugin DTO
   `ScriptClientViewFieldValue` (crates/mc-script/src/client_view.rs) lacked
   `#[non_exhaustive]`; adding it exposed two exhaustive matches in
   `crates/mc-net/src/play/session/loader_views.rs`, which now have explicit
   catch-alls (a substituted-field refusal and a `FieldKind::Unknown` that cannot
   match a declared model field) instead of being silently widened.
2. `cargo test --workspace`: `crates/mc-server/tests/cli.rs`
   `check_reports_derived_deployment_for_every_plugin` still built its fixture
   plugin with `[client] schema = 1`, which the schema-2 cutover now rejects; the
   fixture is schema 2 (`content = ["assets"]`, `permissions = ["load_assets"]`
   remain valid pairs). 42 cli tests green after the fix.

## C4 combat proven — five real executor bugs fixed

Agent `A10Combat` (43m33s) removed both `#[ignore]`s after finding the recorded
reason was a **misdiagnosis**: `resident_perception` does see a spawned zombie; the
executor was wrong in five places, each a gameplay bug, not a test artifact:
1. proximity orders dropped their engagement radius (`let _ = engagement_radius`),
   so Hold/patrol never perceived anything;
2. no order ever issued a *fresh* target ref (only TTL-refreshed resolved ones), so
   the Attack op was unreachable — proximity orders now perceive with their own
   radius, fill targets and mint server-issued refs, with attack refs deduped;
3. ranged detection used `weapon.ends_with("_bow")`, which is never true for
   `minecraft:bow`, so the ammunition gate was dead and every "archer" meleed;
4. a dead guard (`references.get(&uuid_of(record))` — the attacker is not in the
   target-ref map) aborted every attack;
5. the ally set was matched against the member's *handle* instead of resolved
   resident handles, so allies were issued and could be hit.

Tests: `cargo test -p mc-net --lib --features load-bench resident_order` → 11
passed, 0 ignored; full mc-net lib → 2152 passed, 8 ignored; fmt/clippy clean;
`cargo check -p mc-server` clean. Six mutations each fail the named assertion
(ammo, LOS, ally-hit, ally-issued, retreat, patrol-resume), so the tests defend
behaviour rather than plumbing.

Residual to prove live (I1 item, not a code gap on this evidence): production
residents are not tracked through the session-local fixture path the tests use, so
their perceivability must be confirmed on a running server (spawn/claim a resident,
put a hostile nearby, order an attack, observe committed damage). The per-tick
simulation-input publication path is the expected tracker; that assumption is not
yet verified outside unit tests.

## Loader wire cutover closed (protocol 3 / schema 2 both sides)

Agent `L1Core` (33m9s, verdict `pass`) landed the core half: bundle schema 2 with
no schema-1 decoder, Loader protocol 3, the full wire-3 view lifecycle
(`crates/mc-net/src/play/session/loader_views.rs`, `script_client_view_endpoint.rs`,
`crates/mc-script/src/client_view.rs`), client ingress `view_action` /
`cancel_selection` admitted through ledger-owned permission pairs and re-read per
action, single-use tick-expiring selection contexts invalidated by
replacement/close/disconnect/revocation, and a clean cutover that deleted
`ScriptClientUi`, `present_client_ui`, `loader_interaction` and their endpoint
files/tests. The shipped `examples/loader-live-gate` fixtures were migrated and
rebuilt. `docs/PLUGINS.md` now states that no shipped package declares `[client]`.

Main verified the cross-repo signal directly, not just through the profile: the
harness `java` profile reports `loader-core:test` as UP-TO-DATE (the fixture test
reads a system property), so I forced
`./gradlew --offline --no-configuration-cache --rerun-tasks :loader-core:test
--tests '*LoaderLiveGateFixtureTest*'` → BUILD SUCCESSFUL with
`LoaderLiveGateFixtureTest tests=2 failures=0 errors=0 skipped=0` (written 02:07).
Harness receipt: `20260912T190619-java-1p3l15pg`. Also green: mc-script 262
passed, mc-net `loader_view` 6, `script_client_view` 3, `cargo check -p mc-server
--all-targets`, fmt and clippy clean.

Two intentional deferrals, now written into the frozen wire doc as the shipped
contract rather than left as open gaps: (1) the marker payload on the wire is
`{ marker_id, selection_token, action_id, formation, radius }` — the earlier
`selection_context_id`/`preview_id` naming was never implemented, and the
projection binding (`world_preview_ref`) plus the V/R `view_request` message stay
deferred with the Loader UI feature; `LoaderViewRequest.java` is the intended
carrier for the latter. Declared view kinds and `revoke_loader_views` are
implemented and unit-tested but unwired while no `[client]` package exists.

Next: A10 — `resident_perception` does not see a test-spawned hostile, so the C4
archer-ammo/LOS/ally-policy and attack→retreat→patrol behaviours are implemented
but unproven (both tests `#[ignore]`d with that reason).

## C4 execution landed (verified)

Agent `C4Exec` (56m38s, verdict `changes`) landed the mc-net execution half:
`script/storage/resident_orders.rs` (870), `resident_order_execution.rs` (2429),
`play/resident_work.rs` (418), `play/session/resident_orders.rs` (294),
`resident_order_tests.rs` (1278). Design: an order change rides inside the existing
`PreparedStorageBatch` and its operation receipt (`OP_RESIDENT_ORDER_CHANGE` /
`OP_SNAPSHOT_ORDER`), so the admission frame *is* the commit; `recover_resident_orders`
applies pending members exactly once at actor start; server-issued target refs are
persisted with the batch and forged/expired/allied/out-of-reach/wall-blocked targets
are refused; damage commits through `damage_batch_if_current` fenced on the observed
snapshot. mc-script's frozen DTOs needed no change.

Independently re-run by Main: `cargo test -p mc-net --lib --features load-bench
resident_order` → 9 passed, 0 failed, 2 ignored; `cargo fmt`/`clippy -D warnings`
clean for mc-net/mc-script/mc-entity; `cargo check -p mc-net --all-targets
--features load-bench` clean.

Proven: A08 (harvest needs its tool and commits real drops; craft consumes inputs
exactly once; haul moves items between canonical resident endpoints across process
boundaries), A09 (a squad reforms through an open passage and refuses a closed one
with `blocked_route`, no teleport, distinct slots), A11's stale-member leg (a member
dying between prepare and commit changes no order), A12 (a committed admission
replays exactly once and a repeated operation id with a new payload conflicts),
demobilisation without a warehouse keeps the handle and the gear. The tests also
caught and fixed a real bug: haul re-used a stale slot clone and over-reported work
units.

Open C4 gaps, each recorded rather than papered over:
- A10 and the attack leg of A11 are `#[ignore]`d with an explicit reason:
  `resident_perception` returns no candidate for a test-spawned hostile, so the
  archer-ammo/LOS/ally-policy and attack→retreat→patrol behaviours are implemented
  but unproven. This is the next C4 item after `L1Core`.
- Garrison post occupancy has no POI → position resolver yet (reports `blocked_route`);
  demobilisation cannot complete without a warehouse resolver; `construct` is wired
  to C2's committed reservation but has no focused test.
- C1's Lua `transfer_owned_items` still rejects the `resident_equipment` /
  `resident_carry` endpoint kinds, so player↔resident gear movement through the
  script API is not exercisable yet (the execution layer moves gear internally).

## Open regression carried into the next wave: Loader protocol cutover

Wave-1 agent `LoaderSchema2` cut the Loader to bundle **schema 2 / wire protocol 3**
and made schema-1 bundles fail closed, but core still advertises protocol 2 and
schema 1: the core L1 endpoint (`open/present/close_client_view`,
`begin/cancel_client_selection`, `on_loader_view_action`, `view_request`) was never
built, so a Loader client and the current core disagree on the handshake. The Loader
repo's own gates are green (`python3 -m tools.harness run java` PASS,
`20260912T173749-java-bi9gs9sg`) and `loader-live` is the cross-repo gate that stays
red until core lands the cutover.

Plan: land the core L1 endpoint as the next mc-script slice *after* `C4Exec` (both
edit `operations.rs` / `lua/operations.rs` / `lib.rs`, so they must not run
together). Frozen decisions to implement, from
`'/home/kaiserroman/.omp/agent/sessions/-solaris/2026-09-11T15-31-29-764Z_01a09118-6364-762a-a2ac-b4dd04f8e34c/local/settlement-wire-freeze.md'`:
- `view_request { request_kind }` with `request_kind ∈ {settlement, army}` is the
  client→server open request; the Loader-side `LoaderViewRequest.java` is that
  message, not dead code — core must accept it, admit by session + owning plugin +
  declared view permission, deliver it to that owner, and open nothing on refusal.
- the selection context id is a **top-level field of the marker model**
  (`markers[].selection_context_id`), not buried in an unspecified inner shape;
- a view/marker binds to a verified projection through `world_preview_ref` on both
  sides (markers reference a `world_previews[]` entry of the same bundle);
- `entity_presentations` stays deferred (C4 may claim it later).
Owner decision still holds: no shipped plugin declares `[client]`, so nothing
Loader-facing is *enabled* — this slice only restores cross-repo agreement.

## Other carried items

- `materialize_resident` (C3 worldgen seam) still carries a localised
  `#[allow(dead_code)]`; `reserve_resident_site` is used by C2Runtime, the
  materialiser stays unused until the plugin's `populate` path lands. Remove the
  allowance when that caller exists.
- The regression manifest `docs/real-client-regression/manifests/m94-regression-pack.json`
  lacks boolean `no_debug_commands` on 6 of 21 scenarios (including
  `m94-01-join-rejoin-chunks-movement`), so `run regression --run` rejects them.
  Not "fixed" blindly: labelling a scenario's debug-command policy wrongly would
  weaken a gate; decide per scenario when that manifest is next touched.
- Money stays a plugin-side ledger (solaris-economy / the settlements treasury
  projection); there is no core money authority, and none may be invented.
- Probe attribution: the live probe above ran against
  `target/debug/mc-server` sha256 `446e17b57d0b821e228f2767025db183cbd3747a0c2abc6d72cf2e3ee7ea5c0e` (rebuild it before re-probing; see
  `.analysis/codex-logs/live-probe/receipt.md` for the full commands and log lines).

## Live probe of the default plugin set (owner request)

Set installed with `../solaris-default-plugins/install.sh` into
`.analysis/live-probe/plugins`: solaris-permissions, solaris-essentials,
solaris-economy, solaris-towns, solaris-audit, solaris-settlements; config
`.analysis/live-probe/server.toml` (absolute directory, strict, six expected ids,
operators SolarisMcp/SolarisPrimary). `--check` shows all six discovered as
`deployment: "server_only"` with no client bundles or permissions, i.e. the
server-side v1 decision holds. Canonical gate: `run regression --run` with
scenario `m94-02b-rejected-block-resync` PASSED in 27.0 s
(`.analysis/validation/20260912T165404-regression-zjobn0xi`) driving a real
Gradle client under Xvfb.

All six plugins answered with their own messages, including operator and
adversarial paths (`Only an operator ...` refusals, usage text for wrong args,
`Cannot create that town.` on a repeat, `Chunk is claimed ...` on a repeat claim,
`No matching bounded audit records.`). Full table in
`.analysis/codex-logs/live-probe/receipt.md`.

Settlements v1 is live server-side: `/settlement create probetown small` ->
`Founded probetown (small hamlet).`, `list`/`info` report real progress
(`Next village missing: houses 0/9, residents 0/24, jobs 0/12, food 0/64,
committed meeting hall; pause=running`), `/settlement site probetown` prints the
C2 deterministic candidates (`site_3_0_31f075c1 village origin 1788,0,58 size
192,32,192 buildings=12`, `site_1_0_0ac99d6a hamlet ... 128,32,128 buildings=8`),
`adopt` commits (`Adopted site_3_0_31f075c1 (village): 12 buildings, 8 points of
interest, revision 0.`), the workflow gates correctly (`Adopt a deterministic site
first`, `Survey the plot first`), records survive a server restart, and startup
logs `settlement blueprint catalog validated ... blueprints=22`.

**Fixed and re-verified live**: agent `SurveyBudgetFix` (32m) measured the real
cause — the request was fine (core survey 2.1 ms at 64x64, 8.1 ms at 128x128,
never loads chunks); the *script-visible result* carried one Lua record per
surface column (<=16,384 records; 47 ms at 4096, 190 ms at 16384) against the
50 ms `HOST_EVENT_WALL_BUDGET`, so `set_result` alone guaranteed the trap. The fix
is a bounded aggregate snapshot (plots/water/claimed/chunks/tags) instead of
per-column records; no deployed plugin read the columns, so no plugin change and
no tiling (which would have shipped dead data) was needed. Per-column heights and
slopes deliberately no longer cross the script boundary.

Post-fix live chain, same real client: survey -> `plots=4096 chunks=loaded`,
project -> `house_sm_1 projected (solaris:house_small, 4 stages)`, fund without
materials -> `Not enough materials in your inventory; nothing was reserved.`,
fund with the authored materials (planks 314/oak_log 16/glass 10/red_bed 10/
torch 2/oak_door 2/crafting_table 1/chest 1) -> `Reserved real materials for
house_sm_1 (84c74c16...)`, build -> `house_sm_1 committed (solaris:house_small)
at revision 28.`, `/settlement info` -> `houses=1`, and a client block scan at the
anchor shows 32 oak_planks / 4 oak_log / 2 glass / 1 torch, i.e. the building is
physically in the world. `grep -c "wall-clock budget exceeded"` = 0.
Screenshots: `.analysis/live-probe/house-front.png`, `house.png` (captured,
unverified visually).

Historical record of the defect: `/settlement survey probetown plot` trapped the Lua host
(`Lua plugin disabled after handler failure plugin=solaris-settlements error=Trap
{ message: "wall-clock budget exceeded" }`), after which the plugin was disabled
and later subcommands answered `Unknown command` (root cause and fix above).

Also noted: `m94-01-join-rejoin-chunks-movement` cannot run via the harness
because the regression manifest does not declare boolean `no_debug_commands` for
it (6 of 21 entries lack it) — manifest gap, not a server bug.

## Settlement overhaul program — waves

Owner order: "добивай поселения полностью" with subagents authorized (still capped at
two concurrent, disjoint write sets). Contract:
`../solaris-default-plugins/SETTLEMENT_OVERHAUL_CONTRACT.md` §10 queue. Frozen
shared interfaces written as local artifacts (not repo files):
`local://settlement-wire-freeze.md` (bundle schema 2 / wire 3, resolved the four
gaps the Loader pass reported: selection context id travels in
`model.markers[]`, markers reference a verified `world_previews[]` entry,
key-driven open uses a `view_request` message admitted server-side, entity
presentation stays deferred) and `local://settlement-blueprint-freeze.md`
(blueprint schema 1, authoring layout, hard limits, determinism, ruins).

Wave 1 (uncommitted):
- **C3 persistent residents** (agent, 42m23s, verdict `changes`): five §6.1 calls
  (`claim/spawn/query/release/set_resident_pois`) as closed DTOs on the C1
  operation envelope, `persistent_residents` capability replacing the old
  `villagers` API, core-owned `ResidentLedger` replayed from the plugin storage
  journal (OP_RESIDENT_CHANGE 13 / OP_SNAPSHOT_RESIDENT 14) so a handle resolves
  to the same UUID after reopen and reports `alive_unloaded`, not `dead`.
  Files: `mc-script/src/resident_operations{,_tests}.rs`,
  `mc-net/src/script/storage/residents.rs` (1166) + `resident_tests.rs` (438),
  `mc-net/src/play/session/script_resident_endpoint.rs`.
  Not finished: site-bootstrap wiring (C2), POI validity (C2), `resident.changed`
  notifications and assignment exclusivity (C4).
- **L1 Loader half** (agent, 32m8s, verdict `pass`) in `../solaris-loader`:
  schema 2 / wire 3 cutover, closed widget set, view-instance + selection-context
  lifecycle, bounded model validation, world-selection input, one shared
  model/validator/presenter for all three adapters, no schema-1 decoder left.
  Graphical U01–U06 still need the harness and the core endpoint (below).

Main integration work after wave 1: `cargo fmt`/`clippy -D warnings` clean for
mc-script/mc-net/mc-entity. Fixed by shrinking the shared types rather than
boxing 29 call sites: both heavyweight `ScriptOperationPayload` variants
(`OwnedInventory`, `Resident`) now hold `Box<...>`, which removed both
`large_enum_variant` findings (C3's new results had grown the old outcome enum);
deleted dead `track_villager_override` and `bootstrap_resident_change`; the two
worldgen-facing seams `materialize_resident`/`reserve_resident_site` are kept
with a localised `#[allow(dead_code)]` and a reason naming C2 as their caller.
Evidence: mc-script `--features lua-runtime` 233 passed, mc-net lib 2118 passed /
8 ignored, mc-net `--all-targets` compiles.

Known debt from wave 1:
- `crates/mc-entity/src/regional.rs` still carries the unreachable old villager
  binding lane (`claim_nearest_villager`, `apply_villager_binding_goal`,
  `release_villager_binding`, purge hooks, `villager_binding_tests.rs`) with no
  callers: delete it in the entity/C4 wave.
- `../solaris-default-plugins/colony-villager-scaffold` still calls the removed
  villagers API: the contract deletes it when P1 lands.
- Wave 1 broke `crates/mc-test-harness/tests/commands.rs`
  (`lua_villager_goal_reaches_the_regional_owner_and_returns_targeted_result`
  fails 0 vs 1); wave 2 migrates it to the resident API.

Wave 2 closed:
- **C2 catalog/sites/construction** landed by agent `C2CatalogSites`, which then
  failed (exit 1) after 1h4m before reporting; the integration owner verified the
  tree and wrote `.analysis/codex-logs/c2-catalog/receipt.md` from verified state.
  Landed: `mc-script/src/settlement_operations.rs` (1097) + tests, catalog loader
  and deterministic sites in `mc-worldgen/src/settlement_catalog*.rs` /
  `settlement_sites*.rs`, execution/receipts in
  `mc-net/src/script/storage/settlement.rs` (1996) + tests (1612),
  `session/settlement_authority.rs`, Lua install, `docs/PLUGINS.md`.
  Main ruled and the implementer landed the bound split: 64-axis is a blueprint
  bound (`MAX_BLUEPRINT_FOOTPRINT_AXIS`), a site is territory
  (`MAX_SETTLEMENT_SITE_AXIS = 256`), pinned by
  `site_territory_footprint_is_accepted_above_the_blueprint_bound`; the interim
  "report the built layout bbox" workaround was rejected. Verified: fmt/clippy
  clean for mc-script/mc-net/mc-worldgen, mc-script settlement 22 passed,
  mc-net settlement 36 passed, mc-worldgen settlement modules green, determinism
  (A02), catalog rejection, rotation, survey/prepare/cancel/replay coverage.
- Harness fallout fixed by agent `C3HarnessTail` (19m10s):
  `mc-test-harness/tests/commands.rs` migrated to `persistent_residents`
  (13 passed), and `cargo fmt -p mc-test-harness` closed the last fmt debt — the
  workspace `cargo fmt --all --check` is now clean (0 diffs).

Wave 3 in flight: **C4** (resident work orders, squad orders/formations,
cross-region group admission, combat commits, equipment, plus deleting the dead
mc-entity villager binding lane) and **P1 v1 server-side** (the merged
`solaris-settlements` package: strict manifest, authored blueprints, growth /
economy / population domain logic, no Loader dependency).

Owner decision recorded: settlement plugin **v1 is server-side only**; everything
Loader-dependent stays implemented-but-disabled (`local://settlement-wire-freeze.md`
frozen, Loader repo work landed and unused for now). Next after wave 3: a live
probe of the default plugin set with a real player, then fixes.

## Bounded region pregeneration CLI + login-burst flake closed

`mc-server --config server.toml pregenerate --from x,z --to x,z` (uncommitted,
base tree f77525da). Inclusive block corners, either order, rounded outward with
`div_euclid`; fail-closed cap `MAX_PREGENERATE_CHUNKS = 4_194_304` (4096x4096
chunks). Runs the same startup path as serve (contract/baseline/seed), generates
the spawn window plus the rectangle through one shared worker batch
(`generate_chunk_positions(..., label)` — `label` = "spawn" keeps the tested
panic/incomplete messages), flushes dirty chunks, logs
`region pre-generation finished; every chunk is on disk`, exits before the
listener. Args need `allow_hyphen_values = true`: clap's negative-number
heuristic rejects `-600,900` because of the comma (`allow_negative_numbers` is
not enough).

Evidence: debug run `--from 2000,2000 --to 2060,2060` -> 16 region chunks,
`flushed=241`, `world/region/r.3.3.mca`; installed release (`~/.local/bin/solaris`,
18:16) `--from -600,900 --to -450,1050` -> 100 chunks, region `r.-1.1.mca`;
serve then opened that world with `existing world startup spawn window warmed
... region_files=8` and reached `Solaris is listening`. Tests: unit
`parses_pregenerate_block_coordinates`, `region_positions_normalise_corners_and_refuse_absurd_requests`,
`region_pre_generation_stores_every_requested_chunk_and_repeats` (reopens the
storage and proves the chunks are on disk); CLI
`pregenerate_rejects_malformed_block_coordinates`,
`pregenerate_cannot_be_combined_with_check`,
`pregenerate_accepts_negative_coordinates_and_repeats_on_a_fresh_world` (real
binary twice on one tempdir world: success, `solaris/world.json`, region files).

Flake closed: `plugin_owned_command_argument_limits_do_not_terminate_play_ingress`
(crates/mc-server/tests/play.rs) failed under load because the test client never
read the login burst (tab list + roster fills the socket), so the session task
blocked writing and `PlayerCommand` missed the 2s budget. Fix is the file's own
convention: `drain_initial_play_burst` after `drive_to_play`. 8/8 green at
0.76-0.80s (failure path was 2.5s). No production change.

Gates: `cargo test -p mc-server --all-targets` green (79 lib / 46+1 ignored bin /
2 / 42 cli / 14 / 0+4 ignored / 12 / 1 / 19 play / 2), `cargo fmt -p mc-server
--check` clean, `cargo clippy -p mc-server --all-targets -- -D warnings` clean,
`code-health` PASS (20260912T111836 and 20260912T111927). Workspace `cargo fmt
--all --check` still reports 58 pre-existing diffs, all in
`crates/mc-test-harness/tests/**`, none touched here.

Docs: `docs/OPERATING.md` "Pre-generating a region" + worldgen revision 21 text
(was stale at 20) + console-vs-CLI operator/whitelist wording (console commands
apply live, standalone CLI applies at next start) + `whitelist.json` defaulting;
`README.md` mentions the subcommand.

Reviewer round (read-only agent `PregenerateReview`, 7m5s, verdict
"incorrect", 0.8) found five real defects, all fixed:
1. `pregenerate` wrote Solaris terrain into an unversioned vanilla Anvil import
   that serve keeps read-only -> `ensure_pregenerate_target` now bails
   fail-closed on `WorldSource::ExistingVanilla`;
2. `insert_generated_chunk` bypasses the disk-first rule, so a rerun rewrote
   stored chunks (reviewer measured changing md5s) and would erase in-game
   edits -> `WorldStorage::chunk_is_stored` (resident-or-on-disk probe, no
   payload load, no generator) + `pending_region_positions` split; the log now
   reports `chunks/generated/skipped/flushed` and stored chunks are never
   regenerated;
3. the constant comment and the operator doc called 4,194,304 chunks a
   4096x4096 square -> corrected to 2048x2048 in both places;
4. the CLI test's `regions.count() > 0` passed on spawn-window files alone ->
   now asserts the rectangle's own r.-2.1/r.-1.1/r.-2.2/r.-1.2 files;
5. the cap check ran after the world had been created and the spawn window
   generated -> positions (and the cap) are resolved before the world is
   touched, so an oversized request cannot create a world.

Those fixes are verified live with the rebuilt release binary (installed
`~/.local/bin/solaris`, 12:00):
- oversized request `--from 0,0 --to 999999999,999999999` -> `error: requested
  region covers 3906250000000000 chunks, above the 4194304 chunk pre-generation
  cap` and the world directory is never created;
- first run of the -600,900/-450,1050 rectangle -> `chunks=100 generated=100
  skipped=0 flushed=325`; second run -> `chunks=100 generated=0 skipped=100
  flushed=0` and `md5sum -c` reports all four region files unchanged, so a rerun
  no longer rewrites stored chunks or in-game edits;
- unversioned-vanilla-import refusal is pinned by
  `pregenerate_refuses_an_unversioned_vanilla_import` (a live vanilla import is
  not reproducible locally, so this evidence is code-level only).

Final gates on the combined tree (my checkpoint plus the C1 inventory work):
`cargo test -p mc-server --all-targets` green (79 / 47+1 ignored / 2 / 42 / 14 /
0+4 ignored / 12 / 1 / 19 / 2), `cargo test -p mc-net --lib owned_` 27 passed,
`cargo fmt -p mc-server -p mc-world --check` clean, `cargo clippy -p mc-server
-p mc-world --all-targets -- -D warnings` 0, `code-health` PASS
(20260912T120025).

## Settlement contract C1 (inventory) — delegated slice complete, uncommitted

Agent `C1Inventory` (39m56s, verdict `changes`) implemented the C1 inventory half
on top of the existing DTO layer: `inventory_transfers` capability through the
existing `required_features` gate, 5 Lua functions installed, new
`crates/mc-net/src/play/session/owned_inventory_endpoint.rs` +
`crates/mc-net/src/script/storage/owned_inventory.rs`, and one durable
world-journal decision per mutation carrying the plugin receipt and the player
after-image (idempotent by `operation_id` + canonical fingerprint; typed
failures). Its crash test forks the process and `kill -9`s at three durable
boundaries, then reopens and replays: the item moves exactly once, and reverting
to two independent writes fails the test. Receipt:
`.analysis/codex-logs/c1-inventory/receipt.md`.

Two documented gaps, both correct scope boundaries rather than shortcuts:
1. the warehouse endpoint cannot resolve container block-entity NBT from the
   storage actor (that needs `&mut mc_world::WorldStorage` in the simulation
   interaction path); the endpoint kind is defined and fails closed with
   `unloaded`, and the handle issuer is documented for C2, so player<->warehouse
   transfer and cross-endpoint reservation blocking are not exercisable yet;
2. reservation `consumed` is always 0 in C1 because consumption receipts are
   written by the C2/C4 world/work operation; release arithmetic and the
   blocking predicate are implemented and tested.

next: owner decides whether to keep the pregenerate chunk cap and whether to
commit these two checkpoints; the next contract task is C3 (persistent resident
handles) or C2 (catalog/sites), which also unblocks the warehouse endpoint.

## GitHub release v0.0.6 republished with current binary

Built exactly per CI release-build (locked, x86_64 target), packaged,
smoked (VERSION/version/--check), uploaded with --clobber over the old
assets, installer verified end-to-end into a temp dir. aarch64 asset
still comes from CI tag runs only.

## v0.0.6 at d8ec969e — journal race test deterministic

CI failed the journal test twice despite the fail-closed record_commits
flag check: the test asserted post-mortem timing and could win against
worker teardown. Test now subscribes to the worker's own failure_reporter
watch channel (plus post-subscribe flag re-check), tokio::test with a
5s fail-closed timeout — push, not pull, no Instant polling. 30/30 local.
Production record path unchanged since 881461ca.
next: watch CI d8ec969e; owner retests recipe book on real client.

## v0.0.6 at 5f72864d — recipe_book_add client kick fixed

Symptom: real 26.1.2 client (NeoForge) kicked at login with
`Failed to decode packet clientbound/minecraft:recipe_book_add`,
`NoSuchElementException` inside ingredient decode. Decompiled vanilla
client.jar + NeoForge universal: vanilla HolderSet tag branch does
`registry.get(tagKey).orElseThrow`. We shipped 16 item tags in UpdateTags
but recipes reference 40 (e.g. `minecraft:coals` for torches) — first
unknown tag kills the client. Fix (5f72864d): added the 27 missing tags
with vanilla member lists (all members resolve in our ItemRegistry);
new regression test fails pre-fix naming the tags, green post-fix.
Reviewer verdict pass (0.9). CI on the tag in flight; real-client retest
pending with owner.

## v0.0.6 at 881461ca — all known reds fixed, quarantine lifted

Tag v0.0.6 = 881461ca (main pushed). Since 68549c44:
1. Journal race (CI-only): `record_commits` now checks the writer death flag
before enqueue — buffered send could succeed mid-teardown, accepting a commit
never persisted. 30/30 stress green; mc-net lib 2085/0.
2. Village defense un-quarantined and green (15.4s): root cause was the
ravager killing the observing player ~tick 230 (live=0 clears active chunks,
simulation freezes with golem ~3 blocks short). Observer now goes creative;
no production behavior change. Spawn ~tick 100 (villagers join projections
only each 100th tick — by design), pursuit ~1 block/s, attack lands ~260.
Full local L2-equivalent: mc-net lib green + defense file green; CI run on
the tag is the remaining gate.
next: watch CI 881461ca; villages gameplay follow-up (golem tuning only with
vanilla evidence).

## v0.0.6 republished green (tag moved, main pushed)

Full harness `test` PASS on committed tree
`.analysis/validation/20260911T181117-test-vzzubkvr` (268s). Tag v0.0.6 now at
16333834 (was f6da95d8); main pushed. Two fixes since f6da95d8:
1. village_defense attack test quarantined with #[ignore] + reason (spawn ok,
zero golem EntityEvents in 20s over wire; registry-level plan/commit/goal/
velocity all verified working — pursuit-vs-commit gap tracked for villages
follow-up, do NOT re-ignore further reds without owner consent).
2. witch_presence fixed for real: stale pre-flattening effect ids (slowness
2->1, poison 19->18) vs vanilla registry report
(`data/vanilla/reports/registries.json`: poison protocol_id 18); production
enum was already correct, test constants updated, 4.9s green. The old
full-run log never executed witch_presence (run stopped after the village
failure), so witch was never green — not a flake.
Receipt: `.analysis/releases/public-v0.0.6/receipt.json` updated
(tag_republished_ci_green_local). All temp probes removed; owned diff vs
f6da95d8 is exactly the two test files. Maturity draft.
next: villages follow-up — golem pursuit/attack root cause (wire census:
golem spawns, no EntityEvent at all); then un-quarantine the defense test.

## Test-repair sweep (dead field + stale wire expectations, no push)

Removed proven-dead `StructureSetFacts::placement_type` (+`RawStructurePlacement::type_id`
parsing, no consumer; `grep placement_type` empty). Backfilled `stew_effects:
Vec::new()` into 17 stale `FurnaceSlot`/`RecipeResult` test constructors.
Aligned stale zombie fixtures to HEAD 2.3 (`HOSTILE_FOLLOW_SPEED`, regional goal
test now pins target + 2.3 speed). Moved one heavy pickup integration test onto
the existing 4MiB-thread pattern (stack overflow fix, no behavior change).
Fixed two obsolete hurt-event expectations to 26.1.2 `ClientboundDamageEvent`
(PVP helpers + `player_entity_killed_lua` nonlethal fence; production untouched).
Rewrote the flaky short-grass seed wire test into a deterministic single-break
update+ack transaction (1/8 seed probability stays covered by
`mc-data/tests/plant_loot.rs`; prior form failed ~7/8 plus a 2026-08-30 known-flake
record). `SimulationAuthority` kept as capability token per review.
Validation: mc-data 261, mc-worldgen 137, mc-entity 635, mc-server 76, mc-net
2085, block_edit 36/0/70ignored, fmt + code-health PASS, one read-only reviewer
pass (pre-lua/grass deltas). L2 `correctness` red on
`village_defense_spawns_golem_and_attacks_hostile_over_tcp` (golem spawns,
no attack in 20s; fails isolated too; zero overlap with owned files —
pre-existing, needs its own vanilla-evidenced slice, not fixed here).
Ignored `mob_presence` helper holds the same obsolete EntityEvent-2 shape;
untouched (needs sidecars, unverifiable here) — follow-up with the defense slice.
base_tree: 961ed9ecb596b363d40360c2ce37605c78b7bf7a
diff_hash: 366c88566bcfb750b87814ce106ae6d1a44feb5c1df8ec423779bceb7901b38b
changed_files (13 owned, uncommitted): worldgen_structures.rs, regional.rs
(test only), play.rs (cfg-test const), startup_data_tests.rs,
block_edit.rs (DamageEvent import), campfire.rs, chests_and_hoppers.rs,
furnaces.rs, pvp.rs, survival_lifecycle.rs, survival_pickup_overflow.rs,
wheat_seed_source.rs, player_entity_killed_lua.rs. No commit/push. Maturity draft.
next: village-defense golem-attack slice with vanilla evidence (spawn ok,
attack never arrives); then re-run L2 green.


## Combined commit 961ed9ec (owner-authorized, no push)

One local commit with all 6 slices (owned 18 files only, path-limited):
ender emission, fluid wash, nether gen, end gen, structure loot, chest-loot
wiring. Validation per slice as recorded above; tree was fingerprinted stable
across the final gate. Known reds documented in the message. Left dirty and
unstaged: WATCHDOG.yml model swap, worldgen_structures placement removal,
regional.rs (also caught my earlier blanket `cargo fmt --all` — content-neutral
reformat of a stranger file, not staged), startup_data_tests + harness stew
fixes, .analysis deletions, bench json. No foreign session reachable via hub
(all peers are own subagents) — authorship of those edits undetermined.

## Old questions closed (owner decision 2026-09-11)

1. Stale-baked-light repair: NO automatic repair. Rationale: per-edit
invalidation already covers live changes; a validity bit is new persisted
state (vanilla-shaped but invasive) and a force-relight-all is unmeasured
work. Worlds baked pre-fix keep old light until retouched — same wart vanilla
carries across its own light fixes. Reopen only with a measured complaint.
2. Lava wash: KEEP stop-at-plant. Rationale: no vanilla evidence for
lava-vs-plant displacement or drop-burning semantics; changing it would be
invented behavior. Water wash stands alone as the reported gameplay case.

## Chest-loot startup wiring: done

Startup loads `simple_dungeon` + `village_toolsmith` tables from
`<vanilla_data_dir>/data` into `TerrainGenerator::with_chest_loot`; missing
dir/tables warn + keep fixed loot (hermetic fallback test). 8-arg Clippy lint
fixed by bundling into one `Option<(catalog, items)>` tuple (7 args), not
suppressed. Validation: server bin 40 passed; strict bin Clippy clean
(lib-test `stew_effects` initializer is a pre-existing HEAD breakage, left
alone); fmt + code-health PASS. Covered by the same independent reviewer as
End (disjoint paths). Nothing staged/committed/pushed.

## End generator foundation: done

`EndGenerator` worker slice (mirrors nether): end-stone island with void rim,
obsidian pillar ring + bedrock caps, single `the_end` biome, order-free
overlay, 4 tests green. Validation: worldgen lib 137 passed (nether 4 + end 4
included); strict Clippy clean; fmt + code-health PASS. Independent reviewer:
correct, no findings. Portals/travel/multi-world stay queued (needs server
coordination, Main-owned). Nothing staged/committed/pushed.

## Settlement plugin worker: P1-a done (sibling repo)

`SettlementPlugin` closed `solaris-settlements` P1-a: settlement domain ledger
(contract 3.1/3.2/3.3-roles/7-records) on API 0.6.0, storage-only. Validation:
STRICT-OK (real loader discovery+typecheck, fail-closed on unknown capability
and stray entries) and BEHAVIOR-OK 40/40 (lifecycle, gates, roles,
conflict-retry, restart recovery, abandon, orphan-free). Receipt:
`../solaris-default-plugins/server/evidence/solaris-settlements-p1a/receipt.md`.
Next slice: P1-b intent ledger; most value after core C1 `storage_batch_cas`
lands. No core files touched; nothing staged/committed/pushed.

## Nether review fixes (same slice)

Reviewer verdict was `incorrect` on tests only (production code held):
(1) buried-lava assert was inverted — replaced with per-column lava-xor-rock
exclusion; (2) ceiling-band interior 123..126 unasserted — now pinned to
bedrock-or-netherrack; (3) determinism never exercised order independence —
now interleaves neighbors + a second same-seed generator and compares full
columns. Advisor also caught a real generator bug: buried lava under high
columns + hollow air to the roof. Fixed to solid body (land = netherrack to
field, lakes = lava to sea level). Lake frequency retuned (field mean 52,
range [22,82]) after proving zero lakes across 256 chunks. Constants 32/127
labeled vanilla, [22,82] labeled Solaris tuning. Revalidated: full
`mc-worldgen --lib` 133 passed, Clippy/fmt/code-health green. No second
reviewer (findings fixed per policy). structures `pub use` re-export
accidentally dropped by a lib.rs edit — restored.

## Structure loot worker: done

`StructLootRolls` closed chest loot rolls at paste time: `TemplateChest` gains
`loot_table`, rolls are SplitMix64-deterministic per (seed, pos, chest index)
with vanilla overwrite-into-random-slots semantics, fixed contents stay as
fallback. New `mc-data/src/loot/chest_26_1_2.rs` compiles the exact JSON
surface of `simple_dungeon` + `village_toolsmith` and fails closed otherwise.
Validation (worker): worldgen lib green, 4 new loot tests + 7 chest tests pass,
Clippy/fmt/code-health green. Queued next: production wiring
(`ChestLootCatalog::load_vanilla_tables` at startup via `with_chest_loot`;
live servers still paste fixed loot until then).

## Current checkpoint: nether generator foundation (dimensions slice 1)

New `mc-worldgen/src/nether.rs` (`NetherGenerator`, `ChunkGenerator` impl):
bedrock floor y=0, fbm height field [26,92], lava sea below 32, netherrack
body, rough bedrock ceiling cap at 127 (4-deep hashed band), air above to
256, single `nether_wastes` biome. Deterministic per seed+chunk, order-free.
Generation-only: no server wiring, no portals/travel/respawn (single-dimension
architecture queued as its own slice). Multi-biome regions, ores, glowstone,
fortresses queued.

Changed (new files): `nether.rs` + `nether/tests.rs`; `lib.rs` module export.
Validation: 4 nether tests (floor/ceiling, body invariants over 16 chunks,
determinism, missing-block startup error); full `mc-worldgen --lib` 129 passed;
strict Clippy clean; fmt + code-health PASS. Independent review pending a free
agent slot (cap 2/2 busy: plugin + structure-loot workers). Maturity `draft`.

Next: End generator (same pattern), structure-loot worker result, portal/
multi-world architecture, graphical gates.

## Current checkpoint: stale baked light repro (handoff issue 7, migration half)

Headless repro proven (throwaway, removed after green): all-zero block light
baked over an emission-15 cell is served verbatim at stream with zero compute;
the same chunk without baked light recomputes nonzero. So chunks baked under
the old opacity-15/no-ender-7 metadata keep serving darkness after a fixed
binary is installed, until a light-changing edit retouches them. Per-edit
invalidation cannot repair them — nothing re-touches them.
Vanilla precedent (decompiled server-26.1.2.jar, `javap -c`): Anvil stores
`isLightOn` + light arrays; load applies `setLightCorrect(isLightOn)` and
false chunks relight through the engine. We have no such bit; baked light is
trusted unconditionally. Repair choice queued for owner (below) — nothing
automatic built.

Changed: none kept (throwaway reverted; receipt in subagent history
`history://StaleLightRepro`). No commit/push. Maturity `draft`.

## Current checkpoint: fluid wash (plant follow-up, water half)

Flowing water now displaces ground-support plants and columns
(`is_water_washable_plant`: shared ground set + shared column set, minus
seagrass/tall_seagrass which live submerged). The wash edit carries the
same read preconditions as any fluid edit; cells above a washed plant pop to
air through the reused break-path cascade; drops resolve at commit from the
previous states with survival loot rules (no tool) and a deterministic
tick+pos seed — upper double halves yield nothing. Lava keeps stop-at-plant
(queued, not a desync). Block-delta broadcast and light publication order at
the fluid commit are untouched (one advisor-caught near-miss restored before
validation).

Follow-up: column match deduped onto shared
`block_break::is_vertical_support_cascade_block` (no forked list; seagrass
exclusion stays the single documented water-specific rule, pinned by test).
Revalidated: fluid_runtime 15/15, `mc-net` Clippy clean, fmt + code-health PASS.
Lava wash deliberately untouched (no vanilla evidence for lava-vs-plant;
stays queued, current stop-at-plant is no regression).

Changed: `play/fluids.rs` (predicate + flow arm + cascade + `fluid_wash_drops`),
`play.rs` (`spawn_fluid_wash_drops` wired into `run_scheduled_fluid_ticks_owned`),
`play/tests.rs` (fixture +poppy/short_grass/tall_grass-halves/seagrass, old ids
stable), `play/tests/fluid_runtime.rs` (5 regressions: poppy wash, tall-grass
cascade, seagrass coexistence, lava stop, poppy-drop/upper-skip loot).

Validation: fluid_runtime 15/15; plants 80; session::tests 283; strict `mc-net`
Clippy clean; harness `fmt` PASS (`20260911T145709-fmt-p5798_17`), `code-health`
PASS. Full `mc-net --lib`: 2084 passed, 1 failed —
`hostile_pathing_keeps_full_speed` ALSO fails on clean HEAD 7518c29d
(HEAD zombie-speed slice leftover, pin still expects 1.25 vs attribute 2.3);
left for the slice owner, not re-pinned here. One read-only reviewer: correct,
no findings. Maturity `draft`; no commit/push without authorization.

Next: bucket OUTLINE nit (low confidence), stale-baked-light migration question,
all graphical gates. Lava wash queued.

## Current checkpoint: light publication trace (handoff issue 7, ordering half)

Traced immediate, deferred-mutation and deferred-storage relight paths with
no server-side ordering bug found. Deferred-mutation publishes through
`publish_computed_light_updates` (conditional publish, recompute fallback);
deferred-storage checks `incremental_light_sources_are_current` with a full
`collect_full_light_updates_for_current_world` fallback; the initial stream
serves baked light when present else computes. `block_edit_changes_light`
compares emission/opacity/sky, so the chest opacity fix correctly silences
ordinary-chest relight while the new ender emission 7 correctly triggers it.
The dark→correct→dark intermittency is not explainable by static metadata
and was not reproduced headless; it stays open pending the graphical gate
(client-side staleness vs stream ordering still unverified).

Changed: none (investigation only). No fake resends or forced brightness.
Validation: focused `mc-net --lib light` 29 passed (covers relight fencing,
baked publish, prepared-chunk invalidation). Maturity `draft`.

Next: fluid wash (needs drop plumbing in fluid-tick plans), bucket OUTLINE
nit (low confidence), all graphical gates.

## Current checkpoint: ender chest emission (handoff issue 7, emission half)

Conservative fallback gave every ender_chest state emission 0; vanilla 26.1.2
`block_light.json` rows for all 8 ender states are `[7, ..]` (ordinary/trapped
chests are 0). Fix: `conservative_emission` returns 7 for `ender_chest`,
after the candle branch, before the 0 fallback — reachable, shadow-free
(no earlier `contains` arm matches). This is the production path: the owner
server loads `blocks-report-conservative`, not the sidecar report.
Opacity still follows the chest waterlogged rule (0/1, never 15).

Changed: `mc-data/src/block_light.rs` (branch + `conservative_ender_chest_
emission_is_seven` regression + ender pin in ignored `real_table_matches_
known_blocks`, verified passing against local reports).

Validation: `mc-data` full 254+4+2+11+3 passed, strict `mc-data` Clippy clean,
harness `fmt` PASS (`20260911T142843-fmt-bj5g1rjg`), `code-health` PASS
(`20260911T142847-code-health-jotfj1vl`). One read-only reviewer: correct,
no findings. Maturity `draft`; no commit/push without authorization.
Intermittent dark-chest behavior itself still needs the publication-ordering
half + graphical gate.

Next: light publication ordering, then fluid wash (needs drop plumbing in
fluid-tick plans — not a one-line `can_flow_into` widening, which would
destroy plants without drops). Queued: all graphical gates, bucket OUTLINE
nit (low confidence, gameplay-only).

## Owner field follow-up — 2026-09-11 (uncommitted)

Latest priority: delayed leaf drops and expensive random ticks. Leaf fallback
hardness now matches 0.2; the packet-to-owner regression confirms STOP at tick 5
commits both air and a deterministic configured drop without delayed ticks.
Random candidate selection moved into `play/random_ticks.rs`: snapshot only the
budgeted chunks and generate samples only in eligible sections, retaining seed
offsets and order. Copied owner-region benchmark and exhaustive parity receipt:
`.analysis/codex-logs/field-followup-20260911/random-tick-receipt.json`.
This is candidate-stage evidence, not a new live-server p95 measurement.

Related fixes in this working tree: actual 26.1.2 damage packet instead of legacy
entity event 2, squid max health 10, pig passive behavior initialization,
Overworld timeline tags, and queued crafting clicks processed despite stale
state ids. Independent read-only review found stale carried predictions still
vetoed crafting actions; that veto is removed for stale packets and the
three-slot regression now supplies an incorrect nonempty cursor prediction.
Owner inventory fences remain authoritative.

Food recipe completeness and special edible-item effects remain open.
Basic food metadata now covers
all 40 consumable foods from local vanilla 26.1.2 reports: nutrition, saturation,
duration, animation and stack limits. Cooked cod/salmon/mutton now pass a server
use-item-to-commit regression (timing, debit, hunger and persisted state).
Source receipt: `.analysis/codex-logs/field-followup-20260911/food-data-receipt.json`.
This source change is not installed and has not had owner/client acceptance.
The `food.can_always_eat` flag now crosses start, completion and owner commit:
golden apples/chorus fruit consume at full hunger, ordinary food still cannot.
Real report loading excludes food holders without a consumable component.
Container remainders now commit with food: replace the final portion in hand,
otherwise use canonical inventory insertion, then publish one overflow entity.
Four consumption regressions cover hunger/timing and hand/merge/full-inventory
conservation; four owner food-transaction regressions and six item-component
tests pass. Strict affected-crate Clippy passes; `mc-net --lib` reports
2076 passed, 8 ignored. Independent remainder review found no defect.
Food eligibility is closed as the 40-item eating contract, not full special-item
parity. Consumption-effect execution and recipe-specific stew components remain
open with the food-recipe work. Do not reopen combat/absorption while closing
food eligibility.
Player status effects now use the persisted active-effect store. A save/load
regression covers unsorted effect input, hidden-effect restoration, actual
health/food mutations, and decoded packets for owner, tracker and late tracker.
The focused regression passes. No new graphical acceptance or binary install.
Manual leaf/grass loot now uses contextual probability rules: 11 leaf variants,
shears/Silk Touch preservation, Fortune tables, and short grass seed chance 1/8.
An empty roll stays empty instead of falling through to the block item.
Pre-fix: grass dropped seeds on 65,536/65,536 breaks and shears returned apples.
Post-fix: three deterministic distribution/tool regressions and the network
fallback regression pass; affected `mc-net --lib` 2074 passed / 8 ignored,
strict affected-crate Clippy and harness code-health passed.
Receipt: `.analysis/codex-logs/field-followup-20260911/plant-loot-receipt.json`.
These loot changes are source-only; no graphical acceptance or binary install.
The previously built debug binary remains installed at `~/.local/bin/solaris`;
SHA-256 `e0ce9ccf7fb25bb77deb524fee27c26711e406ec6da343d924374bad671c532a`.
Installation receipt:
`.analysis/codex-logs/installation-20260911T023358Z/receipt.json`.
The owner then verified time and crafting-table interaction as fixed.
Eating fish failed in the installed binary; the source-only regression above
does not replace owner/client acceptance. Hurt reactions mostly work, but squid
first-hit damage still fails; the 10-HP data regression is not acceptance.
Animals flee more slowly than vanilla, zombies move more slowly, and skeletons
hold bows without a visible string/arrow draw cycle. Those observations remain
open owner failures. Future fixes must address shared consumption, damage,
movement and bow-use paths, including newly added archer mobs, not type-name
exceptions. No owner process, configuration or world was changed by the agent.
Maturity remains draft; the above graphical observations are owner-run. Base tree:
`c9f3fba3087af9fd0b7510e1e55c160e21e5d209`. Evidence and validation closeout:
`.analysis/codex-logs/field-followup-20260911/checkpoint.json`.
Final affected scope: `mc-net --lib` 2073 passed / 8 ignored; field runtime
facts 3 passed; harness fmt, code-health and strict workspace Clippy passed.
Full workspace tests are not green: the retry stalled in
`mc-script::lua::loader_tests::shipped_two_owner_live_gate_fixture_is_discoverable_and_runnable`
and was interrupted; its log path and the earlier corrected failures are in
the checkpoint receipt. No change to that unrelated fixture was made.

## Current checkpoint: skeleton ranged slice (handoff issues 11-equip + 12)

Skeleton arrows now carry vanilla spread: normalize → per-axis triangular
offsets scaled by 0.0172275 × divergence → scale by 1.6, no renormalize
(javap evidence from local server-26.1.2.jar `AbstractSkeleton` +
`Projectile`). Divergence 10.0 = 14 − 4 × EASY, matching the advertised
`ChangeDifficulty 1`; seeded per (shooter, tick), no shared RNG; crossbow
passes 0.0 (bit-identical path). Skeletons/strays/bogged project a bow in
the main hand at spawn (same `finalizeSpawn` projection as pillager
crossbows, covering late trackers through the shared snapshot fn). Bow aim
fixed per the same bytecode: `target.getY(0.333)` (≈ +0.6, matching the
existing crossbow offset) plus `horizontal * 0.2` drop compensation (was
+1.0, no compensation). Draw-pose driver (probed): client `SkeletonModel`
poses BOW_AND_ARROW iff `isAggressive && mainHandItem.is(BOW)` — no
using-item flag needed; aggressive = mob-flags byte bit 0x04, published on
transitions and cleared on lost-target/death/re-track.

Draw pose done (no new state): `server_entity_snapshot_from` projects
`aggressive` for bow skeletons with a `FollowPosition` goal; the hostile tick
evaluates it every tick from the same budgeted projection fetch (no
due-gating, zero owner-lane reads — volley/melee budget tests pin this) and
diffs against the published snapshot, emitting `Byte{15, 0x04}` on change and
silence when steady. Index 15 by javap: Entity defines 8 accessors (0-7),
LivingEntity 7 (8-14, matches `LIVING_FLAGS=8` pin), Mob 1 → 15, PathfinderMob
0 → `AGEABLE=16` pin holds; aggressive = `Mob.isAggressive` bit 0x04.

Changed (uncommitted, C1 preserved): `session/outbound.rs` (flag +
`is_bow_skeleton_type_26_1_2`), `session.rs` (re-export), `session/
visibility.rs` (projection + shared bow predicate), `session/
visibility_tests.rs` + `wire_entities_tests.rs` (literals), `play/
wire_entities.rs` (index/bit consts, pairing + update encode, wire tests),
`session/hostile_authority.rs` (every-tick check + diff publish), `session/
tests.rs` (acquire/loss/steady/pillager-control test).

Validation: full `mc-net` lib 2069 pass, Clippy clean, harness `fmt` PASS
(`20260910T234125-fmt-inist4vt`), `code-health` PASS
(`20260910T234129-code-health-_e243jij`). Three read-only reviews (spread+
equip, aim delta, draw pose): the draw-pose review caught a real early-return
drop (fixed: publish helper called from both tick branches; test proven to
fail muted and pass fixed). Closing nit: pairing gated by bow type for
symmetry with the update path (+ leak-guard test). Maturity `draft`; no
commit/push without authorization.

Next: time (8) needs the graphical gate — blocked headless. Queued: fluid
wash, all graphical gates, light publication ordering, ender emission,
bucket OUTLINE nit.
## Current checkpoint: mob hurt flash trace (handoff issue 10, signal half)

Traced all three hit paths with vanilla evidence; no server-side signal bug
found. `attack_server_entity_locked` (mob/mob, village defense) sends entity
event 2 + knockback on every accepted nonlethal hit; the player path
additionally writes event 2 to the attacker stream; dragon/death paths send
2/`ENTITY_EVENT_DEATH`. Codec verified against local client-26.1.2.jar
`ClientboundEntityEventPacket`: writeInt entity id + writeByte event id,
wire id 0x22 — matches ours. Existing session tests pin `Damaged` outcomes
(which carry the event dispatches) for cow punches. Not yet done: the
observed-client half — attacker/observer/invuln-reject/death in a real 26.1.2
graphical run (entity-id mapping at spawn and client handling unverified
headless). No fake damage events added; no code changed in this slice.

Changed: none (investigation only). Issue 10 stays open past this slice
pending the graphical gate.

Next: skeletons (11, 12). Queued: fluid wash, time/bucket/chest/fish/light
graphical gates, light publication ordering + graphical, ender emission.

## Previous checkpoint: chest lighting opacity (handoff issue 7, metadata half)

Confirmed with the local 26.1.2 block-light report: every chest-family state
is opacity 0 dry / 1 waterlogged, never 15. The conservative fallback table
gave all chests 15 (opaque, no skylight) — the constant-darkness mechanism.
Fix: chest branch (`chest`/`*_chest`, 11/11 family IDs verified, no false
positives) mapping waterlogged→1 else 0, with propagates/suffocating
derived exactly matching vanilla rows. Intermittency (dark→correct→dark)
is NOT explained by the static table; publication ordering + graphical gate
stay queued. Ender emission 7 gap noted, untouched.

Changed (uncommitted, C1 preserved): `mc-data/src/block_light.rs` (branch +
1 test).

Validation: 13/13 block_light; `mc-data` Clippy clean; harness `fmt` PASS
(`20260910T142737-fmt-nmqmafam`), `code-health` PASS
(`20260910T142740-code-health-r9b8pf16`). One read-only reviewer: pass, no
findings. Maturity `draft`; no commit/push without authorization.

Next: mobs (10), skeletons (11, 12). Queued: fluid wash, time/bucket/chest/
fish graphical gates, light publication ordering + graphical, ender emission.

## Previous checkpoint: explosion support cascade (plant follow-up, blast half)

Blasts now pop ground plants/columns above destroyed supports in the same
conditional batch, reusing `append_vertical_support_cascade` (now
`pub(super)`, body unchanged) via `plan_explosion_support_cascade`, which
skips already-destroyed and unreadable cells with per-edit preconditions.
Drops flow through the existing explosion table (upper halves yield nothing,
matching survival semantics). Incidental: chest placement keeps the
same-kind/`single` guard and places single (not abort) when a candidate
neighbor chunk is unloaded. Still queued: fluid wash + placement-neighbor
paths, and all graphical gates.

Changed (uncommitted, C1 preserved): `play/simulation.rs` (helper + hook +
1 test), `play/block_break.rs` (visibility only), `play/block_placement/
chest.rs` + `chest_tests.rs` (unloaded continue + test).

Validation: explosion cascade unit test; 23 block_placement; 20
furnace/chest/plant neighbors; `mc-net` Clippy clean; harness `fmt` PASS
(`20260910T142301-fmt-v7nj2ot7`), `code-health` PASS
(`20260910T142305-code-health-ypubcobo`). One read-only reviewer: pass, no
findings. Maturity `draft`; no commit/push without authorization.

Next: lighting (7), mobs (10), skeletons (11, 12). Queued: fluid wash,
bucket OUTLINE nit, chest trapped/waterlogged/two-player/graphical, fish
campfire/graphical, time graphical gate.

## Previous checkpoint: region flush preservation (handoff issue 9)

`DirtyFlushPlan::write` no longer decodes/retains/recompresses untouched
slots. New `RawChunkRecord` + `read_region_raw` (location/comp/count/
aggregate validation, no retention) and `write_region_create_new_mixed`
(`Fresh` zlib-encodes, `Preserved` copies bytes verbatim with its timestamp).
The existing writer shares the same assembler with identical behavior.
Review drove two hardenings, both fixed: LZ4 compressed blocks are now
checksum-verified while counting (the counter skipped them, unlike exact
decode), and mixed-write validation enforces decode budgets on preserved
slots too — corrupt input fails raw read, mixed write, and decoded read
identically, no silent carry. Unique-tmp/stale-fence/journal/fsync/rename/
parent-sync/dirty-generation semantics untouched.
Benchmark (same 64-slot/4-dirty copied workload, debug build, receipt +
log in `.analysis/codex-logs/flush-preserve-bench/`): rewrite-all 2309ms vs
preserving 286ms (8.1x), retained uncompressed 3.11MB vs 86KB per flush
(36x). Synthetic NBT caveat noted in receipt; owner-workload RSS/CPU still
needs the owner environment.

Changed (uncommitted, C1 preserved): `anvil/region.rs` (raw + mixed +
validator + 4 tests), `anvil/mod.rs` exports, `storage/dirty_flush.rs`
(raw map + mixed tmp + 1 test). Throwaway bench file removed after receipt.

Validation: 288/288 `mc-world` lib; `mc-world` Clippy clean; harness `fmt`
PASS (`20260910T140833-fmt-fl0uwii1`), `code-health` PASS
(`20260910T140837-code-health-7_ar5vrc`). One read-only reviewer returned 2
findings (LZ4 count gap, write-budget gap); both fixed, no second review per
policy. Maturity `draft`; no commit/push without authorization.

Next: lighting (7), mobs (10), skeletons (11, 12), plant explosion follow-up.
Queued parity nits: bucket ray vs grass OUTLINE, chest trapped/waterlogged/
two-player/graphical, fish campfire/graphical, time graphical gate.

## Previous checkpoint: time set trace (handoff issue 8, server half)

Server chain verified end to end with vanilla evidence; no server-side value
bug found. Console `night` parses to 13000 (aliases ruled out already);
`OperatorControlHandle` uses the server-owned fence; simulation stores then
broadcasts; `send_outbound_world_time` maps simulation_tick→game_time and
world_time→overworld total (existing `world_time.rs` test pins both clocks).
Vanilla `javap` on local client-26.1.2.jar: `WorldClocks.bootstrap`
registers OVERWORLD first (id 0) then THE_END (id 1) — our constants match;
`ClientboundSetTimePacket` layout (gameTime, holder-id map, VarLong total,
floats) matches our codec; client `handleSetTime` applies `gameTime` via
`setTimeFromServer` AND clock updates via `ClientClockManager.handleUpdates`,
which keys by holder and sets total/partial/rate. Encoding suspects ruled out.
Not yet done: the observed-client half — capture the actual packet bytes and
sky/hostile-spawn agreement for night/day/noon/midnight in a real 26.1.2
graphical run (no client credentials in this environment). A subsequent
tick/sleep override was reviewed in code shape only (sleep `Skipped` is the
sole alternate publisher; dedup cannot resurrect stale values).

Changed: none (investigation only). No fix claimed; issue 8 stays open past
this slice pending the graphical gate.

Next: handoff issue 9 (region flush), then lighting (7), mobs (10),
skeletons (11, 12), plant explosion follow-up. Queued parity nits: bucket ray
vs grass OUTLINE, chest trapped/waterlogged/two-player/graphical, fish
campfire/graphical.

## Previous checkpoint: fish display IDs (handoff issue 2, logic half)

The 6 real vanilla fish recipes sort right after `minecraft:chest`, shifting
every later display ID +6. Per handoff 43-46 the shift is accepted (no
ordering hack, no fake `zz_` IDs): 76 pins across 15 `mc-data` recipe tests
moved +6, `chest`=5 untouched, production `solaris_required_recipes()`
(BTreeMap sorted + `bone_meal` tail) unchanged. Advertisement and lookup
share the same ordered set, so client/server stay consistent by construction.
A reverted `STABLE_TAIL_IDS` detour is recorded and was wrong: it broke
sorted order to preserve pins, against the handoff.
Queued: campfire real-item + graphical client acceptance.

Changed (uncommitted, C1 preserved): `mc-data/src/recipes.rs` test module
only, on top of the chest + bucket + flower/grass slices below.

Validation: 28/28 `mc-data` recipe tests; fish furnace/smoker regression;
32 campfire; 2 play recipes; `mc-data` Clippy clean; harness `fmt` PASS
(`20260910T133935-fmt-e27bi13b`), `code-health` PASS
(`20260910T133939-code-health-l3s2h58o`). One read-only reviewer: pass.
Maturity remains `draft`; no commit/push without explicit owner authorization.

Next: handoff issue 8 (time set), then issue 9 (region flush). Queued plant
follow-up: explosion candidates (`plan_explosion_candidates`) and
fluid/placement-neighbor removals do not yet reuse `is_ground_support_plant`;
queued parity nits: bucket ray vs grass OUTLINE, chest trapped/waterlogged/
two-player/graphical, fish campfire/graphical.

## Previous checkpoint: double-chest pairing (handoff issue 1, logic half)

Placement pairing proven in both orders with complementary left/right types,
equal facing, and either-half `paired_position` symmetry. Shared `opposite`
from `mc_data::block_placement_26_1_2` replaces the local duplicate.
Changed: `play/block_placement/chest.rs`, `chest_tests.rs` (mirror test).
Validation: 3/3 chest planning, 22/22 block_placement, Clippy clean,
harness `fmt` PASS (`20260910T133149-fmt-7vdqceed`), `code-health` PASS
(`20260910T133152-code-health-r9b8pf16`); reviewer pass, no findings.
Queued: trapped-vs-normal, waterlogged, two-player contents, graphical.

## Previous checkpoint: authoritative buckets (handoff issues 5+6)

Empty-bucket `UseItem` now picks up source fluid through a validated-pose
raycast (source-only, occlusion, 4.5 range) reusing `BucketUsePlan` + the
simulation commit transaction. Filled-bucket `UseItemOn` follows vanilla
ordering (pickup-first, vegetation in-place replace, target reach,
placeability incl. source refusal); every bucket-held outcome ends terminal
via resync+ack with no double-ack, both hands. Unrelated stack moves/swaps,
close/reopen and reconnect stay authoritative. Look math consolidated onto
`player_look_direction` after review (no second convention).
Queued parity nit (not a desync): the ray passes grass vanilla OUTLINE would
stop at; confidence low, gameplay-only.

Changed (uncommitted, C1 preserved): `play/bucket_interactions.rs`
(raycast + hardened ordering + 8 tests), `play.rs` bucket branch + pose
plumbing, `use_item_on_adapter.rs` visibility + pose arg, plus the
flower/grass cascade below.

Validation: 9 bucket module tests; neighbors bucket 19 / fluid 20 /
use_item_on 18; `mc-net` Clippy tests clean; harness `fmt` PASS
(`.analysis/validation/20260910T132734-fmt-k05y538m`), `code-health` PASS
(`.analysis/validation/20260910T132738-code-health-l3s2h58o`). One read-only
reviewer: correct with 2 P3 (look-dup fixed, grass-OUTLINE queued).
Maturity remains `draft`; no commit/push without explicit owner authorization.

## Previous checkpoint: flower/grass support cascade (handoff issues 3+4, break path)

Breaking a support block authoritatively pops poppy, short grass and tall
grass (lower + upper halves) in one transaction; every cascade edit carries a
read precondition (unloaded neighbor chunk rejects the whole batch).
Support-pop carries no held tool (no shears-only grass grant); the upper
double-plant half drops nothing twice. `hanging_roots` excluded after review
(ceiling-hung). Explosion/fluid/placement-neighbor paths still open.
Changed: `play/block_break.rs`, `plant_rules_26_1_2.rs`,
`play/tests/plants.rs` (4 regressions + import/sort fix),
`play/tests/furnace.rs` Clippy borrow.
Validation: 4/4 cascade + 101 plants/block_break; fish furnace test;
`mc-net` Clippy clean; harness `fmt` PASS
(`20260910T121027-fmt-rqtlu3zo`), `code-health` PASS
(`20260910T121510-code-health-twwl4b6u`); reviewer changes (hanging_roots)
fixed.

## Previous checkpoint: owner-requested as-is handoff to main

The owner stopped implementation and explicitly requested an immediate single
commit and push to `main`, with unfinished work documented for another agent.
Read [FIELD_TEST_HANDOFF.md](FIELD_TEST_HANDOFF.md) first: it contains all twelve
reported issues, evidence, source entrypoints, partial changes and acceptance
steps. Do not treat this snapshot as a release or a completed gameplay fix.

Fish cooking has a passing focused before/after regression. Double-chest changes
are partial. The final correctness gate passed formatting but failed strict Clippy
at `play/tests/furnace.rs:1195` (`&format!` needless borrow); full tests did not run.
Receipt: `.analysis/validation/20260910T095908-correctness-0t0ey1bm/result.json`.
The installed owner binary/process/world were not changed by these field fixes.
Maturity remains `draft`.

Next: resume the documented issues, starting with the exact Clippy error and
chest regressions; then close the remaining gameplay and save-allocation defects.
The older checkpoint narrative below is historical, not the active queue.

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

The new target contract is [ARCHITECTURE.md](ARCHITECTURE.md). It is not a claim
that the runtime has already been migrated. Current plugin API: `0.6.0`;
[current reference](PLUGINS.md). Loader has one common implementation and
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
The contract is in [ADR 0005](decisions/0005-regional-simulation.md#journal-durability).

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
