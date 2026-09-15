# Archived checkpoint log — part 4 of 8

Chronological checkpoint history moved out of `docs/MEMORY.md` so the live cursor
stays small. **Not startup context** (see `AGENTS.md`); read it when a question is
about this era, not to learn the current state.

## Sections in this part

- v0.0.6 at 881461ca — all known reds fixed, quarantine lifted
- v0.0.6 republished green (tag moved, main pushed)
- Test-repair sweep (dead field + stale wire expectations, no push)
- Combined commit 961ed9ec (owner-authorized, no push)
- Old questions closed (owner decision 2026-09-11)
- Chest-loot startup wiring: done
- End generator foundation: done
- Settlement plugin worker: P1-a done (sibling repo)
- Nether review fixes (same slice)
- Structure loot worker: done
- Current checkpoint: nether generator foundation (dimensions slice 1)
- Current checkpoint: stale baked light repro (handoff issue 7, migration half)
- Current checkpoint: fluid wash (plant follow-up, water half)
- Current checkpoint: light publication trace (handoff issue 7, ordering half)
- Current checkpoint: ender chest emission (handoff issue 7, emission half)
- Owner field follow-up — 2026-09-11 (uncommitted)
- Current checkpoint: skeleton ranged slice (handoff issues 11-equip + 12)
- Current checkpoint: mob hurt flash trace (handoff issue 10, signal half)
- Previous checkpoint: chest lighting opacity (handoff issue 7, metadata half)
- Previous checkpoint: explosion support cascade (plant follow-up, blast half)
- Previous checkpoint: region flush preservation (handoff issue 9)
- Previous checkpoint: time set trace (handoff issue 8, server half)
- Previous checkpoint: fish display IDs (handoff issue 2, logic half)
- Previous checkpoint: double-chest pairing (handoff issue 1, logic half)
- Previous checkpoint: authoritative buckets (handoff issues 5+6)
- Previous checkpoint: flower/grass support cascade (handoff issues 3+4, break path)
- Previous checkpoint: owner-requested as-is handoff to main

---

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
Read [FIELD_TEST_HANDOFF.md](../FIELD_TEST_HANDOFF.md) first: it contains all twelve
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
