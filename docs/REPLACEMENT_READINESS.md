# Replacement Readiness

Solaris can be evaluated as a scoped vanilla 26.1.2 overworld-survival server,
not as a bit-perfect vanilla clone.

## Operator Setup

1. Put a Mojang 26.1.x `server.jar` at `.analysis/server.jar`.
2. Run `tools/extract-vanilla-data.sh` to populate `data/vanilla/`.
3. Prepare or reuse the local world under `.analysis/test-world`.
4. Start the debug server:

```sh
cargo run --bin mc-server -- --config example.toml
```

5. Connect with a vanilla 26.1.2 PrismLauncher client.

Do not commit `.analysis/`, `data/vanilla/`, local worlds, downloaded jars, or
client files.

## Ready Scope

- Vanilla login/config/play path.
- Overworld chunk streaming and lighting.
- Block breaking/placing, doors/trapdoors, buckets, signs as placeable blocks,
  beds as respawn anchors, and save/restart persistence.
- Crafting table, chest, barrel, furnace, smoker, and blast furnace. Barrels use
  single-container 27-slot storage and persist with barrel block-entity ids;
  smokers and blast furnaces use their own client menu ids and persisted
  block-entity ids while sharing the current furnace-family cooking runtime.
- Recipe loading accepts furnace, blasting, smoking, and campfire variants;
  furnace runtime routes furnace, blast furnace, and smoker categories.
- Basic farming, fluids, mobs/combat, death/respawn, XP, drops, inventory, and
  multiplayer visibility/pickups. Runtime block/entity drops use embedded
  repo-owned loot by default, or a configured local vanilla data sidecar's simple
  loot-table subset when `data.vanilla_data_dir` is present and contains loadable
  simple drops. The sidecar subset only accepts one-item drops with no loot
  functions and no conditions except `minecraft:survives_explosion`.
- Runtime metrics and budgets from M37 for debug-load visibility.

## Partial Scope

- Beds set respawn points; sleeping and time-skip are not implemented.
- Signs have partial plain-text editing support: regular sign placement opens the
  vanilla sign editor, matching serverbound updates store plain four-line text in
  vanilla-shaped block-entity NBT, and loaded clients receive the block-entity
  update. Solaris harness coverage verifies editor open, mismatched-position
  rejection, and client-visible text updates. Hanging signs,
  styled/filtered/clickable text, waxed sign editing semantics,
  sounds/statistics/game events, and full visual/manual parity are not claimed.
- Smokers/blast furnaces share the furnace-family runtime; lit-state block
  updates, sounds/particles, hopper/comparator automation, and full vanilla
  block-specific state-machine parity are not claimed.
- Barrels share chest storage mechanics; open/close animation, sounds, game
  events, and automation parity are not claimed.
- Campfire cooking has usable partial support with Solaris harness coverage for
  consuming matching held inputs, sending vanilla-shaped `Items` block-entity
  updates for the visible input and completion clear paths, dropping cooked
  results as item entities, and picking those results up. In-flight persistence
  across restart, hopper/comparator automation, smoke/sound particles, exact
  ejection vector/timing parity, and broader block-state parity are not claimed.
- Loot/drop parity remains partial and scoped. Solaris executes deterministic
  one-stack block/entity drops from the effective loot table, with embedded
  repo-owned fallback data available by default and a local vanilla sidecar
  simple subset available when configured. Harness coverage includes stone to
  cobblestone through configured block loot, configured passive mob drops,
  zombie fallback drops, visible item entities, pickup slot updates, and entity
  removal. Full vanilla loot execution is not implemented: `set_count`/random
  rolls, fortune/silk touch, tool predicates, entity/killer predicates,
  explosion decay, looting, equipment drops, exact XP ranges, and
  sounds/particles/statistics/game events remain out of scope.
- Farming has partial crop growth, bonemeal, wheat harvest drop support, carrot
  harvest drop support, potato harvest drop support, beetroot harvest drop
  support, and nether wart harvest drop support:
  wheat, carrots, potatoes, beetroots, nether wart, melon stems, pumpkin stems,
  sweet berry bushes, and cocoa can advance one age at a time through random ticks or
  successful bonemeal use; bonemeal consumes exactly one item only after growth; breaking
  wheat drops deterministic local
  yields, with mature `age=7` wheat dropping wheat plus seeds and immature
  `age=0` through `age=6` wheat dropping seeds only; breaking carrots drops
  deterministic local yields, with mature `age=7` carrots dropping carrot x2
  and immature `age=0` through `age=6` carrots dropping carrot x1; breaking
  potatoes drops deterministic local yields, with mature `age=7` potatoes
  dropping potato x2 and immature `age=0` through `age=6` potatoes dropping
  potato x1; breaking beetroots drops deterministic local yields, with mature
  `age=3` beetroots dropping beetroot x1 plus beetroot seeds x1 and immature
  `age=0` through `age=2` beetroots dropping beetroot seeds x1; breaking
  nether wart drops deterministic local yields, with mature `age=3` nether
  wart dropping nether wart x2 and immature `age=0` through `age=2` nether
  wart dropping nether wart x1. Mature melon and pumpkin stems can place one
  adjacent fruit block through random ticks or successful bonemeal use and
  convert the stem to the matching attached-stem state. Age 2 and age 3 sweet
  berry bushes can be harvested with use-on, reset to `age=1`, and drop
  deterministic local sweet berry yields. Breaking cocoa drops deterministic
  local cocoa bean yields, with mature `age=2` cocoa dropping cocoa beans x3
  and immature `age=0` through `age=1` cocoa dropping cocoa beans x1. Cocoa
  beans can place age-0 cocoa on horizontal jungle-log faces when the target
  cell is clear. Crop harvest item ids are resolved through the loaded item
  registry and missing ids are omitted safely. Solaris harness coverage exists
  for young wheat growth, mature wheat no-consume behavior, mature wheat harvest
  drops, mature carrot/potato/beetroot/nether wart harvest drops, mature melon
  and pumpkin stem fruit placement, cocoa placement, mature cocoa harvest, and
  sweet berry harvest; immature crop harvest variants remain unit-covered only.
  Particles/sounds/statistics/game events, vanilla RNG/growth-rate or loot-table
  parity,
  exact soil/moisture/light growth rules beyond the existing random-tick
  sampling, poisonous potato chance, exact crop loot/RNG/fortune parity, exact
  vanilla stem fruit placement/support/RNG parity, exact sweet berry drop RNG or
  collision behavior, exact cocoa placement/support parity, exact cocoa loot/RNG
  parity, and immature crop harvest harness coverage are not claimed yet.
- Plant support semantics are local and deterministic: crop item placement checks
  exact clicked support blocks for farmland crops and soul sand for nether wart,
  plus a clear target cell, but crop growth/bonemeal/random ticks advance by block
  state and do not re-check soil, moisture, light, or survival support. Mature
  stems scan north, south, west, then east for the first air fruit cell and do not
  validate fruit support below. Cocoa placement accepts horizontal jungle-log
  faces with a clear target cell; later cocoa growth and drops do not re-check log
  support. These are scoped Solaris semantics, not vanilla support parity.
- Sugar cane, cactus, and bamboo have partial vertical-plant growth support:
  random ticks can grow supported clear columns by one block up to the local
  height-three cap, and samples from any block in the column grow only above the
  contiguous top. Sugar cane growth requires adjacent water at the base support,
  and cactus growth requires air on the horizontal sides of the new growth cell.
  Existing support-break cascade behavior is preserved. Solaris harness coverage
  exists for visible sugar cane, cactus, and bamboo random-tick growth. Vanilla
  age-counter/RNG/timing parity, exact sugar cane support-block whitelist, cactus
  damage/collision or full neighbor survivability, bamboo age/stage/leaf-size
  transitions, bonemeal, particles/sounds/statistics/game events are not claimed
  yet.
- Vertical-plant growth support is intentionally coarse: the bottom of the
  contiguous column must have any non-air support block, sugar cane additionally
  requires adjacent base water, the cell above the top must be air, cactus growth
  requires clear horizontal sides around the new cell, and the local height cap is
  three blocks. Support-break cascades remove sugar cane, cactus, and bamboo
  blocks above a removed support. Exact sugar cane support-block whitelist, full
  cactus side-neighbor survival after arbitrary neighbor edits, and
  bamboo-specific age/stage/leaf support semantics are not implemented.
- Common one-by-one saplings have partial tree-growth support: using bonemeal on
  a clear oak, birch, spruce, jungle, acacia, or dark oak sapling creates a
  deterministic Solaris-owned small tree through existing block-edit paths, and
  random ticks can grow the same tree without item consumption. Matching
  log/leaves/air states are resolved from the loaded block registry, and bonemeal
  is consumed only after successful edits; a Solaris harness covers the oak
  bonemeal survival path. Vanilla tree-feature parity, special/two-by-two trees,
  particles/sounds/statistics/game events, and vanilla random shape variation are
  not claimed yet.
- Bows can launch basic arrow projectile entities with local physics, block stop,
  lifetime despawn, grounded pickup, simple entity/player-hit damage, owner
  self-hit safety, entity knockback, and configured mob drops/XP on lethal hits;
  full vanilla combat parity is not claimed.
- Shields have partial runtime support with Solaris harness coverage: using a
  main-hand or offhand shield can block frontal mob melee and arrow player damage
  after a short activation delay; durability damage, axe disable behavior,
  sounds/particles, shield pose metadata, exact vanilla angle/timing, broader
  projectile/damage-source parity, and full vanilla parity are not claimed.

## Not Claimed

- Other dimensions, portals, structures, full biome parity, villages/trading,
  redstone parity, full vanilla loot-table parity, full bow/arrow combat parity,
  full shield parity, boats, minecarts, full recipe-book parity, weather parity,
  or M39 lock-free ownership hardening.

## Manual Gate

Run a PrismLauncher session against `cargo run --bin mc-server -- --config
example.toml` and cover:

1. Join/rejoin.
2. Exploration/chunk streaming.
3. Mining and block placement.
4. Doors, trapdoors, buckets, signs, and beds.
5. Crafting table, chest, barrel, furnace, smoker, blast furnace.
6. Farming and fluids.
7. Mobs, combat, death, respawn, drops, XP.
8. Save/restart persistence.
9. Two-client visibility, edits, pickups, containers, and reconnects.

Record any client disconnect, visual desync, missing packet, panic, or protocol
decode error back into the next milestone plan.
