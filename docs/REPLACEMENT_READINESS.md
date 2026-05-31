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
- Crafting table, chest, barrel-style storage, furnace, smoker, and blast furnace
  using the current furnace storage/cooking model.
- Recipe loading accepts furnace, blasting, smoking, and campfire variants;
  furnace runtime routes furnace, blast furnace, and smoker categories.
- Basic farming, fluids, mobs/combat, death/respawn, XP, drops, inventory, and
  multiplayer visibility/pickups.
- Runtime metrics and budgets from M37 for debug-load visibility.

## Partial Scope

- Beds set respawn points; sleeping and time-skip are not implemented.
- Signs place; sign text editing is deferred until the serverbound packet is
  verified from the local vanilla oracle.
- Smokers/blast furnaces use furnace behavior; faster/specialized cooking is not
  claimed.
- Barrels use chest storage; barrel animation parity is not claimed.
- Campfire cooking has usable partial support with Solaris harness coverage for
  consuming matching held inputs and dropping cooked results, but visual
  item-on-campfire metadata, in-flight persistence across restart, smoke/sound
  particles, exact ejection vector parity, and hopper/comparator automation are
  not claimed.
- Farming has partial crop growth, bonemeal, wheat harvest drop support, carrot
  harvest drop support, potato harvest drop support, beetroot harvest drop
  support, and nether wart harvest drop support:
  wheat, carrots, potatoes, beetroots, nether wart, melon stems, pumpkin stems,
  and sweet berry bushes can advance one age at a time through random ticks or
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
  wart dropping nether wart x1. Crop harvest item ids are resolved through the
  loaded item registry and missing ids are omitted safely. Solaris harness
  coverage exists for young wheat growth, mature wheat no-consume behavior, and
  mature wheat harvest drops; young wheat, carrot harvest, potato harvest,
  beetroot harvest, and nether wart harvest remain unit-covered only.
  Particles/sounds/statistics/game events, vanilla RNG/growth-rate or loot-table
  parity,
  exact soil/moisture/light growth rules beyond the existing random-tick
  sampling, poisonous potato chance, exact crop loot/RNG/fortune parity, and
  stem fruit placement/conversion, sweet berry harvesting/collision behavior,
  all-crop harness coverage are not claimed yet.
- Sugar cane and cactus have partial vertical-plant growth support: random ticks
  can grow supported clear columns by one block up to height three, and samples
  from any block in the column grow only above the contiguous top. Existing
  support-break cascade behavior is preserved. Vanilla age-counter/RNG/timing
  parity, exact sugar cane water adjacency or cactus neighbor survivability,
  bonemeal, particles/sounds/statistics/game events, and harness coverage for
  random-tick growth are not claimed yet.
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
  redstone parity, full bow/arrow combat parity, full shield parity, boats,
  minecarts, full recipe-book parity, weather parity, or M39 lock-free ownership
  hardening.

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
