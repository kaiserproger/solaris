# Archived checkpoint log — part 2 of 8

Chronological checkpoint history moved out of `docs/MEMORY.md` so the live cursor
stays small. **Not startup context** (see `AGENTS.md`); read it when a question is
about this era, not to learn the current state.

## Sections in this part

- Settlement profiles: no fake vanilla villages, loud gap (landed 2026-09-14)
- Core-only closeout (owner 2026-09-14: "доделывай че по ядру есть") - landed
- Warehouse write path - C1 slice 2 (landed + verified 2026-09-14)
- Handover snapshot (pushed `2ae90ff4`)
- Queued after handover (dependencies, not motion)
- Handover state (owner manual test)

---

## Settlement profiles: no fake vanilla villages, loud gap (landed 2026-09-14)

The owner corrected the settlement-profile semantics mid-flight: "vanilla
villages belong to vanilla" was **not landable honestly** with today's
`StructureRules` API, and the main agent stopped the five-type loader before it
was written. What the core can express: `plains_village_prototype_with_parts`
combines a *fixed three-template* composite (`village/plains/town_centers/
plains_fountain_01`, `houses/plains_small_house_1`, `houses/plains_tool_smith_1`)
into one template, `plains_village_markers` places that single vector on a 34/8
grid, `with_structure_set_facts` only reads `minecraft:village_plains`, and no
biome filter per village type, per-type pool, or jigsaw assembler exists (jigsaw
blocks are used only to attach villager spawn markers,
`is_plains_villager_jigsaw`). Building a "five-type vanilla village loader" on
that would place three plains houses in every biome — a fake. So the landed
change is truthfulness plus a loud report:

1. **Docs now say exactly what each profile places.**
   `SettlementProfile::Vanilla` (`crates/mc-server/src/lib.rs`) is documented as
   "places no villages: core does not implement vanilla village generation", and
   `PlainsVillagePrototype` as a bounded three-template Solaris composite on the
   extracted plains village spacing — explicitly not full vanilla generation and
   no desert/savanna/snowy/taiga villages. Same for the `DataSection` field and
   `example.toml`.
2. **The silent zero is gone.** The default `vanilla` profile no longer quietly
   places nothing. A shared `VANILLA_SETTLEMENT_NOTICE` (`crates/mc-server/
   src/main.rs`) is emitted as the typed `--check` operator warning
   `settlement_profile_vanilla_generates_no_villages`, and
   `structure_rules_for_startup` logs the same notice with `tracing::warn!` on
   the serve path whenever no plugin plan is deployed and the profile is
   `vanilla`. The notice names both real routes: a deployed Luau settlement plan,
   or the explicit `plains_village_prototype` opt-in with `vanilla_data_dir`.
3. **Plugin-plan precedence untouched and now pinned on CI.** A deployed
   settlement plan still returns first and builds the composite; the new
   `structure_rules_tests::deployed_settlement_plan_replaces_core_villages`
   proves it with the synthetic sidecar (default `vanilla` profile + plan ⇒ 15
   pasted marker blocks).
4. **Identity/revision unaffected.** `Vanilla` placement semantics did not
   change — it still places no structures — so `WORLDGEN_REVISION` stays 21 and
   no existing world gains structures or hits the persisted-contract mismatch
   (`crates/mc-server/src/startup_validation.rs`). Only the report is new.
- tests: `structure_rules_tests::default_settlement_profile_reports_missing_core_village_generation`
  (new) asserts the typed notice fires for `vanilla`, names
  "does not implement vanilla village generation" and the prototype opt-in,
  does **not** fire for `plains_village_prototype`, and that the default profile
  still returns empty rules (reported, not silent);
  `deployed_settlement_plan_replaces_core_villages` (new) as above; the existing
  synthetic-sidecar test still asserts 15 blocks for the prototype vs 0 for the
  default. `cargo test -p mc-server --bin mc-server structure_rules_tests` =>
  3 passed / 0 failed / 48 filtered.
- **open item (the only true vanilla village path, not implemented):** real
  vanilla village generation needs per-type village structure sets with their
  biome tags (`minecraft:village_plains|desert|savanna|snowy|taiga`, biome tags
  under `data/minecraft/tags/worldgen/biome/has_structure/`), each type's own
  template pools, and a jigsaw assembler that reads those pools. None of that
  exists in `crates/mc-worldgen/src/structures.rs`; until it does, no code or doc
  may claim Solaris generates vanilla villages. `PlainsVillagePrototype` stays
  the honest, explicitly-labelled prototype.
- owner/manual: the live proof with real Mojang village data (extracted
  `data/vanilla/` sidecar, startup logged notice absent for the prototype,
  real templates pasted client-side) — this machine has no Mojang sidecar.

## Core-only closeout (owner 2026-09-14: "доделывай че по ядру есть") - landed

Both items that were open here are closed in the core repo; neither needed a
sibling plugin package.

1. Append-before-publication ordering probe - closed, and order-sensitive rather
   than structural. The regional run arms a `#[cfg(test)]` probe
   (`arm_warehouse_publication_probe`,
   `crates/mc-net/src/play/simulation/regional_mutation.rs`) for every
   server-owned warehouse publication it holds, before it appends. The
   observation itself is taken inside the publication path
   (`publication_probe::observe`, called from `dispatch_visibility_commands` in
   `crates/mc-net/src/play/session/outbound.rs`), where the container's
   `ChestSlots` command leaves the run, and the journal records the append state
   at that moment (`WorldChunkJournal::record_warehouse_publication_for_test`,
   backed by `next_append_id`; read back with `warehouse_publications_for_test`).
   Test:
   `play::simulation::tests::server_owned_warehouse_deposit_appends_before_publishing_and_recovers_both`
   (renamed from `..._journals_images_and_receipt_then_recovers_both`) asserts
   the run's only observation is `(decision_id, true)` beside its container,
   journal and recovery assertions. Both failure directions were exercised as
   scratch edits and reverted: publishing before appending (the run's append
   deferred until after the publication loop) fails the assertion with
   `left: [(1, false)]`, and a publication that never passes through the observed
   path fails it with `left: []` - the test cannot pass while the slots leave
   without the decision behind them. Production behaviour is unchanged: the
   probe, its arming and its re-export are all `#[cfg(test)]`
   (`cargo check -p mc-net --features load-bench` clean).
2. Vanilla villages without a plugin - closed as the existing path, verified and
   pinned, nothing re-implemented (no new profile, template, or generator).
   `[data] settlement_profile = "plains_village_prototype"` already builds the
   vanilla plains prototype with no plugin at all (`structure_rules_for_startup`,
   `crates/mc-server/src/main.rs:1313`). The profile *default* is
   `SettlementProfile::Vanilla` in `crates/mc-server/src/lib.rs:150-159` (enum
   `#[default]`) with the config field at `:194-195`; the
   `crates/mc-server/src/startup_validation.rs:33` `vanilla_profile` fallback is
   only the persisted-contract reader's default, not the config default. The
   profile requires `data.vanilla_data_dir` (Mojang NBT never enters Git),
   `example.toml` documents the switch, and the profile name is the recorded
   world identity whenever no deployed plugin plans settlements
   (`crates/mc-server/src/main.rs:667-670`, `:743`). New coverage of the real
   generation path with a *synthetic* sidecar:
   `structure_rules_tests::builtin_settlement_profile_generates_village_structures_from_the_sidecar`
   (`crates/mc-server/src/structure_rules_tests.rs`, included from `main.rs`)
   writes three synthetic structure templates plus one structure-set fact file
   into a temp dir, parses the stock `[data]` TOML, builds the rules through
   `structure_rules_for_startup`, generates the fixed-centre village chunks
   through `build_terrain_generator`, and asserts 15 pasted marker blocks for
   `plains_village_prototype` against 0 for the default `vanilla` profile. World
   identity: `tests::world_contract_accepts_and_persists_the_builtin_settlement_profile`
   creates, persists and reopens a world carrying the profile and refuses a world
   whose persisted profile differs; `builtin_settlement_profile_requires_the_vanilla_sidecar`
   keeps the missing-sidecar refusal.
- validation of record for this revision: `cargo test -p mc-net --lib -- warehouse`
  13 passed / 0 failed, `-- journal` 68 passed / 0 failed / 1 ignored, `-- chest`
  30 / 0, `-- owned_inventory` 19 / 0, and `cargo test -p mc-net --lib` 2188
  passed / 0 failed / 8 ignored; `cargo test -p mc-server --bin mc-server` 47
  passed / 1 failed / 1 ignored - the failure is
  `tests::deployed_sibling_plugins_prepare_runtime_and_worldgen_profiles`,
  which panics on the missing sibling package (see the owner/manual line) - and
  `cargo test -p mc-server --lib` 79 / 0; `cargo test -p mc-worldgen --lib` 167
  passed / 0 failed / 5 ignored; `cargo clippy -p mc-net --all-targets -- -D
  warnings` clean; `cargo clippy -p mc-server --all-targets -- -D warnings`
  clean; `run fmt` PASS `.analysis/validation/20260914T030153-fmt-azhvfdq9`;
  `run code-health` PASS `.analysis/validation/20260914T030157-code-health-64kxddrv`.
- owner/manual, and explicitly NOT run here: L2 `correctness` and the real-client
  gates (they need the unpublished
  `../solaris-default-plugins/solaris-settlements`; this machine has only
  `data/vanilla/README.md`, so the sidecar-present field proof - startup logging
  `materialized built-in settlement prototype` and a client seeing village
  terrain at the seed-0 fixed centre - also stays owner/manual). The
  synthetic-sidecar test covers the generation path itself; it does not claim
  that the real Mojang templates paste identically.
- changed_files: `crates/mc-net/src/play/world_journal.rs`,
  `crates/mc-net/src/play/session/outbound.rs`,
  `crates/mc-net/src/play/session.rs`,
  `crates/mc-net/src/play/simulation/regional_mutation.rs`,
  `crates/mc-net/src/play/simulation.rs`, `crates/mc-server/src/main.rs`,
  `crates/mc-server/src/structure_rules_tests.rs` (new sibling `*_tests.rs`),
  `docs/decisions/0004-staged-single-writer-simulation.md`, `docs/MEMORY.md`.
- next: nothing from the owner's 2026-09-14 core ask remains open. The
  settlements/live-chain items stay blocked on the unpublished sibling package.
  Superseded in part by the settlement-profile section above: real per-type
  vanilla village generation is named there as the one open core item.
- review closeout (independent read-only `WarehouseInvariantReview`, 13m32s): verdict pass,
  `overall_correctness: correct` (confidence 0.76), all seven invariant questions answered with
  `file:line` evidence. Residual it named: the append-before-publication rule is structural only -
  no probe or test fails if publication preceded the append.
- validation of record for the final tree, run by Main after the last source edit: `run fmt` PASS
  `.analysis/validation/20260914T021459-fmt-ei9lc2u_`; `run code-health` PASS
  `.analysis/validation/20260914T021508-code-health-qgw6zram`; `cargo test -p mc-net --lib -- warehouse`
  13 passed / 0 failed; `cargo clippy -p mc-net --all-targets -- -D warnings` clean. Artifact paths
  cited elsewhere in this file that predate the last edit do not cover this revision.

## Warehouse write path - C1 slice 2 (landed + verified 2026-09-14)

- checkpoint_closed: the writable warehouse endpoint (`CommitChest` server-owned
  mode, C1 slice 2). `base_tree` `8414853e`; the previous cursor's design,
  rejected alternatives and mandated test shape are unchanged below/in
  `docs/decisions/0004-staged-single-writer-simulation.md`.
- landing: `SimulationCommand::CommitChest` gained `plugin_receipt:
  Option<Vec<u8>>` (present == server-owned) instead of a parallel command.
  `ChestTransaction::commit_server_owned`
  (`crates/mc-net/src/play/session/transactions.rs`) is the second entry point
  beside the menu `commit`: same composite, minus the `actor_has_open_view`
  fence, publishing `ChestSlots` to every viewer INCLUDING the actor. The
  journaled regional run stamps the container chunk for the run's reserved
  decision id (`stamp_chunks_for_world_journal`), carries the after-image plus
  the encoded receipt in ONE `record_reserved_decisions` group, then releases the
  flush fence, publishes and responds. `command_needs_world_journal` keeps a
  receipt-bearing command out of the non-journaled lanes (the menu path answers
  `WorldMutationFailed` rather than committing unjournaled). A refused deposit
  appends its reserved decision with NO participant, so the run's append stays
  contiguous. The endpoint gate is gone: `commit_owned_inventory_transfer`'s
  `Warehouse` arm was replaced by routing in
  `InventoryRuntime::execute_owned_inventory`, which resolves the binding and
  loaded container through the same helper the read path uses
  (`InventoryRuntime::resolve_warehouse_container`, now shared with
  `warehouse_inventory_snapshot`), fences BOTH endpoints (the DTO already
  requires one fence per distinct endpoint), plans with
  `plan_owned_item_transfers`, prepares the receipt batch with the player
  after-image, sends it through `SettlementWorld::commit_warehouse_transfer`
  (live impl over `SimulationHandle`), then projects the ledger frame + the
  player recovery, publishes the actor's `AuthoritativeInventory` and only then
  `mark_inventory_projected`.
- decision-id ownership (settled, do not re-derive): the RUN allocates the id and
  the response carries it. A player endpoint's receipt fence revision IS the
  post-commit `inventory_operation_revision`, which `PlayerInventoryRecovery::
  recover`/`load_player_state` force to be the journal decision id; a submitter
  pre-reservation would break `record_reserved_decisions` contiguity (regional
  runs reserve one block each and cannot wait without deadlocking the owner task
  that must process the deposit). Consequence, deliberate and documented in
  `docs/PLUGINS.md`: a warehouse transfer receipt's `Transfer` result names the
  WAREHOUSE endpoint's resulting fence (binding revision + hash of the planned
  container slots) and NOT the actor's, whose fence is re-read by a query —
  listing a pre-commit revision would hand the plugin a permanently stale fence.
- changed_files: `crates/mc-net/src/play/simulation.rs`,
  `crates/mc-net/src/play/simulation/regional_mutation.rs`,
  `crates/mc-net/src/play/session/transactions.rs`,
  `crates/mc-net/src/play/session/container_views.rs`,
  `crates/mc-net/src/play/session/owned_inventory_endpoint.rs`,
  `crates/mc-net/src/play/session/owned_inventory_endpoint_tests.rs`,
  `crates/mc-net/src/play/owned_inventory.rs`, `crates/mc-net/src/settlement.rs`,
  `crates/mc-net/src/script/storage/settlement.rs`,
  `crates/mc-net/src/script/storage/settlement_tests.rs`,
  `crates/mc-net/src/script/storage/resident_settlement_tests.rs`,
  `crates/mc-net/src/script/storage/world_inventory.rs`, `docs/PLUGINS.md`,
  `docs/decisions/0004-staged-single-writer-simulation.md`, `docs/MEMORY.md`.
  The cursor line below (journal enabler) stays as landed.
- validation: focused `cargo test -p mc-net --lib --features load-bench` filters
  `-- warehouse` 13 passed / 0 failed, `-- journal` 69 passed / 0 failed / 1
  ignored, `-- chest` 30 passed / 0 failed, `-- owned_inventory` 19 passed / 0
  failed (one pre-existing assertion updated: a warehouse transfer on a runtime
  with no settlement profile now answers `runtime_unavailable` instead of the old
  endpoint-gate `unloaded`); `python3 -m tools.harness run fmt` PASS
  `.analysis/validation/20260914T020927-fmt-zv4q3obc`; `run code-health` PASS
  `.analysis/validation/20260914T020938-code-health-sgsxhyty`;
  `cargo clippy -p mc-net --all-targets -- -D warnings` clean.
- mandatory acceptance test: `server_owned_warehouse_deposit_journals_images_
  and_receipt_then_recovers_both` (`play/simulation.rs`) drives a real
  deposit and asserts ONE decision whose decoded image holds the POST-deposit
  container (real `stamp_chunks_for_world_journal` images, so dropping the image
  fails it) with the batch still attached (so a two-decision split fails it),
  the checkpoint cutoff blocked until projection, the restart reopening the
  container from that image, and `InventoryRuntime::recover` replaying both
  participants; `server_owned_warehouse_deposit_refuses_stale_fences_without_
  mutating` covers the stale state-id fence, the rejected conditional commit and
  the stale player fence (each typed, nothing mutated, no receipt journaled);
  `publish_warehouse_transfer_advances_the_actor_inventory_projection` covers the
  actor's revision + `AuthoritativeInventory`; settlement-level
  `warehouse_transfer_refuses_foreign_unknown_unloaded_and_stale_containers` and
  `warehouse_transfer_commits_both_participants_under_one_decision` cover routing,
  the foreign/absent/unloaded/cancelled refusals and the receipt fence identity.
- not run here: L2 `correctness` and the real-client gates — the sibling
  `../solaris-default-plugins` still lacks the `solaris-settlements` package, so
  `crates/mc-test-harness/tests/settlement_lifecycle.rs` and
  `settlement_pause_repro.rs` panic and those gates cannot be green from this
  checkout. Reported, not worked around.
- next: the deposit path is writable and tested; the remaining C1 work is the
  deferred double-chest container (one 27-slot `ChestBlockEntity` today) and any
  plugin-facing follow-up the owner wants from the receipt-fence narrowing above.

## Handover snapshot (pushed `2ae90ff4`)

- base_tree: `0febdfa3` (pushed to `main`; chain `bdb665cd` -> `2ae90ff4` -> `f313b4a6` ->
  `44eea177` -> `5ebb33c4` batch commit -> `f77525da6b1f7c6e460d1ae538aca409ce9b6d6b`). The
  pause fix in `44eea177` was live-verified as insufficient and is superseded by `2ae90ff4`
  (content-scoped footprint fence), which `0febdfa3` documents in the owning ADR.
- cursor_commits: cursor and documentation bookkeeping is never content; any commit after
  `base_tree` that touches only `docs/MEMORY.md` or only documentation under `docs/` is the
  same kind, so the next session's content diff starts at `base_tree` and any later commit
  listed here is bookkeeping for it.
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
- tree_freeze_proof: before pushing, no other omp/agent writer was alive and two
  `git status --porcelain` + `git diff HEAD` fingerprints taken apart were identical
  (`97a4df1a411b6a64970e446b95eb6ded7fd1bffb5051a65f4950bb545aa08d1d`), with an empty
  index and only the five local-only paths differing from HEAD.
- validation_on_pushed_commit: `run correctness` PASS
  `.analysis/validation/20260913T231751-correctness-wwdhk_td` (244.3 s) re-run on
  `44eea177c0f2a25998d876f5c5945ed0e830ef02`, so the pushed revision itself - not an
  earlier snapshot of it - is the one the L2 gate covers.
- pushed: `main` now carries both live-chain fixes (ingress burst limiter no longer drops a
  serialized command and answers a dropped one; `advance_structure` re-observes the footprint
  after its own portion commit instead of pausing as `site_changed`), together with the
  owner's concurrent cleanup sweep over mc-data/mc-test-harness.
- validation_latest_push: `run correctness` PASS
  `.analysis/validation/20260913T231231-correctness-a7ak29y2` (248.7 s) on the pushed tree.
- fixed_live_pause (supersedes the `44eea177` attempt): the pause was a race with the WORLD's own
  scheduled-block-tick writes, not with the settlement's own commits. Writer:
  `run_scheduled_block_ticks_owned` (`crates/mc-net/src/play.rs:10090`) ->
  `commit_cross_region_scheduled_block_tick` (`play.rs:8630`) ->
  `WorldChunkJournal::record_reserved_snapshot_groups` (`play.rs:8702`). The paused run's journal
  (`.analysis/validation/20260913T232942-regression-eov066g3`) holds WIF1 id=1 (the fund
  reservation) then WCF1 id=2 tick=680 images=[(215,7) lsn=2] carrying the house blocks; the
  settlement's own staged portions journal nothing (alloc_high stayed 1 across 99/144/99-block
  portions), so the earlier attribution to `apply_structure_portion`/`regional_mutation.rs:848`
  was wrong. Fix: the fence now observes the footprint's CONTENT - an FNV-1a digest over the
  blocks inside `bounds`, `None` when a covering chunk is unloaded (fail closed)
  (`crates/mc-net/src/settlement.rs:409`, `:571`, `:575`) - instead of the chunk's durable
  journal position; the trait method is `observe_footprint`, called at prepare
  (`script/storage/settlement.rs:1736`) and after each own portion commit (`:1896`); the adapter
  no longer takes a session handle (`server.rs`). A genuine edit inside the footprint still parks
  the build as `site_changed` (`settlement_tests.rs:1166`, `:1231`).
- live_after_real_fix: `.analysis/validation/20260914T002326-regression-76agjkm2` reaches
  `Reserved real materials for house_sm_1 (66d36952...)`, then
  `house_sm_1 committed (solaris:house_small) at revision 19.` and
  `regsville | small site_6_0_9af75e77 hamlet tier=hamlet pop=0 houses=1 ...`; the driver's
  commit and info matchers both fired and no `site_changed` pause appears anywhere in the run.
  The harness still reports failed only because the driver times out at the next stage.
- next_blocker: `settlement populate` never reaches `settled in regsville` - the resident spawn is
  refused and the plugin answers "The refused spawn left the site reservation free again."
  (resident/site-vertical path, tracked as blocked; not the fence).
- validation_latest_push: `run correctness` PASS
  `.analysis/validation/20260914T003603-correctness-72k40m62` (326.5 s).
- live_after_fix: `.analysis/validation/20260913T232942-regression-eov066g3` (pushed tree,
  seed 81) still ends at `house_sm_1 paused: site_changed.` - `fund` answers
  (`Reserved real materials for house_sm_1 (0c06d1b4...)`) and the next line is the pause,
  then the 615 s driver timeout. So the post-portion re-observe added in `44eea177` is
  downstream of the write that actually lands between `project` and `build`: the fence sits
  at the top of the stage-advance
  (`crates/mc-net/src/script/storage/settlement.rs:1805`) and pauses before any portion is
  applied in that command, so the re-observe never runs for the offending frame. Next:
  instrument which frame lands in the footprint chunk between `project` and `build`
  (`fund` reservation and/or `prepare_structure`, `script/storage/settlement.rs:1611`), then
  re-observe at the end of that path (or attribute the structure's own decisions) instead of
  only after a portion commit.
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
