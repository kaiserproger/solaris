# Village Generation

Quality label: `draft`.

This page is the operator-facing description of vanilla village generation in
Solaris: what places a village, what each `[data] settlement_profile` value
means, the one terrain-adaptation divergence we declare on purpose, what it does
to world identity, and what is still not implemented.

Village assembly follows the placed-feature executor
(`crates/mc-worldgen/src/vanilla_features/`,
`crates/mc-data/src/vanilla_feature_closure.rs`). Its engine
(`crates/mc-worldgen/src/village/`) and its data loaders
(`crates/mc-data/src/village_data.rs`) are active library code: the default
`settlement_profile` builds a village plan source from the derived content cache
at startup, `WORLDGEN_REVISION` is 24, and the interim
`settlement_profile_vanilla_generates_no_villages` warning is retired. The notices
a vanilla village world still carries are the terrain-adaptation analogue and the
mobs the lane does not spawn, both below.

## What generates a village

Vanilla villages are a single structure set, `minecraft:villages`, holding five
structures at equal weight, each gated by a biome tag:

| Structure | Biome tag | Biomes it may start in |
| --- | --- | --- |
| `minecraft:village_plains` | `#minecraft:has_structure/village_plains` | `plains`, `meadow` |
| `minecraft:village_desert` | `#minecraft:has_structure/village_desert` | `desert` |
| `minecraft:village_savanna` | `#minecraft:has_structure/village_savanna` | `savanna` |
| `minecraft:village_snowy` | `#minecraft:has_structure/village_snowy` | `snowy_plains` |
| `minecraft:village_taiga` | `#minecraft:has_structure/village_taiga` | `taiga` |

Placement is `minecraft:random_spread` with spacing 34 chunks, separation 8, and
salt 10387312. Each 34×34-chunk cell has exactly one candidate chunk, chosen
deterministically inside the cell; the set generates when the chunk being
generated is that candidate.

**Which structure starts there is a re-drawn weighted draw, not one roll.**
Vanilla seeds a `WorldgenRandom` with `setLargeFeatureSeed(seed, chunkX, chunkZ)`
and repeatedly draws a weighted entry from the set's *remaining* entries: a drawn
entry that fails its biome gate is removed and the next draw comes from the same
stream. A candidate chunk therefore holds a village whenever any of the five
entries is eligible there — a single-roll implementation would leave most
candidate chunks empty, which is the difference between a village roughly every
34 chunks and a village roughly every 170.

The biome gate itself is decided on the *assembled* structure: vanilla resolves
the biome at the start position the assembly reports (`StructureStart`'s stub,
the start piece's box centre) and tests it against the structure's biome tag.

All five are `minecraft:jigsaw` structures with `size` 6,
`max_distance_from_center` 80, `start_pool`
`minecraft:village/<type>/town_centers`, `start_height` absolute 0,
`project_start_to_heightmap` `WORLD_SURFACE_WG`, `step` `surface_structures`,
`use_expansion_hack` true and `terrain_adaptation` `beard_thin`. The start piece
is anchored to the world-surface height of the start chunk's centre column; the
village then grows by connecting jigsaw blocks across the template pools:

- the start pool's weighted element and the start piece's rotation are drawn from
  the growth random, and the piece is anchored on the named start jigsaw when the
  structure names one;
- for each source jigsaw — its world position and its `orientation` faces rotated
  by the piece's own rotation, then shuffled and ordered highest
  `selection_priority` first — the target pool's weight-expanded element list is
  copied and shuffled (at less than full depth) and the fallback's list is copied
  and shuffled after it;
- each candidate is tried under the four rotations and then its own shuffled
  jigsaws; the first pair that `canAttach` and whose target box fits the free
  space attaches, `use_expansion_hack` raises the target box first, both
  junctions are written, and the child is queued at the source jigsaw's
  `placement_priority`; one attachment abandons that source jigsaw's remaining
  candidates;
- the free space a candidate must fit into is shared and grown in place: one
  shape for the whole village, plus a per-piece shape for attachments that land
  inside their source piece.

Piece blocks are written with their rotation, projection and processor list.
`feature_pool_element` entries (trees, hay piles, flowers) are **placed**: they
are leaves of the growth, and each is run through the placed-feature executor
with the random vanilla's `FEATURES` step seeds (see [Village decor](#village-decor)).

These are vanilla's own numbers and are read from the derived content cache, not
from Solaris constants: `data/minecraft/worldgen/structure_set/villages.json`,
`data/minecraft/worldgen/structure/village_*.json`, the
`data/minecraft/tags/worldgen/biome/has_structure/village_*.json` tags, and the
`worldgen/template_pool/`, `worldgen/processor_list/` and `structure/` data
underneath them. There is no separate village dataset to install — villages use
the same derived content cache as the rest of vanilla worldgen (see
[Plugin worldgen](PLUGINS.md#package-and-manifest) for cache discovery and
`mc-server content import`).

Code paths:

- `crates/mc-data/src/village_data.rs` resolves the worldgen registries by
  reference and fails closed on anything unsupported, naming the type id and the
  entry that referenced it. It also lists the structure registry's ids and their
  `step` — the one enumeration in the data layer — because vanilla's decor
  placement is seeded from a structure's index within its step.
- `crates/mc-worldgen/src/village/placement.rs` holds the `random_spread`
  candidate-chunk calculation and the `setLargeFeatureSeed` derivation the
  structure selection and the growth share.
- `crates/mc-worldgen/src/village/solver.rs` runs the structure selection, the
  jigsaw growth and the biome gate, and produces the `VillagePlan` the caller
  writes into chunks.
- `crates/mc-worldgen/src/village/decor.rs` compiles the closure's
  `feature_pool_element` entries and seeds the random they run on.
- `crates/mc-worldgen/src/village/plan_source.rs` is the one lookup both
  consumers read, and writes both lanes into a chunk, including the inhabitant
  markers of the villagers the pieces place.
- `crates/mc-worldgen/src/village/processors.rs` and
  `crates/mc-worldgen/src/structures.rs` execute the processor lists and the
  piece templates; the template reader is also what reads each template's
  `entities` list.
- `crates/mc-net/src/play/session/settlement_authority.rs` resolves a chunk's
  inhabitant markers into villagers and spawns them when the chunk is streamed
  (see [Village inhabitants](#village-inhabitants)).

## Village decor

The pools' decorative entries are `minecraft:feature_pool_element`: a single
placed feature (oak/spruce/pine/acacia trees, plain flowers, berry bushes, taiga
grass, cactus, and hay/ice/melon/pumpkin/snow piles) rather than a template. In
vanilla one is a *piece* like any other — a one-block leaf whose synthetic jigsaw
points at the empty pool, so it attaches, terminates the branch and is written
when the chunk it lands in is generated — and its feature is then placed with:

- the `FEATURES` step's random: vanilla's `WorldgenRandom` *wrapper* over an
  `XoroshiroRandomSource` — the wrapper matters, because `WorldgenRandom extends
  LegacyRandomSource` and so composes every draw with the legacy formulas over
  the wrapped bits, which is a different stream from a raw
  `XoroshiroRandomSource` — seeded per chunk with
  `setDecorationSeed(worldSeed, chunkX × 16, chunkZ × 16)` and then per structure
  with `setFeatureSeed(decorationSeed, index, step)`, where `step` is
  `GenerationStep.Decoration.SURFACE_STRUCTURES` for the village set and `index`
  is the structure's position among the registered structures of that step;
- one stream per (chunk, structure): every plan of the same structure that
  reaches the chunk shares it, in plan order, and a piece the chunk does not
  place does not draw from it.

That stream is not the decor's alone. Vanilla rolls a chest's `LootTableSeed`
from it — `StructureTemplate.placeInWorld` draws one `nextLong` per container
block entity it writes, after the block is set and never for a block its clip
dropped — so the piece lane draws its chest seeds from the same per-(chunk,
structure) stream, in piece order, before the decor that follows in that order.
A template that reaches no container and a plan that reaches no feature never
touch the stream at all.

The seeding is pinned against numbers read out of the real 26.1.2 classes
(`.analysis/codex-logs/village-decor-random/`), not restated from the formula:
seed 4242 at chunk (427, 0) gives the decoration seed 3 979 914 027 210 390 498,
and `village_plains`'s index 22 at step 4 then draws
−5 071 117 971 071 978 252 — the value the tests assert.

The features themselves are the data-driven executor's: their types, states,
tokens, noise and placement modifiers come from the derived cache, resolved by
reference from the pool entries the closure reaches, and anything outside that
closure fails the startup load by name.

## `settlement_profile` semantics

`[data] settlement_profile` selects the settlement authority for a world. The
value is part of the persisted world identity, so changing it means a fresh
`world_dir` (see [World identity](#world-identity-and-reusing-a-world)).

| Value | What places villages |
| --- | --- |
| `vanilla` (default) | Core Solaris generates the five vanilla village structures above, decor included: the startup path loads the `minecraft:villages` closure from the derived content cache, compiles its feature elements, validates every reachable piece against the block registry, and attaches the plan source to the terrain generator. |
| A deployed Luau settlement plan | The plan's worldgen declaration wins. Its descriptor string — profile, owner, buildings, inhabitants, extensions — is the recorded settlement identity. |
| `plains_village_prototype` | Interim opt-in, still present because its composite is not yet removable (see below). It attaches no core villages and is not vanilla village generation. |

**A deployed Luau settlement plan wins, and nothing is placed twice.** Core
villages attach to exactly one configuration: `settlement_profile = "vanilla"`
with no deployed Luau settlement plan. A deployed plan owns settlement content,
so startup attaches no plan source and logs that the plugin owns it; the
`plains_village_prototype` profile keeps its own structure rules and likewise
attaches no core villages. Two plugins claiming the same profile fail startup
rather than resolving by load order, and a deployed set carrying no plan leaves
the config value in charge.

The catalogue-driven path is separate from worldgen placement: a package that
declares `world_sites` and `structure_operations` and ships an authored
`structures/` catalogue places and stages its own settlements at runtime through
the world-storage kernel rather than through the structure rules. The shipped
`solaris-settlements` package declares no `[worldgen]` selector, so installing
it does not change the worldgen settlement identity. See
[Luau Plugins](PLUGINS.md) for that path.

`plains_village_prototype` was the interim opt-in. It combined three vanilla
plains templates (`plains_fountain_01`, `plains_small_house_1`,
`plains_tool_smith_1`) into one bounded composite placed on the extracted plains
village spacing, separation and salt, required a `vanilla_data_dir` sidecar for
the piece NBT, and placed no desert, savanna, snowy or taiga villages. It is not
vanilla village generation and the default profile no longer needs it.

**Its removal is not done, and this is the unresolved part of the checkpoint.**
The composite is still the mechanism a deployed Luau settlement plan
materializes its buildings through (`structure_rules_for_startup` maps the
plan's `PlainsFountain`/`PlainsSmallHouse`/`PlainsToolsmith` templates onto
`PlainsVillagePrototypePart` and builds `StructureRules::plains_village_prototype`),
so deleting it now would delete the plugin settlement route with it. Removing the
profile and the composite requires the plugin path to place plan buildings
through its own mechanism first; until that exists, the profile and the composite
stay, and this page does not claim they are permanent.

## Declared divergence: terrain adaptation

**This is a documented divergence, not parity.** The placement grid, the piece
block layout and the ground-level targets follow vanilla's data; the terrain
around a village is not shaped with vanilla's arithmetic.

### What vanilla does

`terrain_adaptation = beard_thin` is a **density** term inside the noise router
(`net.minecraft.world.level.levelgen.Beardifier`). For the village pieces near a
chunk it collects the RIGID piece boxes and their jigsaw junctions, computes an
affected box as the union of those boxes inflated by 24, and inside that box
adds, per 3D sample:

- per rigid piece, `0.8 × getBeardContribution(dx, dy, dz, dyToGround)`, where
  `dx`/`dz` are the distances outside the piece's bounding box and
  `dyToGround = y − (box.minY + groundLevelDelta)`;
- per junction, `0.4 × getBeardContribution(...)` around the junction's ground
  position.

`getBeardContribution` multiplies the precomputed 24³ kernel (radius 12,
offsets `+12`, `e^(−d²/16)`) by `−dyWithOffset × fastInvSqrt(distance²/2) / 2`.
Because the result is a density value, it flows through the normal surface and
solidity pipeline and can add or remove material at any height in the box.

### What Solaris does instead

Solaris terrain is a 2D per-column surface
(`TerrainGenerator::surface_height`, `crates/mc-worldgen/src/terrain.rs`) plus
cave carving, so there is no 3D density array for the term to enter. The
analogue keeps the same numbers — the same 24³ kernel support, the same
`e^(−d²/16)` kernel, the same 0.8 per rigid piece and 0.4 per junction, and the
same `box.minY + groundLevelDelta` target — and applies the resulting offset to
the *column height* instead of to a density sample: columns inside a piece's
influence are moved toward that piece's ground level (lowered where they sit
above it, raised where they sit below it). The offset is rounded to whole
blocks, clamped inside the generated vertical range, and a column whose offset
rounds to zero is left alone. The plan reports every moved column, so a caller
can log exactly how much terrain moved.

The divergence is the medium, and only the medium. It lives in
`crates/mc-worldgen/src/village/beard.rs`, where the vanilla contribution
function is kept as a separate, public function so a future density-pipeline
implementation can reuse it unchanged.

### Where the analogue runs

One lookup decides a village's plan per chunk
(`crates/mc-worldgen/src/village/plan_source.rs`): for the chunk being filled
the generator assembles the structure starts the placement grid puts in reach,
keeps every one whose region overlaps the chunk, and drives both consumers from
that one set — the piece blocks written during chunk generation and the column
heights read at the surface decision. The chunk that is being filled looks its
plan set up once and shares it with all of its columns
(`TerrainGenerator::village_plans_for_chunk`), and the ore and cave pass, whose
column cache reaches into neighbouring chunks
(`OreColumnCache::plans`), looks each of those chunks up once rather than once
per column.

The expensive part of a lookup is per *start* chunk, not per question: a
candidate that can start a village runs the whole solver, and the questions
arrive per column. `VillagePlanSource` therefore keeps the start chunks it has
already assembled in a bounded memo (`AssemblyCache`, 64 entries). An assembly
is a pure function of the world seed and its start chunk — the free height and
biome it reads come from the generator and are the world's own — so a memo hit
cannot answer differently from a miss, and eviction costs only the work to redo
it. A candidate the placement formula rejects is never memoized: it costs that
formula and nothing else, and entries holding such answers would crowd out the
real villages.

The memo belongs to one world: a `VillagePlanSource` carries that world's seed,
and every lookup hands it that world's own free height and biome, so an assembly
is reusable for the source's lifetime and a memo hit cannot answer differently
from a miss. Nothing is cached across worlds and there is no second plan
computation: a column query outside generation
([`TerrainGenerator::surface_height`],
[`TerrainGenerator::diagnostic_sample`]) derives the plan of the chunk it
belongs to from the same memo, so a column and its chunk cannot disagree.

Measured on the live village proof (`cargo test -p mc-worldgen --lib
live_village_plan_moves_columns_and_writes_blocks`, debug, one plains village of
123 pieces): the test's 110 moved columns cost 78.4 s before the memo and 0.18 s
after, and generating the house chunk 2.19 s before and 0.05 s after.

A generator with no village plan source is unchanged: it assembles nothing,
places nothing and blends nothing, byte for byte. A build that does place
villages emits the typed notice
`village_terrain_adaptation_beard_thin_is_a_column_height_analogue` at startup,
so the divergence is reported rather than left to be found in a diff.

### What that means on sloped ground

- Vanilla blends a hillside under a rigid piece into the piece's ground level in
  three dimensions, with sub-block detail and material above and below the piece
  footprint. The analogue can only move the top of each column, so a slope
  becomes a stair-step of whole-block moves rather than a smooth skirt.
- Contributions that vanilla would express as a sub-block density nudge vanish
  when they round to zero, so grazing influence is dropped.
- A column keeps a single surface height: the beard never produces an overhang
  or cavity, and terrain under a piece is not buried or boxed in.
- Footprints, piece placement, block layout, chest positions and the ground
  level each piece targets are vanilla's; the surrounding relief is a
  whole-block approximation.

### What full parity would require

A real vanilla version has to carry a 3D density/solidity field through chunk
generation and evaluate the beard contribution where the router evaluates the
`BeardifierOrMarker` density function, instead of adjusting column heights after
the surface is known. It would also need the other adaptation modes vanilla
defines (`bury`, `beard_box`, `encapsulate`), which this analogue does not
implement; no structure in the villages set uses them.

## Declared divergence: decor writes stay in their chunk

Vanilla places a structure into a `WorldGenRegion` that holds the chunk being
generated *and its already-generated neighbours*, so a piece or a decor tree at a
chunk edge writes the part that falls in the neighbour. Solaris fills one chunk
at a time and has no neighbour access, so a decor feature's writes outside the
chunk being filled are dropped, and the neighbour does not place them later: the
leaf piece that carries the feature belongs to one chunk.

What that means as an operator: a village tree standing within a few blocks of a
chunk boundary loses the part of its canopy that crosses the boundary, and so
does anything else a feature places across it — a hay, melon, snow or ice pile
spreads two to three blocks from its position, and a plain-flower patch makes 64
attempts up to six blocks away, so a decor piece near a boundary loses the part
of its own scatter that lands in the neighbour. Piece *blocks* are unaffected —
vanilla clips those to the same per-chunk box this generator writes them through
— and the terrain analogue is computed per column from the plan, so heightmaps
stay correct.

## More than one village over a chunk

`minecraft:random_spread` places a candidate at `grid * spacing + spread` with
`spread` in `0..spacing - separation`, so candidates in neighbouring grid cells
can be as little as `separation + 1` chunks apart (9 for the villages set) while
a village's region reaches up to eight chunks past its start chunk. A chunk in
that gap holds part of two villages.

The engine returns every plan whose region reaches the chunk, in ascending
start-chunk order, places each plan's piece blocks and decor, and sums the
terrain analogue's contributions before the surface is decided — the way
vanilla's `Beardifier` sums every structure the chunk references. A chunk reached
by no plan is byte-identical to the same world generated without villages; that
is asserted directly, blocks, heightmaps and every column's surface.

Two consequences worth knowing as an operator:

- A village's blocks are clipped to the chunk being filled, so a village
  spanning several chunks is written chunk by chunk as each is generated, and a
  piece whose own box does not reach the chunk is not placed in it at all —
  vanilla's own per-chunk piece filter.
- `terrain_matching` pieces (streets, terminators, plazas) follow the terrain:
  their gravity processor snaps each block to the height at that block's own
  column. On sloped ground a street therefore steps with the landscape rather
  than staying level, and its blocks can sit outside the piece's own box
  vertically. Locality of a village's influence is a horizontal property.

Vanilla places the structures that reach a chunk in registry order, each with its
own freshly seeded random, so the order between two *different* structures does
not affect blocks or draws; two villages of the same type reaching one chunk
share one decor stream here in ascending start-chunk order, which is the one
ordering assumption this page makes.

## Village inhabitants

A village's houses carry their population in the *template*, not in the pool: a
house's `minecraft:bottom` jigsaw points at `minecraft:village/<type>/villagers`,
whose elements are `legacy_single_pool_element`s over the one-block templates
`village/<type>/villagers/{unemployed,nitwit,baby}`. Each of those templates
authors one entity in its NBT `entities` list — a `minecraft:villager` with its
`VillagerData` (`type`, `profession`, `level`) and `Age`, a `blockPos` and a
double `pos`.

Solaris reads that list and places the villagers the way vanilla's
`StructureTemplate.placeEntities` does: the entity's `blockPos` goes through the
piece's block transform, and it is dropped when the transformed block is outside
the chunk being filled — the same per-chunk box the piece's blocks are clipped to,
so an entity that belongs to the neighbouring chunk is placed when *that* chunk
is generated. The double `pos` goes through `transform(Vec3, ...)`, which mirrors
with `1.0 - coordinate` and carries the `+ 1` term in the rotated branches; the
yaw is `entity.rotate(rotation) + entity.mirror(mirror) - entity.getYRot()` on the
yaw the template authored (`Rotation[0]`), and the pitch is the entity's own
(`Rotation[1]`), which vanilla never rotates. Those functions are pinned against
the real 26.1.2 classes (`crates/mc-worldgen/src/village_piece_tests.rs`), and so
are the templates' own values: the adult villager templates author
`Rotation = [48.821632, 0.0]` and `Age = 0`, the baby templates
`Rotation = [0.0, -25.827711]` and `Age = -21359`.

The placement does not spawn an entity at generation time. It writes a **chunk
inhabitant marker** (the `SolarisSettlementInhabitants` entry in the chunk's
extras, `mc_world::SettlementInhabitantMarker`), which is the core's one
generation-to-runtime entity handoff: when the chunk is streamed to a client,
`mc-net` resolves each marker against the entity registry — the village types
`plains`/`desert`/`savanna`/`snow`/`taiga` and the professions `none`/`nitwit` —
and spawns the villager, with its authored `Age` (a negative `Age` selects the
baby brain schedule), the yaw its placement computed and the pitch its template
authored (clamped the way `Entity.setXRot` clamps it: `clamp(pitch % 360, -90,
90)`). The marker's claim
(`<pool element>@<piece x>:<piece y>:<piece z>#<entity index>`) is the villager's
identity in place of the `UUID` vanilla strips from the template: it seeds a
deterministic UUID and prevents a second spawn when the chunk is streamed again.
Restarting the server does not respawn a villager whose claim is already known.

The village lane spawns **villagers only**. A village also authors the
meeting-point iron golem, the animal pens' livestock, their cats, a desert camel,
a butcher shop's animals, an armour stand and the zombie villagers of the
weight-1 zombie town centres; those need their own entity state and spawn path,
so they are not placed, and startup reports exactly which ids the loaded closure
reaches (`village_piece_mobs_other_than_villagers_are_not_spawned`) rather than
leaving them silently absent.

The templates author **no POIs** for their villagers, and vanilla's own
behaviour — a villager claiming a bed, a workstation or the bell from the blocks
around it — is not modelled here. A marker therefore carries the core's existing
answer for that case (`default_villager_pois`, the shape the runtime already
applies to a villager with no brain state): the entity's own placed position is
its home, its meeting point, and the job site of a working profession. So a
generated village's villagers rest and gather where they were placed rather than
claiming the bed or workstation next to them, and the templates' two professions
(`none` and `nitwit`) work nowhere at all. That is a divergence from vanilla — the
shape of a village day survives, the POIs are the placement rather than the
furniture — not a reading of the template.

A village **baby** does carry one home-shaped field: the core's entity storage
contract requires a baby to name the home it was born into
(`villager_population_state_is_valid`), and a generated village has no settlement
home to name, so the baby records its own placement claim (the same string as the
villager's identity) as that home. It is not a settlement home claim and no
settlement site can be blocked by it — the two claim shapes never collide — but an
operator reading `entities.dat` will see a village baby whose home is its
placement rather than a bed.

## World identity and reusing a world

Every generated Solaris world persists `solaris/world.json`, which fences
generation revision, seed, worldgen mode, chunk geometry, ore profile,
settlement profile, and the selected spawn block. A world that was generated
without villages, or with a different settlement authority, is a different
generation identity — it is not a superset or a subset of this one.

The village checkpoint changes newly generated terrain in three ways: villages
place structures and their contents where there were none, the terrain analogue
moves columns around every village, and the assembled geometry and placement of
those villages changed with `WORLDGEN_REVISION` 23 (selection re-draws, rotated
source jigsaws, shuffled candidate lists, the expansion-hack box, the shared free
shape, the priority queue, the stub-position biome gate, and the decor lane).
All are generation changes, so all are fenced by `WORLDGEN_REVISION` in
`crates/mc-worldgen/src/lib.rs`.

The revision is **24**: village chunks now carry their inhabitants (see
[Village inhabitants](#village-inhabitants)), so a revision-23 chunk would
generate without them if it were reused.

What to expect as an operator:

- Reusing an existing world with the new binary is **refused**, not silently
  mixed: startup reports that the persisted revision/profile does not match the
  configured one and tells you to use a fresh `world_dir`.
- Existing chunks are never retroactively reshaped; deleting only
  `solaris/world.json` is not a migration and mixes incompatible terrain.
- Back up the whole world directory before upgrading an alpha, then start a
  fresh `world_dir` for the new generation.
- Deploying, removing, or editing a Luau settlement plan changes the recorded
  settlement descriptor and therefore the identity in the same way.
- An unversioned vanilla Anvil import still opens without Solaris fallback
  generation: Solaris does not generate villages into missing chunks of an
  import.

**No real-client run was made for revision 24.** The checkpoint's evidence is the
Rust gates (workspace tests, `fmt`, `code-health`, strict clippy) plus the Java
probe over the real 26.1.2 classes for the entity transform and the decor stream,
not a graphical client: no PrismLauncher walk, no harness real-client profile.
What that leaves unexercised is exactly what a client would show — a village's
silhouette on generated terrain, the villagers standing in it, the chest contents
a player opens, and the decor a player walks through — so a village walk is the
first thing that would test this revision visually.

`WORLDGEN_REVISION` is 24: revision-22 and revision-23 worlds carry villages
assembled from earlier solver fidelity (23) with no inhabitants and no decor
(22), and are refused with the fresh-`world_dir` message above. The
`settlement_profile_vanilla_generates_no_villages` warning is retired — the
default profile now has villages to report — and the notices a village world
carries are the terrain-adaptation analogue and the mobs the lane does not spawn
below.

## Not implemented

These are the places village generation still fails closed or does nothing, and
they are deliberate:

- **Village mobs other than the villager.** A piece template's *villagers* are
  placed (see [Village inhabitants](#village-inhabitants)); the rest of what a
  village authors is not: the meeting-point iron golem, the animal pens'
  livestock and their cats, a butcher shop's animals, a desert camel, a taiga
  armorer's armour stand, and the zombie villagers of the weight-1 zombie town
  centres. Each would need its own entity state and spawn path (a cat's variant,
  a horse's markings, an armour stand's pose). The closure reports every id it
  reaches that the lane does not spawn, and startup logs it, so the gap is
  visible instead of silently missing. `spawn_overrides` is parsed and carried
  but consumed by nothing.
- **Villager behaviour beyond the placement's own POIs.** A village villager is
  spawned with its authored `VillagerData` and `Age` and the placement-derived POI
  set above, so it keeps the schedule its age selects — resting and meeting where
  it was placed, never claiming the bed, workstation or bell next to it. Vanilla's
  POI acquisition, breeding, trading, raids and patrols are outside this page
  entirely.
- **Non-village structures.** Only the `minecraft:villages` structure set is
  assembled. The other structure sets in the content cache (ancient cities,
  mineshafts, ocean monuments, ruined portals, strongholds, mansions, and the
  rest) are not placed by Solaris, and the data layer only accepts
  `minecraft:jigsaw` structures driven by `minecraft:random_spread` placement.
  A structure or placement of another type fails the load by name instead of
  being approximated.
- **Start heights other than a constant absolute value.** Only
  `start_height: {"absolute": n}` is implemented; a structure naming `uniform`,
  `trapezoid` or a non-absolute anchor fails the closure load by name rather
  than placing at the wrong Y (those providers also sample the placement random,
  which the engine does not reproduce).
- **Vanilla's dimension padding.** `JigsawStructure`'s default `dimension_padding`
  clamps the growth limit box and rejects a start whose box comes within ten
  blocks of the world's vertical bounds. Neither is modelled, because a village
  starts at the world surface and never approaches those bounds; a structure
  anchored near the world floor or ceiling would grow differently from vanilla.
- **Other terrain adaptations.** Only `beard_thin` has a version of the
  analogue. `none` needs no behaviour, and `bury`, `beard_box` and
  `encapsulate` have no engine-side analogue; no structure in the villages set
  declares them.
- **Jigsaw content outside the village closure.** The registry readers load
  entries by reference and fail closed on anything they reach that is not
  implemented — other pool element types (`list_pool_element` is refused rather
  than flattened), processor types, rule tests, height providers, pool aliases,
  and non-`minecraft` namespaces are rejected by type id and referring entry
  rather than being skipped.
- **Placed features outside the village decor closure.** The feature executor
  implements the feature types, state providers, placement modifiers, block
  predicates and int providers the village pool's `feature_pool_element` entries
  reach. Everything else fails closed at load or compile time, including
  `schedule_tick` on `simple_block`, double plants, and foliage providers that
  can place a state without a `distance` property.
- **Not claimed.** Vanilla's exact behaviour for structure sets other than
  `villages`, and the structure-spacing nuances no other set exercises here, are
  outside this page.
