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
at startup, `WORLDGEN_REVISION` is 22, and the interim
`settlement_profile_vanilla_generates_no_villages` warning is retired. The one
notice a vanilla village world still carries is the terrain-adaptation analogue
below.

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
generated is that candidate. Which of the five structures starts there is a
weighted pick over the entries whose biome tag contains the start column's
biome, rolled from the same per-chunk random vanilla uses.

All five are `minecraft:jigsaw` structures with `size` 6,
`max_distance_from_center` 80, `start_pool`
`minecraft:village/<type>/town_centers`, `start_height` absolute 0,
`project_start_to_heightmap` `WORLD_SURFACE_WG`, `step` `surface_structures`,
and `terrain_adaptation` `beard_thin`. The start piece is anchored to the
world-surface height of the start chunk; the village then grows by connecting
jigsaw blocks across the template pools. Piece blocks are written with their
rotation, projection and processor list, and `feature_pool_element` entries
(trees, hay piles, flowers) run through the placed-feature executor.

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

- `crates/mc-data/src/village_data.rs` resolves the four worldgen registries by
  reference and fails closed on anything unsupported, naming the type id and the
  entry that referenced it.
- `crates/mc-worldgen/src/village/placement.rs` holds the `random_spread`
  candidate-chunk calculation and the weighted structure pick.
- `crates/mc-worldgen/src/village/mod.rs` assembles the pieces into the
  `VillagePlan` the caller writes into chunks.
- `crates/mc-worldgen/src/structures.rs` reads piece templates and follows the
  jigsaw walk.

## `settlement_profile` semantics

`[data] settlement_profile` selects the settlement authority for a world. The
value is part of the persisted world identity, so changing it means a fresh
`world_dir` (see [World identity](#world-identity-and-reusing-a-world)).

| Value | What places villages |
| --- | --- |
| `vanilla` (default) | Core Solaris generates the five vanilla village structures above: the startup path loads the `minecraft:villages` closure from the derived content cache, validates every reachable piece against the block registry, and attaches the plan source to the terrain generator. |
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
keeps the one whose region overlaps the chunk, and drives both consumers from
that single plan — the piece blocks written during chunk generation and the
column heights read at the surface decision. There is no second plan
computation and no plan cache; the lookup is re-derived per generated chunk, and
a column query re-derives the plan for its own chunk.

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

## More than one village over a chunk

`minecraft:random_spread` places a candidate at `grid * spacing + spread` with
`spread` in `0..spacing - separation`, so candidates in neighbouring grid cells
can be as little as `separation + 1` chunks apart (9 for the villages set) while
a village's region reaches up to eight chunks past its start chunk. A chunk in
that gap holds part of two villages.

The engine returns every plan whose region reaches the chunk, in ascending
start-chunk order, places each plan's piece blocks, and sums the terrain
analogue's contributions before the surface is decided — the way vanilla's
`Beardifier` sums every structure the chunk references. A chunk reached by no
plan is byte-identical to the same world generated without villages; that is
asserted directly, blocks, heightmaps and every column's surface.

Two consequences worth knowing as an operator:

- A village's blocks are clipped to the chunk being filled, so a village
  spanning several chunks is written chunk by chunk as each is generated.
- `terrain_matching` pieces (streets, terminators, plazas) follow the terrain:
  their gravity processor snaps each block to the height at that block's own
  column. On sloped ground a street therefore steps with the landscape rather
  than staying level, and its blocks can sit outside the piece's own box
  vertically. Locality of a village's influence is a horizontal property.

## World identity and reusing a world

Every generated Solaris world persists `solaris/world.json`, which fences
generation revision, seed, worldgen mode, chunk geometry, ore profile,
settlement profile, and the selected spawn block. A world that was generated
without villages, or with a different settlement authority, is a different
generation identity — it is not a superset or a subset of this one.

The village checkpoint changes newly generated terrain in two ways: villages
place structures and their contents where there were none, and the terrain
analogue moves columns around every village. Both are generation changes, so
both are fenced by `WORLDGEN_REVISION` in `crates/mc-worldgen/src/lib.rs`.

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

`WORLDGEN_REVISION` is 22: revision 21 worlds were generated without villages
and are refused with the fresh-`world_dir` message above. The
`settlement_profile_vanilla_generates_no_villages` warning is retired — the
default profile now has villages to report — and the terrain-adaptation analogue
notice is the one a village world carries.

## Not implemented

These are the places village generation still fails closed or does nothing, and
they are deliberate:

- **Non-village structures.** Only the `minecraft:villages` structure set is
  assembled. The other structure sets in the content cache (ancient cities,
  mineshafts, ocean monuments, ruined portals, strongholds, mansions, and the
  rest) are not placed by Solaris, and the data layer only accepts
  `minecraft:jigsaw` structures driven by `minecraft:random_spread` placement.
  A structure or placement of another type fails the load by name instead of
  being approximated.
- **Village decor features.** The `feature_pool_element` entries the village pools
  reach (the 13 placed features the executor implements: trees, flowers, berry
  bushes, hay/ice/melon/pumpkin/snow piles, cactus) are not yet placed by the
  activated village path. The executor and its live proof exist, and the closure
  carries the features, but the jigsaw growth loop only accepts piece elements
  today, so a village generates its buildings, streets and terrain analogue
  without its decorative features. Nothing is placed in their place, and the
  operator sees the gap named here rather than a silently thinner village. The
  owner accepted this gap for the activation checkpoint and made wiring the
  decor lane the next village item.
- **Other terrain adaptations.** Only `beard_thin` has a version of the
  analogue. `none` needs no behaviour, and `bury`, `beard_box` and
  `encapsulate` have no engine-side analogue; no structure in the villages set
  declares them.
- **Jigsaw content outside the village closure.** The registry readers load
  entries by reference and fail closed on anything they reach that is not
  implemented — other pool element types, processor types, rule tests, height
  providers, and non-`minecraft` namespaces are rejected by type id and
  referring entry rather than being skipped.
- **Placed features outside the village decor closure.** The feature executor
  implements the feature types, state providers, placement modifiers, block
  predicates and int providers the village pool's `feature_pool_element` entries
  reach. Everything else fails closed at load or compile time, including
  `schedule_tick` on `simple_block`, double plants, and foliage providers that
  can place a state without a `distance` property.
- **Not claimed.** Villager population and behaviour, raid and patrol
  interaction, and vanilla's exact structure-spacing behaviour for structure
  sets other than `villages` are outside this page.
