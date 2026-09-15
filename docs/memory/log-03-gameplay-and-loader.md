# Archived checkpoint log — part 3 of 8

Chronological checkpoint history moved out of `docs/MEMORY.md` so the live cursor
stays small. **Not startup context** (see `AGENTS.md`); read it when a question is
about this era, not to learn the current state.

## Sections in this part

- Buildings buried in terrain: diagnosed, guarded, and the real fix dispatched
- Long-path world identity fixed (owner-relevant)
- Long-range mob wander (owner: "чтобы мир реально был живым и в движении")
- Mob spin near leaves fixed and measured (owner bug)
- Plugin ↔ C4 wiring landed; live combat blocked by one plugin bug
- Live re-verification on the gate-green build + the last gameplay gap
- Full L2 `correctness` gate PASSES on the whole settlement program
- C4 combat proven — five real executor bugs fixed
- Loader wire cutover closed (protocol 3 / schema 2 both sides)
- C4 execution landed (verified)
- Open regression carried into the next wave: Loader protocol cutover
- Other carried items
- Live probe of the default plugin set (owner request)
- Settlement overhaul program — waves
- Bounded region pregeneration CLI + login-burst flake closed
- Settlement contract C1 (inventory) — delegated slice complete, uncommitted
- GitHub release v0.0.6 republished with current binary
- v0.0.6 at d8ec969e — journal race test deterministic
- v0.0.6 at 5f72864d — recipe_book_add client kick fixed

---

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
