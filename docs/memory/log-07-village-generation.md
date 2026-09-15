# Archived checkpoint log — part 7 of 8

Chronological checkpoint history moved out of `docs/MEMORY.md` so the live cursor
stays small. **Not startup context** (see `AGENTS.md`); read it when a question is
about this era, not to learn the current state.

## Sections in this part

- Vanilla feature executor A2 — trees (partial, internal only — uncommitted 2026-09-14)
- Core vanilla village generation activated (uncommitted 2026-09-14)
- Vanilla-faithful village solver and the decor lane (landed 2026-09-14)

---

## Vanilla feature executor A2 — trees (partial, internal only — uncommitted 2026-09-14)

`minecraft:tree` implemented for the four village placer pairs (straight+blob oak,
straight+spruce, straight+pine, forking+acacia), so the WHOLE village decor closure
(19 pool elements / 13 distinct features) now loads and places. Loader gained
`ConfiguredFeatureKind::Tree(Box<TreeSpec>)` (trunk/foliage providers and placers,
`minimum_size` with the codec defaults, `ignore_vines`, `below_trunk_provider`
defaulting to `PLACE_BELOW_OVERWORLD_TRUNKS`) and `IntProviderSpec::Uniform`; a
non-empty decorators list, an unsupported placer/size, a `root_placer` and a
`list_pool_element` all fail closed naming type id + referring entry. Executor
`crates/mc-worldgen/src/vanilla_features/tree.rs` is transcribed from the decompiled
`TreeFeature`/`TrunkPlacer`/`FoliagePlacer` bodies, including `updateLeaves`' BFS,
whose final leaf `distance` depends on Java `HashSet` iteration order — emulated
explicitly (`java_hash_set`), because Rust's randomised set order produced different
distances. `StructureTemplate.updateShapeAtEdge` is deliberately not reproduced: every
reachable tree block returns its state unchanged there, documented in the module header.

All seven A1 review findings closed with named tests: nested `rule_based` resolves the
existing state, the `DOUBLE_MULTIPLIER` comment states the exact-2^-53-through-`f32`
requirement, non-string property values and `list_pool_element` fail closed, offsets are
bounded to vanilla's ±16, pool `weight` and `block_column.prioritize_tip` are required.
Proof strength raised: the live proof now places all 13 reachable features and asserts
the exact vanilla block sets for all four trees, on top of the synthetic fixture tests.

Evidence (reproduced by Main): `cargo test -p mc-data --lib vanilla_feature_closure`
18 passed / 0 failed; `cargo test -p mc-worldgen --lib vanilla_features` 22 passed /
0 failed; `SOLARIS_CONTENT_CACHE=/tmp/jdk-cold2 cargo test -p mc-worldgen --lib
village_decor_closure_places_from_the_real_cache -- --nocapture` 1 passed with
coordinates for every feature and `deterministic replay matched for all 13 features`.

Still partial/internal: tests-only reachability, no activation, `PlainsVillagePrototype`
intact, `WORLDGEN_REVISION` still 21, no `docs/MEMORY.md` self-writes by agents.
Review of A2: **pass** (independent, read-only, `ImportProgress`). The reviewer
transcribed the whole tree path into Java against the real `LegacyRandomSource` from
the bundled jar and real `java.util.HashSet` and reproduced all four tree fixtures
exactly (oak 59/59, spruce 62/62, pine 98/98, acacia 93/93, leaf `distance` included),
and mirrored the `JavaHashSet` order against Java's across 46,540 comparisons. It also
confirmed from the decompile that `StructureTemplate.updateShapeAtEdge` cannot write for
reachable tree blocks, and closed out all six prior findings and the new fail-closed
entry kinds. Two items kept open and assigned before B: the `JavaHashSet` emulation
models `HashMap` only up to the treeify boundary (`tree.rs:744-812`, guard at :770) so a
large enough per-distance set could silently diverge in a RELEASE build — it must fail
loudly there instead of relying on `debug_assert`; and two coverage gaps (tree
fail-closed tests assert the type id but not the referring entry; the clipping test
asserts only `assert_ne!`/`len <` for one seed per tree). Reviewer ran
`cargo test -p mc-data --lib vanilla_feature_closure` 18/0,
`cargo test -p mc-worldgen --lib vanilla_features` 22/0, the live proof (13 features,
four trees with verified constants, deterministic replay), `run fmt` PASS
`.analysis/validation/20260914T064428-fmt-8ndbobhr`, and clippy clean.
Next: checkpoint B — village assembly (structure sets and their biome tags, jigsaw
placement and template pools from the cache, `feature_pool_element` calling this layer,
per-position `RuleProcessor` seeding via the now-read `Mth.getSeed`), activation,
prototype deletion and the worldgen identity bump.

## Core vanilla village generation activated (uncommitted 2026-09-14)

Core now generates the five vanilla village structures under the default
`settlement_profile = "vanilla"`, and only there: `village_plan_source_for_startup`
(`crates/mc-server/src/main.rs`) attaches a `VillagePlanSource` only when no Luau
settlement plan is deployed and the profile is `vanilla`, so a plugin plan keeps
settlement ownership and the retained `plains_village_prototype` profile keeps its
own composite without double-placing. The
`settlement_profile_vanilla_generates_no_villages` warning is retired; the
terrain-adaptation analogue notice is the one a village world carries.
`WORLDGEN_REVISION` is 22 (revision-21 worlds are refused with the fresh-`world_dir`
message), and `docs/VILLAGE_GENERATION.md` documents the activated semantics,
including the multi-village-per-chunk rule and the still-unresolved prototype
removal.

Engine shape. `village/plan_source.rs` is the one lookup both consumers read:
`plans_for_chunk`/`plans_for_column` return a `VillagePlanSet` (every plan whose
region reaches the chunk, ascending start-chunk order, no cache, no second source of
truth), `VillagePlanSet::adjusted_columns` sums the plans' beard contributions and
rounds once the way `Beardifier` sums every structure a chunk references, and
`write_pieces` places each plan clipped to the chunk. The source is data-only: it
holds the closure, the block registry, the block tag index and the biome tag index,
and takes the terrain's plan-free biome per lookup, so it can never call back into
the generator that holds it (`TerrainGenerator::base_biome` is the plan-free biome
accessor; `diagnostic_sample` would have recursed). `VillagePlanSource::new` is the
only constructor and validates every closure piece under every rotation against two
probe world states (air, water) before handing the source out, which is what makes
the `expect` in `TerrainGenerator::apply_village_pieces` an invariant rather than a
hope. `vanilla_features::CacheTags` wraps the already-loaded `TagsData` plus
`VanillaData` and implements both tag traits, so no second tag authority exists.

Two fidelity defects found by the live activated-path proof, both fixed and pinned:

- **`GravityProcessor` added the world `y`.** `PieceBlock` now carries `template_y`
  (vanilla's `originalBlockInfo.pos().getY()`, the template-local position that
  `StructureTemplate.processBlockInfos` passes as the *original* while the world
  position is the *processed* one), and `Processor::Gravity` adds that. A scratch
  runner over the real 26.1.2 classes with the two readings made distinct (local
  `y = 0` and `5`, both at world `y = 70`) printed `y=64` and `y=69` — height +
  local `y`, never `+` world `y`. Before the fix every `terrain_matching` village
  block was flung to `height + piece.y`.
- **Processor lists came from the wrong pool element.** A template id can appear in
  several pools with different processor lists or projections (592 village pool
  elements, 478 distinct templates, 91 shared, four live conflicts among the
  terminators: `plains/terminators` vs `savanna`/`snowy`/`taiga`, all
  `terrain_matching` with four different lists). `PlacedPiece` now carries the
  placing element's `owner`, `processors` and `legacy` from the solver, and
  `plan_from` uses them; the old first-match-by-template-id lookup is deleted.

Evidence. `cargo test -p mc-worldgen --lib` 250 passed / 0 failed / 5 ignored;
`cargo test -p mc-worldgen --lib -- village_processors` 19/0 (18 + the new
`gravity_uses_the_template_y_not_the_world_y`, which pins the measured reference
numbers); `cargo test -p mc-server --bin mc-server -- structure_rules` 4/0; the live
proof `live_activated_path_places_a_vanilla_village` prints
`seed 4242, village at chunk (427, 0), 9 pieces, 16 junctions, 5 beard pieces,
region 6812,59,-20..6853,86,24, 52 village blocks, 5 columns moved` and asserts
deterministic replay through the activated path; `run fmt` PASS
`.analysis/validation/20260914T092746-fmt-at4crltn`, `run code-health` PASS
`.analysis/validation/20260914T092238-code-health-elgo627a`, clippy clean for
`mc-worldgen` + `mc-server` with `--all-targets`. Base tree `8414853e`.

Unresolved, stated rather than approximated:

- **Village decor is not placed yet.** The `feature_pool_element` entries the pools
  reach (13 features, executor done and live-proved) are skipped by the growth loop,
  which only accepts piece elements, so villages generate buildings, streets and the
  terrain analogue without their decorative features. Next village item.
- **The prototype/composite is not removable yet.** A deployed Luau settlement plan
  materializes its buildings through `StructureRules::plains_village_prototype`, so
  deleting the prototype and its three-template composite would delete the plugin
  settlement route with it. Removal needs a plugin-side placement mechanism first.
- `tests::deployed_sibling_plugins_prepare_runtime_and_worldgen_profiles` fails
  because the sibling `../solaris-default-plugins` checkout lacks the
  `solaris-settlements` package it expects (pre-existing at HEAD; owner-blocked).
- `StructureSpec.spawn_overrides`/`step` are parsed and carried but consumed by
  nothing; piece entity/villager markers are not exposed through `structures.rs`;
  the weighted-roll random source for the structure pick is still the engine's
  per-chunk draw rather than a confirmed `ChunkGeneratorStructureState` reproduction.
- The terrain analogue remains the owner-approved column-height divergence, not
  vanilla density parity.

**Owner decision on the decor gap (2026-09-14, after activation landed).** The
activated path places buildings, streets and the terrain analogue but not the
`feature_pool_element` decor (13 features; executor done and live-proved, lane not
wired). Asked whether to keep the default profile on the incomplete village, roll
the activation back until decor lands, or hide it behind a new opt-in profile, the
owner chose to **keep the default with the decor gap and make the decor lane the
next checkpoint**. So `settlement_profile = "vanilla"` stays the activated default,
`WORLDGEN_REVISION` stays 22, and `crates/mc-worldgen/src/village/mod.rs` plus
`docs/VILLAGE_GENERATION.md` name the gap explicitly rather than implying the
decor lane is landed. The commit that carries the activation (`884857cb`) also
carries the rest of the session's uncommitted work — the managed content import,
the warehouse endpoint work in `mc-net`, the harness capture, and `Cargo.lock`
(which changed for a concrete reason: `mc-server` gained `reqwest`, `sha1`, `zip`,
`thiserror` and the `mc-test-harness` dependency) — so a revert of that commit
drops more than the village activation.

**Independent checkpoint review of the activation (2026-09-14): verdict `changes`.**
The reviewer read the working tree against vanilla and found the assembled village
geometry is **not yet vanilla's**; the fixes below are the next checkpoint and each
one changes generated terrain, so the activation needs another
`WORLDGEN_REVISION` bump (23) when they land.

- **blocker — source jigsaw position and faces are not rotated** (`solver.rs:534-539`).
  Vanilla iterates `sourceElement.getShuffledJigsawBlocks(manager, sourceBoxPosition,
  sourceRotation, random)` -> `StructureTemplate.getJigsaws(position, rotation)`, whose
  entries are `calculateRelativePosition(settings, local).offset(position)` with
  `state.rotate(rotation)` (`StructureTemplate.java:201-219`). `grow` uses the raw
  template-local position and the unrotated `front`, so `target_jigsaw_pos`
  (= pos + front) drives `attach_inside`, the target's placement base and the
  `free_height` column from a jigsaw that is not where the piece actually placed it,
  and `can_attach` compares unrotated faces on both sides while vanilla compares both
  rotated by their own piece rotations (`JigsawBlock.canAttach`). Fires on every
  rotation except `None`. `anchor_origin` already rotates, so the module contradicts
  itself, and no test covers a rotated source attachment.
- **major — target pool and fallback candidate lists are not shuffled**
  (`solver.rs:556-563`). Vanilla uses `targetPool.value().getShuffledTemplates(random)`
  then `fallback.value().getShuffledTemplates(random)`
  (`JigsawPlacement.java:351-356`); `flat_elements` is right for `pick_element` only.
  Besides choosing a different first-fitting template, the two missing shuffles consume
  no draws, so every later draw (`Rotation::get_shuffled`, `shuffled_jigsaws`) comes
  from a stream vanilla has already advanced by `len(pool) + len(fallback) - 2` per
  source jigsaw.
- **major — `use_expansion_hack`'s `expandTo` is not applied** (`solver.rs:584-600`).
  All five village structures set the flag; vanilla raises the target box to
  `max(expandTo + 1, ySpan)` before the free-space test, before the piece is created
  and therefore before the RIGID piece's `BeardContribution`
  (`JigsawPlacement.java:368-406`). The engine computes the hack box and discards it.
- **major — the source-jigsaw loop is not continued, and the inside free shape is
  never grown** (`solver.rs:636-660, 717`). Vanilla's `continue label129` abandons the
  remaining candidates for that jigsaw after an attachment; `continue 'candidates` keeps
  scanning them, so a second piece can attach to the same jigsaw. And the grown shape is
  written back only when the target attached outside the source, while vanilla writes it
  to whichever `childrenFree` it used, so several attachments inside one source piece
  may overlap where vanilla rejects them.
- **minor — non-rigid `ground_level_delta` must be 1, not 0** (`solver.rs:662-666`);
  `StructurePoolElement.getGroundLevelDelta()` returns 1 and it feeds the second
  junction's `ground_y`, i.e. the analogue's `BeardJunction.ground_y`.
- **minor — `reference_pos` is the stub centre, not vanilla's**
  (`plan_source.rs:496-499`); `StructureStart.placeInChunk` uses
  `(centerPos.x, centerBB.minY, centerPos.z)`. Nothing observable changes today (no
  reachable village list declares `position_predicate`), but the modelled input is wrong.
- **minor — a named processor list missing from the closure silently becomes empty**
  (`plan_source.rs:535-543`), and `validate` probes through the same function, so the
  piece validates clean and then places with only the ignore/jigsaw/projection passes.
- **minor — stale docs**: the added paragraphs in `docs/PLUGINS.md` and
  `docs/decisions/0008-overworld-density-router.md` still say the `vanilla` profile
  places no villages, and `docs/VILLAGE_GENERATION.md` said the lookup "keeps the one"
  plan. (The `village/mod.rs` header claim about `feature_pool_element` was corrected in
  `d766018c`.)

The advisor added the same class of defect for the skipped decor lane: `grow`'s
`PlacedKind::Feature` arm `continue`s past a drawn element, while vanilla places the
feature there and the branch terminates because the element has no connectors - so the
candidate order and the RNG stream already diverge, and the village may connect a piece
vanilla never placed. The decor checkpoint must reproduce `FeaturePoolElement`'s
position/terminal-vs-retry/RNG semantics, not merely call `CompiledPlacedFeature`.

**Decor lane: the decompiled `FeaturePoolElement` contract (verified 2026-09-14,
`/tmp/vandecomp/net/minecraft/world/level/levelgen/structure/pools/FeaturePoolElement.java`).**
Recorded so the decor checkpoint reproduces the seam instead of guessing it:

- `getSize` returns `Vec3i.ZERO`, so `getBoundingBox(manager, position, rotation)` is
  the degenerate box `position..position` (`getYSpan() == 1`) and `place` ignores
  `rotation`, `referencePos` and `chunkBB` entirely. It is a single-block leaf, not a
  template.
- `StructureTemplatePool.getMaxSize` filters out only `EmptyPoolElement`, so an
  all-feature pool (`village/*/decor`, `village/*/trees`) reports `maxSize == 1` — that
  value feeds `use_expansion_hack`'s `expandTo + 1` for children that point at such a
  pool.
- As a *source*, `getShuffledJigsawBlocks` returns exactly one synthetic jigsaw at
  `position` with front `DOWN`, top `SOUTH`, name `bottom`, pool `Pools.EMPTY`
  (`minecraft:empty`), target `EMPTY_ID`, joint `ROLLABLE`, `final_state`
  `minecraft:air`. A one-element list means `Util.shuffle` draws nothing, and the empty
  pool then attaches nothing, so the branch terminates. A drawn feature element must
  therefore be **placed as a terminal leaf**, never skipped in favour of the next
  candidate the way `grow` does today (`solver.rs:565-577`) — skipping it changes both
  the candidate order and the RNG stream, which is why "village geometry is vanilla's"
  is currently unverified rather than merely decor-incomplete.
- `place` calls `feature.value().place(level, generator, random, position)` with the
  **same** `RandomSource` that drives jigsaw growth, so the feature consumes draws from
  the shared stream at exactly that point in the assembly.
- Because the target box is degenerate, `Shapes.create(AABB.of(targetBB).deflate(0.25))`
  is empty, so the `joinIsNotEmpty(..., ONLY_SECOND)` acceptance test cannot reject a
  feature element.

## Vanilla-faithful village solver and the decor lane (landed 2026-09-14)

The solver now reproduces `JigsawPlacement`'s growth seam and the
`feature_pool_element` decor lane is wired, as one checkpoint: the assembled
geometry, the placement seeds and the chunk writes changed together, so
`WORLDGEN_REVISION` is 23 (`crates/mc-worldgen/src/lib.rs`). Base tree
`e1a46a31`; committed on `main` as the `feat(worldgen)` commit that carries this
entry (naming its own hash would make the record stale the moment it is written,
so this entry names its base tree instead).

What landed, all read from the decompiled 26.1.2 classes and pinned by tests:

- **Selection re-draws.** `ChunkGenerator.createStructures` removes the failed
  entry and draws again from the *same* `setLargeFeatureSeed` stream, so a
  candidate chunk holds a village whenever any entry's biome gate passes — the
  single-roll model that made most candidates empty was wrong
  (`village_solver_tests::selection_retries_until_an_entry_passes_its_biome_gate`).
- **The biome gate is decided at the stub** and the growth random is
  per-attempt/discarded, so the gate can be answered before growing: identical
  villages, ~5× cheaper per candidate (`plan_start`/`grow_start` split).
- **Growth fidelity:** rotated source jigsaw positions and faces, shuffled pool
  *and* fallback candidate lists, the empty element terminating the candidate
  list, `use_expansion_hack`'s `max(expandTo + 1, maxY - minY)` applied to the
  target box, `continue 'source_jigsaw` after an attach, one shared-and-grown
  free shape per source (inside vs outside), both junctions with
  `groundLevelDelta` (rigid `parentDelta − deltaY`, non-rigid 1), RIGID boxes as
  beard contributions, the `placement_priority` queue, and feature elements as
  terminal leaves carrying the synthetic `bottom` front jigsaw.
- **The start anchor subtracts the named connector's local offset**
  (`JigsawPlacement.addPieces`: `adjustedPosition = position − (anchor − position)`).
  No village structure declares `start_jigsaw_name`, so nothing observable moved,
  but the modelled input was wrong.
- **Decor lane.** `village/decor.rs` compiles the closure's
  `feature_pool_element` entries and seeds per (chunk, structure) the way
  `ChunkGenerator.applyBiomeDecoration` does — `setDecorationSeed(seed, chunkX ×
  16, chunkZ × 16)` then `setFeatureSeed(decorationSeed, index, step)`, index
  being the structure's position in its step's registry list, which the data
  layer derives by sorting on `(path, namespace)` (`Identifier.compareTo`).
- **The decor random is vanilla's `WorldgenRandom` wrapper**, not a raw
  `XoroshiroRandomSource`: `WorldgenRandom extends LegacyRandomSource`, so every
  composed draw is the legacy formula over the wrapped bits. The first versions
  used the raw source, which is a different stream from the same seed.
- **Chest `LootTableSeed` comes off that same stream.** `StructureTemplate
  .placeInWorld` draws one `nextLong` per container it writes (after the block is
  set, never for a clipped block), so the piece lane draws it from the
  per-(chunk, structure) random, in piece order, and the decor that follows in
  that order continues it. The plugin prototype lane keeps its own hashtable seed
  (`structures::chest_loot_seed`); only the vanilla village lane uses the stream.
- **Terrain-adaptation and decor divergences stay declared**, not hidden
  (`docs/VILLAGE_GENERATION.md`), and its claim that a pile or flower at a chunk
  border loses nothing was wrong — piles scatter 2–3 blocks and plain-flower
  patches 64 attempts up to 6 blocks, so a border decor piece loses its
  out-of-chunk scatter.

**Measured performance fix, in the same checkpoint.** The lookup's expensive part
is per *start* chunk while questions arrive per column, so the ore/cave halo
(~900 columns per chunk) re-ran the same assemblies hundreds of times.
`VillagePlanSource` now keeps a bounded memo of assemblies keyed by start chunk
(64 entries, only `Some`, solved outside the lock) and `OreColumnCache::plans`
memoizes the chunk's plan set (the *chunk's* set, never a column's filtered
slice). Evidence, debug build: `cargo test -p mc-worldgen --lib` 373 s → 40 s; the
live village test's 110 moved columns 78.4 s → 0.18 s and its chunk generation
2.19 s → 0.05 s. An assembly is a pure function of the world seed and its start
chunk, which is why the memo cannot answer differently from a miss; the memo
belongs to one world's source, and no production path reconfigures a generator
that holds one (world identity fences mode/geometry).

**Evidence.** `cargo test -p mc-worldgen --lib` 256 passed / 0 failed / 5 ignored;
`cargo test -p mc-server --bin mc-server -- structure_rules` 4/0 with the live
activated-path proof printing `seed 4242, village at chunk (379, 0), 95 pieces,
188 junctions, 55 beard pieces, region 5989,49,-28..6146,94,97, 696 village
blocks, 11 columns moved` (257 worldgen tests after the anchor regression test was
added); the full L2 gate `run correctness` PASS
`.analysis/validation/20260914T231326-correctness-wec8qjcb`, run on the tree this
checkpoint was pushed as (`e01b5bb1` on `main`) — earlier runs
`20260914T142832-correctness-rk8v8o75`, `20260914T145752-correctness-cpnapg1f`
and `20260914T225812-correctness-14v_9___` predate it, the last of them by a
doc-comment fix in `mc-server`.

**Sibling package state, for continuing on another machine.** The core's
`deployed_sibling_plugins_prepare_runtime_and_worldgen_profiles` test — recorded
as owner-blocked in the previous entry because the sibling checkout lacked the
package it deploys — passes again, and `../solaris-default-plugins` is pushed (as
`b01342b`, plus `1d7db97` dropping the live entity dumps that first commit
accidentally carried: this repository never ships world or server state):
`solaris-settlements` replaces the removed
`colony-villager-scaffold`/`settlement-prototype` packages (22 authored
blueprints plus their generator, a strict server-only manifest with no worldgen
selector), the wave's narrative receipts moved to a tracked `evidence/`
(`c4-combat/receipt.md` and `live-report2.json`; the entity samples and drive
logs behind them are local only), and
`tools/gen_structures.py --check` reports the catalog current. Verified against
it here: `cargo test -p mc-server --bin mc-server --
deployed_sibling_plugins_prepare_runtime_and_worldgen_profiles` 1/0,
`cargo test -p mc-net -- settlement` 59/0, and a strict deployment of the six
standard-plus-settlement packages under
`target/debug/mc-server --check` (exit 0, all six discovered `server_only`). The decor stream, chest seed and start-connector facts are pinned
against the real classes: `.analysis/codex-logs/village-decor-random/` (Java
probe over the named `client-26.1.2.jar`, with its output and the source facts it
sits on).

**Independent review of this checkpoint (2026-09-14, verdict `changes`).** Six
findings, all resolved here:

- blocker — the decor random was the raw `XoroshiroRandomSource`, and the test
  pinned that: replaced with the `WorldgenRandom` wrapper, and every draw the
  Java probe read is now asserted.
- blocker — chest `LootTableSeed` did not come off the structure stream: fixed as
  above, with a test that a clipped chest does not draw.
- blocker — the ore-halo memo cached a *column-filtered* plan set under the chunk
  key, which suppressed a village for the chunk's other columns: fixed to cache
  `village_plans_for_chunk`.
- should-fix — the start anchor did not subtract the connector's local offset:
  fixed (unreachable for the village set, wrong as a model).
- should-fix — the assembly memo's reuse assumes the caller's terrain answers are
  the world's: kept, and stated as the memo's contract in the code and in
  `docs/VILLAGE_GENERATION.md` rather than enforced structurally.
- note — the doc over-claimed that piles and flowers at a border lose nothing:
  corrected.

The start-anchor fix is now covered: `the_start_piece_is_anchored_on_its_named_connector`
builds a synthetic structure whose named connector sits at a non-origin local
offset (all five village structures leave `start_jigsaw_name` absent, which is why
the live plans cannot see the convention), asserts the connector lands on the
start chunk's minimum block corner, and fails on the pre-fix code with
`left: 202, right: 192` — twice the rotated offset.

The review also repeated an earlier note in this file that
`FeaturePoolElement.place` runs on the *growth* random. That is wrong and the
entry above supersedes it: `JigsawPlacement.addPieces` never places features, and
`FeaturePoolElement.place` is called from `StructureStart.placeInChunk` with the
`FEATURES`-step random `applyBiomeDecoration` seeded per (chunk, structure).

**Unresolved, stated rather than approximated.** Villagers are still not spawned
from piece entity markers; `start_height` providers other than `absolute` and
vanilla's `dimension_padding` are unmodelled (documented in the route page); the
`plains_village_prototype` composite and its profile remain until a plugin-side
placement mechanism exists; chest *contents* are rolled by the `mc-data` catalog
implementation seeded with vanilla's `LootTableSeed`, which is not yet a
vanilla-`LootTable` execution; the plugin settlement `ground.surface_height`
callback pays one plan lookup (and now one memo insert) per column.

**Manual/client gate: not run for revision 23.** No PrismLauncher walk and no
harness real-client profile was run for this revision; the evidence is the Rust
gates plus the Java probe over the real classes. What is therefore unexercised is
what a client shows — a village's silhouette on generated terrain, the chest
contents a player opens, and the decor a player walks through.

**Next.** Take the next village/parity outcome from the route page — the villager spawn from piece
markers is the largest remaining gap between a generated village and vanilla's.
