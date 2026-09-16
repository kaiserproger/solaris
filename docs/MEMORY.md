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

## RESUME CURSOR for the next session (rewritten 2026-09-16, at the owner's stop)

**Start here. The owner stopped the session mid-slice and committed the tree as it
stands.** Exactly two things are unfinished, and both are named in the section
below: the S2 zones slice (WIT and adapter conversion are in, the answer mapping
and its tests are not) and the pre-existing L2 red. Everything else in the tree is
verified by the gates listed where it landed.

### Where the work stands

**Read this first, then "R0 of the settlements overhaul", "R1-A: worker production becomes real cargo", "R1-B: worker cargo reaches the settlement warehouse", and the CP-001 closeout above.**

### Where the work stands

- **R0 client half: landed, verified at the code level; acceptance deferred by the owner.**
- **R1-A: landed** — worker output is real cargo.
- **R1-B: core landed and committed (`84de0bf9`, `4bde4074`); plugin half landed in the working tree of `../solaris-default-plugins`, uncommitted.**
- **CP-001 closed at the order level, not at the chest level** (see "Not reached" above).
- **CP-002 closed:** the load-sensitive settlement red was reproduced under the gate as a plugin that stopped answering (`Unknown command`), and the process's worker budget is now a bounded, configurable number (`[chunk_pipeline] worker_threads`, `playable.toml` = 2, in-process test servers bounded(2)); `correctness` passed three consecutive runs afterwards.
- **CP-003's warehouse->worker half is closed** (see the closeout above): `Haul` is directed by both endpoints, carries an optional `item`, and the package issues named stock out of the settlement's bound warehouse with `/settlement issue`. The plan item's **second half is still owed**: a bounded world-container/village-stock source through the same container owner (no synthetic `DurableStructure`, no second binding authority), plus its checks for an allowed source, a foreign claim, an unloaded/unknown container and the untouched original village.
- **The WASM plugin-host migration is the active route** (owner task; the live plan is the WASM re-edition at `/home/user/Загрузки/SOLARIS_LONG_TERM_PLAN_WASM.md`, which replaces the older long-term plan). **P0 is closed** (safe host, bounded transfer, contract refusal, baseline, hostile-guest evidence). **P1 is done.** **P2 is done**: package discovery, `strict`/`expected`/grants, `--check` without game-side effects, the host runtime, and — since the closeout above — the composition root, so a `runtime = "wasm"` deployment really runs in the server. **P4's startup half is done** (`configure` -> validated contribution -> `startup_rules::apply` -> world contract); its Loader/client half is not. **P3 is the next stage.** The long-term plan's CP-004 is the deferred route, not the next one.
- **`local://composition-root-map.md`** is the reconnaissance behind the wiring: the Luau startup path step by step, every consumer of the prepared-plugin value, what the component path provides, the `RulePlan` vs `GameplayRules` correspondence, the `[plugins]` surface, dependency facts, and the decisions that were open. `local://serve-component-integration.md` is the wiring design note the checkpoint followed.

### Next action

**P3: move the game operations the shipped packages actually use onto the WIT
contract, with their existing permissions, ownership, refusals and durability.**

P0, P1, P2 and the startup half of P4 are done (see the two closeouts above); the
component host now runs in the server. What P3 moves is *existing* surface, not new
gameplay: the P0 API matrix (`docs/MEMORY.md`, "Not done yet (P1 onward)") counted
61 registered `solaris.*` functions — 32 direct command pushers, 29 operation
variants, 20 with no first-party consumer. Take them in the order the shipped
packages need them, and for each one name its caller, capability/grant, DTO, owner
commit, result/durability path and package, exactly as the plan's P3 work item
says. Do not answer `unsupported` where a working API is being moved.

Rules that bind the slice: every command goes through the existing validators and
owners (a WIT command is not a second authority); a result command arrives as the
next delivery, never by recursive re-entry; `request_id` correlates an answer and
does not replace the durable `operation_id`; `ScriptCommitEventOutbox`, receipts,
CAS, inventory fences and `DurabilityUnknown` keep their current semantics; no
second journal, outbox or router. The host's own bound applies to every new DTO —
`PluginLimits::hostcall_bytes` is a budget on the *whole* answer, so a moved
operation whose DTO can name more than the contract admits must be bounded at the
WIT level, not after lifting.

**Still open from earlier checkpoints, in priority order after P3:** the CP-003
world-container/village-stock source; the Loader/client half of P4 (client bundles,
ore profile, settlement plan have no WIT record yet, and the world contract
currently records the no-plugin values for a component deployment); P5 precommit
hooks; P6 reload; P7 the ported packages and fixture centralization; P8 the Luau
cutover. The L2 red named above (the settlement wall-slice residual) belongs to
whoever takes the Lua host's budget metric — it is not this route's, and no gate
result may be reported green while it stands.

**P3's order is now evidence, not preference.** `local://p3-operation-matrix.md`
(40 KB) enumerates all 61 registrations with `path:line`, corrects the earlier
count (40 operations have a first-party consumer, 21 do not, 3 have no caller at
all — the previous "20 with no first-party consumer" was off by one), and gives
each consumer operation its caller, capability, DTO bounds, owner/commit path and
answer path. Its findings that shape the work:

- **Only one slice needs `mc-net` at all** (declarative client views, S-V): the
  grant there is a Loader-manifest content kind, not a plugin capability, and the
  component package model has to express a client bundle first. Every other slice
  is contract-shaped work inside `mc-script`/`mc-plugin-host`, because the DTOs,
  validators and owners already exist.
- **Landed: `teleport_player`** (`solaris-essentials:92`, matrix slice S3) — the first
  real P3 vertical, chosen because it exercises the two things later slices inherit:
  a mutation that must keep the server's refusals, and a typed answer delivered to
  the asking plugin by its own request id. `teleport-player` names the *session*
  (a teleport is an effect on one live connection), its answer is
  `player-teleport-answered` with `committed` or `refused(player-unavailable |
  teleport-pending | runtime-unavailable)`, and the adapter builds exactly the DTO
  the Lua path builds (`ScriptPosition::try_new` + `ScriptPlayerId::new(session)` +
  `ScriptPlayerTeleportRequest::try_new`, compared field-for-field), so the
  semantics are the server's rather than a paraphrase. Evidence:
  `crates/mc-plugin-host/tests/player_operations.rs` (6 cases: the converted DTO,
  the capability refusal by name, a full deployment losing its route instead of
  teleporting, and the bounds), the crate at 55 tests in 10 binaries, clippy clean,
  `harness run fmt` and `code-health` PASS.
- **Landed: `storage_batch_cas` + `operation_status`** (matrix slice S1,
  `solaris-settlements:1325,1641,1822` — the package's only atomic durable-write
  path) with the **correlated operation-answer envelope every later slice reuses**:
  the plugin's own request id, the durable `operation-id` the package re-probes by,
  and the server's own refusal vocabulary (`invalid-request`, `forbidden`,
  `stale-revision`, `not-found`, `unloaded`, `operation-conflict`, …) rather than one
  flattened "refused". The WIT mirrors the Lua registration field for field
  (`request_id`, `operation_id`, `mutations` — the plugin names both ids), and the
  batch's bounds are the DTO's own (16 distinct keys, 128-byte keys, 4096-byte
  values) enforced by `validate_storage_mutations` through the adapter. Evidence:
  `crates/mc-plugin-host/tests/storage_operations.rs` (6 cases; two deliberate
  mutations of the production mapping each failed the case that names the property),
  the crate at 61 tests in 12 binaries, clippy clean, fmt and code-health PASS.
- **Partially landed: S2 (zones)** — the owner stopped the session mid-slice. What is
  in the tree is the WIT (`zone-upsert`, `zone-protected-upsert`, `zone-remove` and
  the answer event) and the adapter conversion; what is **not** in the tree is the
  `host.rs` answer mapping and `tests/zone_operations.rs`, so S2's acceptance is
  unverified and the slice must be finished before anything is claimed about it.
  The contract decision it settled is recorded in the WIT comments: the server's
  zone owner answers per zone with one bit (`ScriptEventKind::ZoneCommandResult {
  zone_id, accepted }`), the finer `ZoneAdapterError`/`ZoneCapacity` vocabulary is
  `pub(crate)` in `mc-net` and never crosses the boundary, so the answer is
  `applied | refused` and carries the **zone id, not a request id** — the contract
  has two correlation styles on purpose, and the reason is "the server's answer is
  zone-keyed", not convenience.
- **The next slices, in the matrix's order:** S4 menus, S5 the
  inventory+storage transaction, S6 owned inventory, S7–S11 the settlements long
  tail, S12 config+timers (the only slice that changes `host.wit` imports — its
  `schedule_timer`/`cancel_timer` return values synchronously in-VM and have no
  import equivalent, so it carries a real contract decision), and S-V last as the
  expensive one.
- **P3 slices serialize on purpose**: they all add a member to `variant command`,
  a member to `variant event` and an arm to the one adapter, so two writers would
  collide on the same three files. The parallelism in this stage is a writer plus a
  read-only reconnaissance, not two writers.
- **Group (b)/(c) operations are out of P3's scope** by the plan's own test: no
  shipped package calls them, so moving them cannot be justified by a consumer.

### Machine rules for whoever continues

- Bounded heavy runs only: `CARGO_BUILD_JOBS=2`, one gate at a time, **no `nice`** (it starves the Lua host's 10 ms/50 ms wall budget, `crates/mc-script/src/lua.rs:53`), nothing else heavy in flight. An unbounded workspace run already OOM-killed this machine once.
- This route's crate is fmt- and code-health-clean as of the P0 closeout (`cargo fmt --all` was needed for the first time on it, and `ScriptBatchSubmissionError` needed `#[non_exhaustive]`). Keep it that way per checkpoint: `rustfmt --edition 2024` your own files, and re-run `harness run fmt` + `harness run code-health` rather than a workspace test sweep.
- Never widen a test bound or add a retry to force green; if a gate is red, record it with its receipt.
- Sibling revisions this cursor depends on: `../solaris-loader` `0972926` (committed; the loader had nothing further to commit at the stop, only its untracked local-only `.cache/` and `config/fml.toml`, which stay unstaged), and `../solaris-default-plugins`, whose `solaris-settlements` package was committed at the stop.

### What the next session does first

1. **Finish S2 (zones).** The WIT and the adapter conversion are already in: add the
   `host.rs` mapping from `ScriptEventKind::ZoneCommandResult { zone_id, accepted }`
   to the new `zone-*` answer event, then write `tests/zone_operations.rs` with the
   acceptance its two predecessors used — exact DTO equality for all three commands,
   the capability refusal by name, a removal the owner refused reporting `refused`
   and not an invented reason, and the answer reaching only the plugin that asked.
   Then the usual per-file `rustfmt --edition 2024` plus
   `cargo clippy -p mc-plugin-host --all-targets -- -D warnings`.
2. **Then S4, S5, S6–S11, S12, S-V in the matrix's order** (see the P3 section above
   for what each needs and which one is the expensive one).
3. **The L2 red is not this route's to fix but blocks any green claim**:
   `settlement_pause_repro` fails under a full workspace run and passes alone. Its
   owner is the Lua host's wall-clock budget metric (CP-002 named it). Do not report
   a gate green while it stands, and do not widen a test bound to make it pass.
