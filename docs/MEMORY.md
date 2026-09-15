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

- **Loader wire 3 / schema 2** (`solaris-loader`, uncommitted there): `PROTOCOL_VERSION = 3`, `INDEX_SCHEMA = 2`, content kinds `views`/`view_actions`/`world_previews`/`world_selection`, the eight schema-2 widget types with their bounds, the `solaris:loader/view` and `solaris:loader/view_action` wire-3 messages, the protocol-3 sound channel, and the view screen in `loader-platform-common`. Verified by my own forced run (`:loader-core:cleanTest … :forge:test`) — 94 tests, 0 failures, 0 skipped across 27 suites; the earlier all-up-to-date run proved nothing and was superseded.
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

## R1-A: worker production becomes real cargo (landed 2026-09-15, uncommitted)

**Why.** R0's overview can only be truthful about a warehouse if something really puts items there. The read-only R1 map showed the first blocking defect: `ItemLedger::extend_drops` only summed a per-item delta for the receipt, so `Harvest`, `CutTree`, `Mine` and `Fish` removed real blocks, rolled real canonical loot, and left it owned by nobody. Every later step of R1 — haul, warehouse stock, food, construction consumption — had nothing real to move.

**What landed.**

- **Capacity is decided before the world changes.** `ResidentWorld` gained `preview_break`, which computes the *same* canonical loot as the commit without touching the world (loot is a pure function of state, tool and the block's own seed), and `LiveResidentWorld` now shares one `break_loot` between preview and commit so the two can never disagree. The work loop previews, asks `drops_fit`, and only then breaks the block: a worker that cannot hold the loot leaves the field, tree or ore standing and reports `no_storage` instead of producing items nobody owns.
- **One cargo, real stack semantics.** `put_resident_item` now takes the item's own max stack size, merges only into *compatible* stacks (same id, no damage, no enchantments, no custom name or model) and fills fresh slots when merging cannot absorb the rest, so a deposit can no longer create an illegal stack or absorb loot into a tool. `deposit_drops` records in the receipt exactly what entered the cargo.
- **`Fish` was minting cod.** It added `RESIDENT_FISHING_CATCH` straight to the receipt per water column with no owner; the catch now goes through the same capacity check and cargo deposit, and the fixed canonical catch (rather than a loot roll) stays a recorded limitation.
- **A namespaced tool resolved to nothing.** `LiveResidentWorld::item_stack` built `minecraft:{item}`, so a work order naming `minecraft:iron_pickaxe` — the only form the Lua contract accepts — looked up `minecraft:minecraft:iron_pickaxe` and yielded no held tool. Tool-gated blocks then dropped nothing while the worker still counted the work unit: the resident mined ore and produced an empty receipt. `item_stack` now accepts a resource id or a bare path, and the new mine test fails without the fix.

**Evidence.** `harvest_deposits_the_real_crop_into_the_worker_cargo` (cargo holds the crop, the receipt never claims more than the cargo holds, and the same stacks come back through `query_owned_inventory{resident_carry}`); `a_full_worker_reports_no_storage_and_leaves_the_crop_standing` (pause `no_storage`, zero units, no positive change, crop still in the world); `mined_ore_reaches_the_worker_cargo`. Suites: `mc-net --lib resident` 57/0, `settlement` 68/0, `play::` 1789/0.

**Recorded gaps.** The block edit is still committed before the resident record batch, so blocks and receipt are not yet one journal decision (`REC-03`); that composite is the next checkpoint and reuses `WorldInventoryCommit`/`commit_owned_decision`. The shipped package still names category-like tools (`minecraft:hoe`, `minecraft:axe`, `minecraft:pickaxe`) that are not items, so its farm/forestry/mining jobs pause as `missing_tool`; naming a real tool belongs with the delivery checkpoint, where the production chain states which tool it expects. Fishing yields a fixed canonical catch, not a vanilla loot roll.

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

## R1-B: worker cargo reaches the settlement warehouse (core landed 2026-09-15, committed and pushed; plugin half NOT done)

**Outcome reached in core.** A `Haul` work order whose destination is a bound warehouse container really moves the worker's cargo into that container in one durable decision; a container that cannot take the cargo leaves every item with the worker and reports `no_storage`.

**What landed.**

- **The chest composite's actor is optional.** `SimulationCommand::CommitChest` now carries `actor_session: Option<SessionId>` and `player: Option<Box<ContainerPlayerPlan>>`; a deposit with no player participant is enqueued with `enqueue_with_fence(None, …)`, exactly like every other server-owned command, and the validator refuses the session-authored menu shape when its session or player plan is missing. `WarehouseTransferRequest` gained `player: Option<WarehousePlayerParticipant>` (one shape with an optional participant, not an actor enum).
- **One container half, factored.** `commit_container_half` (`crates/mc-net/src/play/session/transactions.rs`) owns the state-id fence → `commit_chests_conditionally` → state-id bump → `ChestSlots` publication for both the menu and the server-owned path; only the excluded actor and the refusal family stay caller-specific. The server-owned path keeps publishing to *every* viewer including the actor (verified by the pre-existing `server_owned_warehouse_deposit_appends_before_publishing_and_recovers_both`).
- **The worker's record rides the receipt.** `PreparedStorageBatch::validate_inventory_participant` now accepts a decision whose participants are the receipt and a non-empty `order` change, so the container's after-image and the worker's canonical record are durable together and replay together through the same `append_inventory_projection` / journal recovery the player's after-image uses.
- **Planning is canonical and all-or-nothing.** `plan_warehouse_deposit` (`crates/mc-net/src/play/owned_inventory.rs`) applies every step through `plan_owned_item_transfers`, so a deposit can never state a stack the planner would refuse; a container with no room answers `capacity`, an empty worker answers `insufficient_items`, and the two map to `no_storage` / `missing_input` respectively.
- **A haul is directed again.** `ScriptResidentWorkOrder::canonicalize` swapped `Haul { source, destination }` whenever `source > destination` in enum order (`PlayerInventory < Warehouse < ResidentEquipment < ResidentCarry`), which silently inverted `carry → warehouse` deposits *and* resident→resident hauls. The swap is deleted (`crates/mc-script/src/resident_order_operations.rs`); the fingerprint stays stable because both endpoints are named in the contract. The `Haul` arm also returned its step's move count instead of `already_done + moved`, reporting a watermark that could go backwards on resume; it is now consistent with every other work arm.
- **The plugin half is NOT done.** `../solaris-default-plugins/solaris-settlements`'s `hauling` job still names no warehouse destination, and its job table still names `minecraft:hoe`/`axe`/`pickaxe`, which are not items, so farm/forestry/mining still pause as `missing_tool`. No git operation was performed there.

**Review follow-ups closed in the same checkpoint.**

- **One deposit tail.** `InventoryRuntime::commit_prepared_deposit` now owns everything after a caller has prepared its batch (encode the receipt → commit the container → project the ledger frame → acknowledge the decision); the player's deposit and the worker's deposit share it, so the append/projection ordering ADR 0004 exists to protect has one implementation, not two.
- **Participant pairing is enforced.** The validator admits a chest command only when `actor_session.is_some() == player.is_some()`, and `commit_server_owned` asserts the same against the durable state it holds: a plan that moves a player's items can no longer be committed with the player fence silently skipped.
- **A covered stack no longer stalls the haul.** `plan_warehouse_deposit` advances to the next source slot when the container cannot take the current stack, instead of ending the plan: a worker's cargo whose first stack fits nowhere but whose later stack does deposits the later one. Only a cargo that fits nowhere reports `capacity`/`no_storage`. Guarded by `a_haul_deposits_past_a_stack_the_container_refuses`, which fails against the previous `break` (verified by reverting the one token and rerunning the test).

**Evidence (all green on this tree).** `cargo test -p mc-net --lib` 2206/0; `cargo test -p mc-script` 129/0; `harness run fmt` PASS; `harness run code-health` PASS. New coverage: `worker_haul_deposits_its_cargo_into_the_bound_warehouse` (container merged to the exact count, carry emptied, one decision carrying the receipt, read back through a warehouse query), `a_full_container_leaves_the_cargo_with_the_worker` (no storage, cargo intact, no decision spent, world never asked), `a_replayed_haul_deposits_once` (`REC-02` shape: one real move, one decision), `a_haul_deposits_past_a_stack_the_container_refuses` (mixed cargo progresses past a stack the container refuses). The L2 `correctness` profile was NOT run this checkpoint; the pre-existing load-sensitive red it is known for (`mc-test-harness --test settlement_pause_repro`) is unchanged and unexplained by this work.

**Still open.** Plugin hauling destination and real tool ids (above); warehouse *withdrawal* by a worker (the same principal in the other direction); reserved-stock withdrawal for construction (BUILD-02/03); the construction composite (`REC-03`).

## R1-B plan (superseded by the section above, kept for the design record)

**Outcome.** A `Haul` work order whose destination is a bound warehouse really moves the worker's cargo into that container, in one durable decision, and a full container leaves the cargo with the worker and reports `no_storage`.

**Why the existing path cannot do it.** `ScriptResidentWorkOrder::Haul` already carries `ScriptInventoryEndpoint` values, which include `Warehouse { handle }`, so no new DTO is needed. The executor refuses it: `resident_endpoint_len` cannot resolve a warehouse, so `haul_resident_items` returns `no_storage` (`crates/mc-net/src/script/storage/resident_order_execution.rs:630-660`). The write side is player-shaped end to end:

- `InventoryRuntime::commit_warehouse_transfer` (`crates/mc-net/src/script/storage/world_inventory.rs:748`) accepts only `Warehouse` beside the *same actor's* `PlayerInventory`, requires `sessions.warehouse_transfer_actor(actor_id)` (a live session, `:836`), and plans over a 46-slot `PlayerInventory`.
- `SettlementWorld::commit_warehouse_transfer` → `SimulationHandle::commit_warehouse_transfer` (`crates/mc-net/src/play/simulation.rs:2907`) enqueues `SimulationCommand::CommitChest` through `self.for_session(actor_id)` — a **player session is mandatory**.
- The owner's job (`crates/mc-net/src/play/simulation/regional_mutation.rs:679-760`) already has a server-owned branch (`ChestTransaction::commit_server_owned`, `crates/mc-net/src/play/session/transactions.rs:113`), but that transaction is still session-scoped: it locks and mutates `player_state`, and `prepare_chest_transaction` returns `None` when the session does not exist (`container_views.rs:501-521`).

So the missing capability is a **container + receipt commit with no player participant**, and the resident's canonical record (which lives in plugin storage, not in the world) becomes the second participant through the encoded receipt, exactly as the player's inventory recovery does today.

**Design decision, taken from the code before writing any of it.**

- **No new command variant and no fake actor.** `SimulationCommand::CommitChest` already documents that a `plugin_receipt` "makes the command server-owned: it drops the open-menu fence" (`simulation.rs:464-478`), and the repo's precedent for a session-less commit is the *same* command with `actor_session: None` through `enqueue_with_fence(None, …)` (`apply_server_owned_block_edits`, `simulation.rs:2372`). `actor_session` therefore becomes `Option<SessionId>`; its blast radius was counted, not assumed: the only consumers are `ChestCommitRequest::actor_session` (`simulation.rs:1377`, destructured at `:4707`), the response dispatcher at `:6497-6517`, and the regional job construction (`regional_mutation.rs:346`) — every other `CommitChest` site matches with `..` (`:800`, `:951`, `:1125`, `:4015`, `:5802`). Synthesizing a neutral `ContainerPlayerPlan` is rejected: `ChestTransaction::commit_server_owned` locks `player_persistence` and returns `StalePlayer` unless the live player's inventory and carried item match, and `prepare_chest_transaction` needs a real session (`container_views.rs:501-521`) — a fabricated participant would be the second authority `AGENTS.md` forbids.
- **Two facts that make the worker path fail-closed by construction.** `commit_chest_command` refuses any request carrying `plugin_receipt` ("it only ever runs in the journaled regional lane", `simulation.rs:4705-4713`), so a worker commit can never take the menu path; and `CommitChest { plugin_receipt: Some(_) }` is already classified as a journaled regional block edit (`simulation.rs:4013-4019`), so the journal decision and the receipt ride are inherited rather than added.
- **One container half, factored.** The state-id fence → `commit_chests_conditionally` → post-decision `VisibilityDispatch` publication must become one helper shared by the player and worker cases, with only the actor fence differing; re-implementing either half in a worker-only path is exactly what would let the ADR's ordered-authority guarantee drift.
- **Fence rule for the second participant (ADR 0004).** A player endpoint's post-commit revision *is* the journal decision id, forced by `PlayerInventoryRecovery::recover(root, decision_id)` (`world_inventory.rs:536-540`); the resident record rides the same prepared batch through `append_inventory_projection`, so it inherits the same forcing and must not number revisions independently — otherwise replay skips the write. The `Transfer` result keeps naming the **container** fence, never the actor's.

**The cut (one slice).**

1. `actor_session: Option<SessionId>` on `CommitChest` + a session-less enqueue on `SimulationHandle` (no `for_session`), reusing `enqueue_with_fence(None, …)`.
2. Inside the existing chest job and transaction: skip the player lock/fence only when no actor is present, keep the container half shared, keep the journal stamping and the receipt ride on the container's own decision, and reuse the existing `RegionalWarehouseDecision` handling for replay.
3. `InventoryRuntime`: when a haul endpoint is `Warehouse`, resolve the binding (`resolve_warehouse_container`), plan the move all-or-nothing against the container and the resident's canonical slots, and commit through the new command with the resident record change inside the prepared batch. Fence: the container's binding revision and the resident record revision (the same pair the read path already returns).
4. Tests: cargo → container with the exact counts, empty carry, container snapshot read back through `query_owned_inventory{warehouse}`; full container ⇒ `no_storage` with the cargo untouched; replay of the same operation ⇒ one transfer (REC-02 shape).
5. ADR 0004 gains the worker-principal variant of its "Warehouse inventory transfer" section (the second participant is a plugin-owned record, not a player session).
6. Plugin (`../solaris-default-plugins/solaris-settlements`): the `hauling` job gets a warehouse destination, and the job table's tool ids become real items — `minecraft:hoe`/`axe`/`pickaxe` are not items, so `gear_has` never matches and farm/forestry/mining pause as `missing_tool` today (`main.lua:170-181`).

**Not in this slice:** NPC work that *withdraws* from the warehouse (needs the same principal in the other direction), reserved-stock physical withdrawal for construction (BUILD-02/03), and the construction composite (REC-03).

## RESUME CURSOR for the next session (rewritten 2026-09-15)

**Read this first, then "R0 of the settlements overhaul", "R1-A: worker production becomes real cargo" and "R1-B: worker cargo reaches the settlement warehouse" above.**

### Where the work stands

- **R0 client half: landed, verified at the code level.** `solaris-settlements` declares one schema-2 `[client]` bundle; core reads each views bundle's verified artifact index and declares its screen kinds at bind, so a key-driven `view_request{settlement}` resolves to `solaris-settlements`. The Loader repo's `LoaderShippedSettlementsTest` activates the shipped bytes through real Java code. **Acceptance runs are deferred by the owner**; the real-client profiles need credentials this machine does not have.
- **R1-A: landed.** Worker output is real cargo.
- **R1-B core: landed.** A haul into a bound warehouse deposits the worker's cargo in one durable decision, refusals leave the cargo with the worker, and replay deposits once. Suites on this tree: `mc-net --lib` 2206/0, `mc-script` 129/0, `harness run fmt` PASS, `harness run code-health` PASS. The L2 `correctness` profile was not rerun; the known load-sensitive red in `mc-test-harness --test settlement_pause_repro` is untouched by this work.

### Next action

1. **Plugin half of R1-B** (never started): `../solaris-default-plugins/solaris-settlements` — give the `hauling` job a bound-warehouse destination, and replace the category-like tool ids in the job table (`minecraft:hoe`/`axe`/`pickaxe` are not items, so `gear_has` never matches and farm/forestry/mining pause as `missing_tool`) with the real tools the production chain expects. That repository is uncommitted and load-bearing; do not run git operations there without the owner.
2. Then the R1-B items deliberately left out: worker *withdrawal* from a warehouse, reserved-stock withdrawal for construction (BUILD-02/03), and the construction composite (REC-03).

### Machine rules for whoever continues

- Bounded heavy runs only: `CARGO_BUILD_JOBS=2`, one gate at a time, **no `nice`** (it starves the Lua host's 10 ms/50 ms wall budget, `crates/mc-script/src/lua.rs:53`), nothing else heavy in flight. An unbounded workspace run already OOM-killed this machine once.
- Never widen a test bound or add a retry to force green; if a gate is red, record it with its receipt.
- No git operations in `../solaris-default-plugins`; its uncommitted package state is load-bearing.
