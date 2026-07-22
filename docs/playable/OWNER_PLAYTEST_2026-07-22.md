# Owner Playtest Queue - 2026-07-22

This is the durable queue from the owner's manual O3 survival session. It is
evidence, not a claim that an item is fixed. Update an item only after focused
verification and keep ordinary survival play ahead of rare edge cases.

## Immediate Gameplay

- [x] Block breaking must not intermittently reject valid survival breaks.
  Evidence: stone at `x = 31` repeatedly rejected while nearby blocks broke;
  chat and movement remained live. The first proven cause is unnecessary stair
  dependency reads for non-stair-to-non-stair edits at a chunk edge. A second
  proven cause was an early `STOP` becoming permanently orphaned when another
  delayed break already occupied the single delayed slot. The older break now
  promotes the queued stop on either completion or cancellation. Both focused
  state-machine regressions and the full `mc-net` library suite are green. A
  real 26.1.2 MCP client then broke eight consecutive prepared stone blocks at
  `x=12..19`, crossing the chunk boundary at `15/16`; every first attempt became
  air, exposed its item entity, and the final inventory contained exactly eight
  cobblestone without a disconnect.
- [x] Block placement must work on every valid face and from either hand. The
  owner transaction accepts and debits the packet-selected hand, and focused
  coverage routes ordinary placement through all six clicked faces and places
  from the offhand against an east face. A real 26.1.2 client then placed stone
  upward from the main hand (`64 -> 63`), upward from the offhand through the
  ordinary vanilla use input (`63 -> 62`), and to the east side (`x+1`) from
  the main hand (`62 -> 61`); all three authoritative block updates reached
  the client without a disconnect.
- [ ] Embed exact vanilla collision shapes so torches, doors, crops, slabs,
  stairs, fences, and other non-cubes never fall back to full-cube collision.
  All 29,873 vanilla 26.1.2 states now have their context-free shape embedded in
  a validated zero-copy binary table; focused shape/physics tests and the full
  `mc-net` library suite are green. Entity-dependent overrides, notably leather
  boots walking on powder snow, still need their owning movement context.
- [x] Breaking a block must spawn a world item drop before normal pickup rather
  than crediting the inventory directly. The authoritative break transaction
  creates a persisted item entity without crediting the mined stack to player
  inventory. The focused TCP gate requires block update/ack and tool damage,
  then `AddEntity` plus item metadata, at least a 100 ms visible window, and
  only then the separate pickup claim, take/remove packets, and inventory slot
  update.
- [x] Shift-click crafting must craft the maximum complete batch that fits;
  shift-click container transfers must move the complete stack without a later
  slot teleport. Focused crafting and furnace quick-move regressions are green,
  and Lua craft events report the complete batch. A real 26.1.2 client used one
  shift-click to turn four logs into 16 planks in both the inventory 2x2 grid
  and a crafting table, consuming each grid completely. It also moved one
  61-item stone stack into and back out of a chest. Reopening the crafting table
  and chest kept the final authoritative slots with no delayed correction or
  disconnect.
- [x] Furnace state must visibly become lit while cooking, open promptly, and
  deliver slot/cook events without delayed bursts. The furnace tick now commits
  its block state and block entity atomically, publishes the `lit` block delta,
  and retains baked light as the input to incremental relighting instead of
  forcing a full chunk relight. A real 26.1.2 client opened the menu in 75 ms,
  observed `lit=true` with block light 13, and received cooked porkchop through
  an event-driven exact-slot wait. The rerun produced no over-budget tick.
- [x] Player melee reach and damage must match ordinary vanilla survival closely:
  a sheep at two blocks is hittable and normal mobs do not die in one bare hit.
  Focused geometry covers the exact two-block sheep path and the existing far
  rejection boundary. An embedded real 26.1.2 client attacked with an empty
  selected slot at full strength, received an authoritative motion update, and
  observed the sheep remain alive at `7/8` health after exactly `1.0` damage.
- [x] Hostiles must react without waiting for player movement, cannot hit while
  facing away or from out of reach, and stop attacking dead players. Melee now
  checks the attacker's current head direction during both planning and final
  commit, alongside the existing current distance, visibility, life, and target
  state fences. Focused tests cover a stationary target, facing-away rejection,
  movement out of range, death before planning, and death during commit. An
  embedded 26.1.2 client issued no movement input: the initial summoned-zombie
  observation had yaw `0`; the post-damage observation had yaw `-180` and the
  unchanged-position player had gone `20 -> 17` without player motion.
- [x] Skeletons fire arrows and creepers fuse/explode through authoritative
  paths. Current focused gates prove a skeleton-owned moving arrow, a 30-tick
  creeper fuse with authoritative explosion/removal, and explosion damage over
  TCP. The retained embedded-client combat run observed the arrow and player
  damage, then observed the creeper damage the player and disappear. Operator
  commands only created the deterministic fixtures; they did not perform the
  attacks.
- [x] Passive and hostile mobs spawn naturally; baseline animal motion publishes
  continuously and livestock has one-block stepping physics.
  In a fresh generated world, an embedded 26.1.2 client saw seven naturally
  spawned pigs/sheep, then received five pushed sheep-motion events with stable
  speed, varied yaw, no vertical hopping, and `0.85` blocks of net travel. After
  the server console changed only the time to night, the fresh world exposed a
  naturally spawned moving zombie `20.3` blocks away; no entity summon command
  was used. Focused physics covers full-block climbs for cows, sheep, and
  chickens, while session tests cover every-tick publication for bounded
  natural passive and hostile movement. Visible movement quality over longer
  play remains the separate unchecked movement item below.
- [x] Water has player swimming, drag, buoyancy and breathing; aquatic mobs stay
  and move naturally below the surface. The retained representative O3 26.1.2
  client gate
  proves ascent, diving, a `3.43`-block swimming pass, air depletion and
  `20 -> 18` drowning damage while connected. A command-spawned aquatic mob no
  longer starts with an idle goal: it receives the same three-dimensional
  aquatic wander used by natural spawns across the supported aquatic and
  amphibious classes. In the corrected representative debug client gate, a
  tropical fish produced eight pushed motion events, moved `0.36` blocks, and
  remained underwater at `y=62.50..62.57`.
- [ ] `minecraft_break_block` must confirm pickup in any inventory slot and
  return on the inventory event. The chunk-edge client gate collected all eight
  cobblestone into a non-selected slot, but every call returned
  `pickup_confirmed=false` even though the final inventory was correct.
- [ ] `minecraft_use_item_on` must expose the hand to exercise or perform the
  same main-then-offhand dispatch as ordinary vanilla use input. It currently
  returned `ok` for an offhand-only stack but left its count and target block
  unchanged; `minecraft_press_inputs(use)` then performed the vanilla fallback
  and placed the block.

## World Generation

- [x] Replace the current broken terrain pass: no floating trees, giant terrain
  holes, excessive pumpkins, or random-seed water spawns. Stable tree placement,
  rare-pumpkin density, a 32-block solid surface shell, and sparse caves without
  shafts or chambers are covered across a widened seed/coordinate grid. A real
  TCP login against five fresh extreme/dispersed seeds also verifies dry support
  and clear dry body cells at the server-selected spawn.
- [x] Trees need species-specific vanilla-like silhouettes instead of rectangular
  leaf prisms. Oak-like trees need an irregular crown above the main canopy and
  must never leave unsupported floating foliage/trunks after generation. Oak,
  birch, spruce, and jungle profiles now have distinct tapered layers, hashed
  edge variation, stable support, and generated-shape regressions.
- [x] Biomes need coherent, longer terrain forms instead of noisy short-scale
  height changes. Vegetation density must stay moderate and biome-appropriate.
  Continental, erosion, mountain, and river fields use 610-3,600 block scales;
  rolling hills now use a rotated 720x280-block field and weaker 190-block
  detail. Behavior checks require regional change to dominate half-chunk noise,
  while actual generated vegetation remains below 12.5% of eligible columns.
  Tree, grass, and flower density is biome-specific, and pumpkins remain below
  one per 256 eligible columns.
- [x] Generated plants use their exact embedded vanilla collision shape; grass,
  flowers, crops, and saplings must never become full cubes. The complete
  embedded 26.1.2 state identities for generated/growable plants are checked
  against the collision oracle, including every age/stage state; the runtime
  physics sampler also reproduces an exact partial pitcher-crop shape instead
  of its full-block fallback.
- [ ] Keep vanilla-compatible world load/save while producing coherent terrain
  quality comparable in intent to Tectonic/Tellus.
- [x] Verify vanilla ore height/distribution when the vanilla ore pass is active.
  The default embedded pass now preserves all 18 relevant vanilla 26.1.2
  placed/configured ore facts: separate passes, height anchors, uniform versus
  trapezoid distribution, count/rarity, vein size, air-discard chance, and
  exact emerald/badlands biome scopes. Generated-chunk regressions enforce
  family height bands, bottom-heavy diamond/redstone, and ordinary iron
  availability at branch-mining heights. Solaris still uses deterministic
  connected vein geometry rather than claiming byte-identical vanilla chunk
  RNG.
- [x] Add an optional plugin that disables the vanilla ore pass and generates
  large geological mines/deposits in the style of TerraFirmaCraft. The shipped
  `examples/plugins/geological-mines` manifest selects deterministic elongated
  cross-chunk deposits before pre-generation. No plugin keeps the vanilla
  profile; the selected profile is persisted per world and changing it on an
  existing world is rejected instead of mixing chunk authorities.

## Plugins

- [x] Ship a reusable baseline economy plugin with durable balances and an
  atomic inventory-menu shop. This is `examples/plugins/basic-economy`.
- [ ] Extend economy with configurable item currency and zone activation.
- [x] Ship baseline protection/claims with durable ownership and direct player
  break/placement policy. This is `examples/plugins/land-claims`.
- [ ] Extend claims to containers, fluids, pistons, explosions, fire, and entity
  interaction.
- [ ] Keep plugin APIs region-aware without exposing locks or requiring plugin
  authors to reason about worker ownership.
- [ ] Prototype villages from extracted vanilla structure/template data, with
  stable Lua hooks for settlement generation, inhabitants, buildings, jobs, and
  plugin-owned extensions.
- [ ] Design and prototype Solaris Loader for Fabric, NeoForge, and Forge. A
  server/plugin manifest must be able to supply client-side blocks, items,
  screens, assets, and interactions so rich plugins are not limited to inventory
  GUI or vanilla-block substitutions. Treat downloaded content as untrusted,
  versioned, permissioned, and cacheable.

## Runtime And Distribution

- [ ] Remove autoscaler oscillation and repeated no-op scale-up logging; scaling
  must not disconnect clients or delay gameplay packets.
- [ ] Investigate measured tick spikes from the playtest profile. Common spikes
  were in breeding, entity goals/dispatch, and some block-edit batches; aggregate
  mutex wait totals alone did not prove contention. Breeding state now advances
  in 20-tick batches and inline single-region scheduled block ticks no longer
  wait for a WAL append. The batched breeding pass preserves every eligible
  love window; an O3 profile rerun is still required.
  A debug deep-water fixture also exposed a scheduled-fluid backlog applying
  roughly 94-105 updates in `68-76 ms` per tick and delaying client entry for
  more than two minutes; profile and bound that common loaded-ocean path.
- [ ] Bound breeding and scheduled-block work per tick and move expensive
  preparation off the tick owner. No single animal or block batch may stall
  packet processing.
- [ ] Make animal and hostile movement visually alive: smooth velocity/rotation,
  varied goals, sensible pauses, obstacle stepping, and no unprompted hopping,
  synchronized herds, circular running, diagonal grid motion, or stationary
  jitter.
- [ ] Keep menus, inventory actions, block events, and attacks responsive under
  natural mob/chunk load.
- [ ] Add concise comments to shipped TOML options explaining their effect.
- [ ] A runnable build must not require copying `data/vanilla`; required runtime
  tables belong in compact embedded binary data, decoded with minimal memory.
- [ ] Lower practical CPU/RAM requirements and validate a weak-machine profile
  after gameplay correctness is restored.

## Verification Order

1. Focused unit/integration regression for the exact bug.
2. Debug server plus owner or MCP real-client reproduction of the same path.
3. One O3 survival run with profiling and natural load.
4. Full repository gates once the feature batch is complete.

The 2026-07-22 survival-fix checkpoint passed the full workspace test suite,
workspace clippy with warnings denied, formatting, and `xtask code-health`.

The 2026-07-22 tree/decorations checkpoint passed the full workspace test suite,
workspace clippy with warnings denied, the external worldgen harness,
formatting, and `xtask code-health`; independent review reported no findings. A
fresh-world client visual pass remains part of the broader terrain checkpoint.

The 2026-07-22 surface/spawn checkpoint passed all `mc-worldgen` tests and the
external worldgen server harness. The harness starts a fresh generated server,
logs in over TCP, and verifies the actual selected spawn across extreme and
dispersed seeds. Independent review requested explicit collision/hazard checks
for spawn cells and an exact cave-cutoff definition; both are covered in the
final regressions, followed by full repository gates.

The 2026-07-22 biome-coherence checkpoint replaced round short-scale hills with
rotated elongated relief, reduced fine detail, and made vegetation density
biome-specific. Focused behavior checks cover the isolated rolling-hill signal,
actual per-biome generated surface density, and runtime exact plant collision
shapes. A visual fresh-world client pass remains required by the broader
Tellus/Tectonic-quality item rather than being inferred from numeric tests.

The 2026-07-23 vanilla-ore checkpoint replaced nine merged approximate families
with 18 independent embedded 26.1.2 passes. Local extracted data verifies every
anchor, placement kind, attempt/rarity ratio, size, discard chance and target;
the same oracle checks exact emerald and extra-gold biome lists. Runtime
generation checks the resulting bands and deep peaks without requiring
`data/vanilla` at startup.

The 2026-07-23 geological-mines checkpoint added a startup-only worldgen
declaration to the plugin manifest. Plugin discovery is prepared once before
the world contract and reused by the Lua host. The geological profile empties
the vanilla ore rules and creates deterministic elongated deposits spanning
chunk boundaries; the default profile remains unchanged. World contract schema
2 persists the ore profile and worldgen revision 7 fences mixed-profile worlds.
