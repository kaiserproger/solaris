# Owner Playtest Queue - 2026-07-22

This is the durable queue from the owner's manual O3 survival session. It is
evidence, not a claim that an item is fixed. Update an item only after focused
verification and keep ordinary survival play ahead of rare edge cases.

## Immediate Gameplay

- [ ] Block breaking must not intermittently reject valid survival breaks.
  Evidence: stone at `x = 31` repeatedly rejected while nearby blocks broke;
  chat and movement remained live. The first proven cause is unnecessary stair
  dependency reads for non-stair-to-non-stair edits at a chunk edge. The
  chunk-edge regression is green; owner/client confirmation is still pending.
- [ ] Block placement must work on every valid face and from either hand. The
  owner transaction now accepts and debits the offhand slot, with a focused
  regression; owner/client confirmation is still pending.
- [ ] Embed exact vanilla collision shapes so torches, doors, crops, slabs,
  stairs, fences, and other non-cubes never fall back to full-cube collision.
  All 29,873 vanilla 26.1.2 states now have their context-free shape embedded in
  a validated zero-copy binary table; focused shape/physics tests and the full
  `mc-net` library suite are green. Entity-dependent overrides, notably leather
  boots walking on powder snow, still need their owning movement context.
- [ ] Breaking a block must spawn a world item drop before normal pickup rather
  than crediting the inventory directly.
- [ ] Shift-click crafting must craft the maximum complete batch that fits;
  shift-click container transfers must move the complete stack without a later
  slot teleport. Focused crafting and furnace quick-move regressions are green,
  and Lua craft events now report the complete batch; owner/client confirmation
  is still pending.
- [ ] Furnace state must visibly become lit while cooking, open promptly, and
  deliver slot/cook events without delayed bursts.
- [ ] Player melee reach and damage must match ordinary vanilla survival closely:
  a sheep at two blocks is hittable and normal mobs do not die in one bare hit.
- [ ] Hostiles must react without waiting for player movement, cannot hit from
  behind/out of reach, and stop attacking dead players.
- [ ] Skeletons fire arrows and creepers fuse/explode through authoritative paths.
- [ ] Passive and hostile mobs spawn naturally; animal movement and one-block
  stepping are smooth and vanilla-like, without hopping, circles, or stutter.
- [ ] Water has player swimming, drag, buoyancy and breathing; aquatic mobs stay
  and move naturally below the surface.

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
- [ ] Biomes need coherent, longer terrain forms instead of noisy short-scale
  height changes. Vegetation density must stay moderate and biome-appropriate.
  The existing router already uses 340-3,600 block landform scales; decoration
  density is now biome-specific, grass/flowers are sparser, and pumpkins are
  bounded below one per 256 eligible columns in the sampled regression.
- [ ] Generated plants use their exact embedded vanilla collision shape; grass,
  flowers, crops, and saplings must never become full cubes.
- [ ] Keep vanilla-compatible world load/save while producing coherent terrain
  quality comparable in intent to Tectonic/Tellus.
- [ ] Verify vanilla ore height/distribution when the vanilla ore pass is active.
- [ ] Add an optional plugin that disables the vanilla ore pass and generates
  large geological mines/deposits in the style of TerraFirmaCraft. It must be
  switchable per world without changing the default.

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
- [ ] Bound breeding and scheduled-block work per tick and move expensive
  preparation off the tick owner. No single animal or block batch may stall
  packet processing.
- [ ] Make animal and hostile movement visually alive: smooth velocity/rotation,
  varied goals, sensible pauses, obstacle stepping, and no synchronized herds,
  circular running, diagonal grid motion, or stationary jitter.
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
