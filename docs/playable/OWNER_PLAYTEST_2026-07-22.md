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
- [x] Embed exact vanilla collision shapes so torches, doors, crops, slabs,
  stairs, fences, and other non-cubes never fall back to full-cube collision.
  All 29,873 vanilla 26.1.2 states now have their context-free shape embedded in
  a validated zero-copy binary table. Player movement consults that table before
  any reduced/custom-registry fallback, so torches remain empty while campfires
  use their exact 7/16-block body. Powder snow now uses the player's current
  leather boots, Shift descent, position-above-block check, and `> 2.5`-block
  fall distance through the authoritative movement-correction path. Exact state
  fingerprints prevent custom blocks from inheriting these dynamic semantics.
  Focused shape/physics tests and the full `mc-net` library suite are green.
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
- [x] `minecraft_break_block` confirms pickup from the applied inventory packet
  and counts the expected item across every player inventory slot. A focused
  real 26.1.2 client gate broke a placed stone block, observed its world drop,
  and returned `pickup_confirmed=true` when cobblestone increased from `0` to
  `1` in non-selected slot `1`; the diamond pickaxe remained selected in slot
  `0`, the block became air, and the client stayed in play.
- [x] The embedded non-operator restart pickup regression was a false-positive
  fixture: an ordinary random tick changed exposed dirt to grass and advanced
  its mutation token while the client was still mining the original dirt
  snapshot. The persistence gate now mines stable jungle logs, then verifies
  pickup, placement, shutdown save, and restored inventory; its
  exact isolated run is green.
- [x] `minecraft_use_item_on` exposes `main_hand` and `off_hand`, defaults to
  main hand, dispatches that exact vanilla interaction hand, and returns the
  local interaction result instead of unconditional `ok`. In the focused real
  26.1.2 client gate, one stone occupied offhand inventory slot `40` while the
  selected main hand was empty; `hand=off_hand` returned vanilla `Success`,
  placed the stone, consumed it from `1` to `0`, and kept the client in play.
- [x] `minecraft_respawn` waits for an applied authoritative health packet and
  a live client state instead of accepting the transient healthy player created
  by `ClientboundRespawn`. Against the original persisted dead player, the real
  26.1.2 client started at `health=0`, respawned while holding immediate forward
  input, closed the death/loading screen, restored its inventory, and reported
  positive health only after the server health update.
- [x] `minecraft_wait_for_health_below` uses strict numeric ordering without an
  epsilon-shifted boundary. The real 26.1.2 client returned `matched=true` for
  observed `health=0` and requested `health < 0.001`; equality with the threshold
  remains a non-match and the wait remains event-driven.

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
- [x] Repeat the fresh-world visual gate after the latest owner report instead
  of relying on shape/density regressions alone. Ordinary trees must have a
  raised irregular crown above the main canopy rather than a rectangular leaf
  prism; terrain forms must read as long coherent ranges; vegetation must stay
  moderate; and sampled grass, flowers, crops, saplings, torches, doors, and
  other generated decorations must use their embedded vanilla collision
  shapes. Record the seed, coordinates, screenshots/client observations, and
  exact collision samples in this file before closing the gate.
  Worldgen revision 9 removes the filled 3x3 upper oak/jungle layers. On the
  exact shipped seed-0 `tellus_like` profile, the isolated jungle tree rooted
  at `(12,85,-5)` had leaf-layer counts `19,16,17,6` at Y `88..91`; its nearest
  other trunk was ten blocks away, so the six-leaf raised crown was not an
  overlap artifact. A `31x31` spawn sample used 85 decorated columns
  (`8.84%`), and 24 client-visible grass/poppy/dandelion samples all reported
  empty collision. The complete embedded-table gates additionally sampled all
  states of seven sapling species and eight crop/stem families, both oak-door
  planes, and the runtime torch/campfire pair; none fell back to a full cube.
  A second fresh world used seed `918273645`; at
  `(-78080,215,-28928)` the client rendered a continuous long snow slope rather
  than a round hill, hole, or floating shelf. Evidence:
  `.analysis/codex-logs/worldgen-v9-visual-summary-20260723.json`,
  `.analysis/minecraft-mcp-worldgen-v9/screenshots/worldgen-v9-isolated-tree-clean.png`,
  and
  `.analysis/minecraft-mcp-worldgen-v9/screenshots/worldgen-v9-seed918-high-relief-215.png`.
- [x] Keep vanilla-compatible world load/save while producing coherent terrain
  quality comparable in intent to Tectonic/Tellus. The Anvil encoder now emits
  exactly one `DataVersion`, `LastUpdate`, and `InhabitedTime` root field for
  every chunk, including old Solaris chunks that lacked them. Production
  flushes use the actual simulation tick for `LastUpdate`; imported
  `DataVersion` and `InhabitedTime` survive save/reopen. Focused disk round-trip
  coverage and all `mc-world` tests are green. Runtime now accumulates exact
  active ticks and persists them in batches. Worldgen revision 8 removes low
  coastal shelves from the mountain-surface route, strengthens long rolling
  relief, adds a 520x210-block mountain detail field, gives high peaks snow,
  keeps low shelves in coastal/lowland biomes, and explicitly keeps the spawn
  window dry. An agent-run MCP
  route used a fresh 26.1.2 client, seed `918273645`, and `tellus_like` mode to
  inspect the forest spawn, coast, ocean, and the representative
  high-relief range around `(-78080, -28928)`: canopies tapered above their
  broad layers, terrain stayed solid, vegetation remained coherent, and the
  mountain had a continuous elongated slope instead of the original flat
  gravel plateau. The later owner screenshot exposed that revision 8 still
  filled its upper 3x3 leaf layers; revision 9 closes that narrower visual gap.
  A second agent-run MCP pass used the exact shipped `playable.toml` profile
  (`seed=0`, `tellus_like`) and found a dry, solid, moderately decorated forest
  spawn with the same raised tree crowns. The exact shipped-profile gate also
  exposed the spawn selector treating a leaf canopy as ground; spawn support
  now follows the vanilla no-leaves heightmap intent and rejects leaf blocks.
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

- [x] Ship a reusable baseline economy plugin with a configurable physical item
  currency, durable refund ledger, and atomic inventory-menu shop. This is
  `examples/plugins/basic-economy`.
- [x] Activate the economy shop from a configured zone while retaining
  `/economy` as a manual entry point. The old virtual-wallet implementation and
  duplicate `currency-catalog` fixture were removed. The consolidated plugin
  configures one currency item (emeralds, gold ingots, or another registered
  item), inclusive zone bounds, and up to 16 products. Primary clicks atomically
  remove currency, grant the product, and advance the refund ledger; secondary
  clicks reverse only recorded purchases using the original product and
  currency terms. Stable product ids make catalog reordering safe; changed
  terms require refunding prior purchases before buying again. The production
  TCP/Lua gate proves a configured gold-ingot purchase, insufficient-funds
  rejection, refund, menu refresh, and zone-triggered opening. Focused tests
  cover changed terms, the purchase-count bound, corrupt-ledger retry,
  fractional counts, duplicate product ids, invalid zone ids, and out-of-range
  bounds.
- [x] Ship baseline protection/claims with durable ownership and direct player
  break/placement policy. This is `examples/plugins/land-claims`.
- [ ] Extend claims to containers, fluids, pistons, explosions, fire, and entity
  interaction. Direct right-click block actions now cover containers, buckets,
  cauldrons, toggles, beds, campfires, tilling, planting, and TNT ignition;
  living-entity interaction is checked at the authoritative target position.
  Every chest/furnace click rechecks the backing positions, including windows
  opened before a claim was created. Explosion block planning consumes one
  immutable generic protection snapshot only after due explosions are claimed and before
  taking the world lock, so idle ticks do not copy zones, protected blocks are
  not candidates, and no per-block zone mutex is added. Piston movement and
  fire spread have no implemented mutation path yet; keep this item open until
  those mechanics exist and consume the ambient protection snapshot.
  Protection is plugin-authored through `solaris.upsert_protected_zone`; Rust
  does not match `land-claims` or parse its zone ids.
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

- [x] Remove autoscaler oscillation and repeated no-op scale-up logging; scaling
  must not disconnect clients or delay gameplay packets. The exact 5,227-entity
  O3 rerun completed 975 real-client ticks in play with no runtime work-budget
  info spam, disconnect, reliable drop, or retry. The only CPU-admission info
  transition was the requested shutdown drain.
- [ ] Investigate measured tick spikes from the playtest profile. Common spikes
  were in breeding, entity goals/dispatch, and some block-edit batches; aggregate
  mutex wait totals alone did not prove contention. Breeding state now advances
  in 20-tick batches and inline single-region scheduled block ticks no longer
  wait for a WAL append. The batched breeding pass preserves every eligible
  love window; an O3 profile rerun is still required.
  The debug deep-water fixture's scheduled-fluid backlog is closed. Fluid
  planning previously retained every intermediate transition for one block,
  recursively revisited the same unsupported-flow positions, and scheduled
  follow-up ticks one at a time. It now visits each position once, commits one
  final edit per block, batches follow-up scheduling per chunk, and keeps the
  ordinary resident path off synchronous journal `fsync`. The same loaded ocean
  settled after nine O3 batches with at most 28 final edits; `fluid_tick`
  measured `1,751 us` p50 and `3,321 us` max while the 26.1.2 client remained
  connected for 600 client ticks. Evidence is
  `.analysis/codex-logs/scheduled-fluid-coalesced-o3-summary-20260723.json`.
  The terrain visual run also exposed a separate far-travel path: a 289-chunk
  stream took about 31 seconds while dirty-cache pressure forced a 3.5-second
  flush and chunk fetch/light work accumulated behind it. That path is now
  closed. Chunk preparation no longer starts a second full dirty flush; it
  requests the server-owned save worker and waits for that exact accepted
  action or for stream-generation cancellation. The fallback path without a
  save worker is capped at eight chunks. Three fresh O3 view-distance-8 streams
  each emitted all 289 chunks in `3,195 ms`, `2,726 ms`, and `2,608 ms`;
  first chunks arrived in `74 ms`, `42 ms`, and `48 ms`. The client remained in
  play with no pressure abandonment, slow-client warning, or disconnect. A
  direct O3 shutdown then drained and saved without a timeout. Evidence:
  `.analysis/codex-logs/far-travel-o3-server-20260723.log`,
  `.analysis/codex-logs/far-travel-o3-mcp2-20260723.json`, and
  `.analysis/codex-logs/far-travel-o3-shutdown-20260723.log`.
  Explosion expiry no longer scans every entity and cannot process an unbounded
  simultaneous batch under one world lock. An O3 full-path benchmark with 4,096
  background cows, 64 due explosions, a fresh 27-block solid dirt volume for
  every explosion, and one loaded observer measured idle fuse checks at 0 us
  p99 and bounded explosion ticks at 23,812 us p50 / 37,943 us p95 / 46,463 us
  p99. One explosion is admitted per world tick even if the owner calls the
  claim path repeatedly. This closes the requested explosion benchmark; the
  separate far-travel spike remains.
  The autonomous survival profile found two ordinary block breaks spending
  `117-121 ms` after CPU admission. The regional path copied an entire 8x8-chunk
  ownership region after every break only to inspect falling blocks above the
  edited columns. It now snapshots only the chunks containing applied edits.
  The same real-client break path completed in `9.1 ms` total simulation-command
  work with no slow-command warning. The apparent `49.5 ms` breeding spike was
  separately traced to sheep grazing issuing one synchronous timer mutation per
  sheep. Grazing now uses one selected read and one conditional timer-update
  batch, while telemetry reports grazing and breeding separately. A fresh
  1,020-tick real-client run with 11-16 visible natural entities emitted no
  over-budget tick warning. Fluid, far-travel, and scheduled-block spikes remain
  open.
  The exact dense fixture then exposed three remaining full-population reads:
  AI/physics selected all 5,208 active cows, sheep grazing read every loaded
  entity before filtering type, and breeding repeatedly read idle adults.
  Autoscaler-derived fair cohorts now cap dense simulation while leaving
  ordinary populations at 20 Hz; grazing and breeding use exact maintained
  indexes. Across the same 975-client-tick gate, over-budget warnings fell from
  223 to 8. Conditional warning p50 fell from `93,123 us` to `56,775 us`,
  entity-goal p50 from `78,237 us` to `17,473 us`, and grazing p50 from
  `8,389 us` to `234 us`; breeding was at most `1 us` in an over-budget sample.
  The 26.1.2 client remained in play with no disconnect, reliable-command loss,
  or runtime work-budget info spam. This closes the dense entity-read causes;
  scheduled block/fluid work and an interactive menu/block/attack natural-load
  gate remain open.
- [x] Bound scheduled-block work per tick and move expensive preparation off
  the tick owner. The scheduled-block phase starts one bounded background job
  and services pushed simulation commands while its autoscaler-admitted worker
  plans and commits. A shared admission fence rejects overlapping entry points,
  and the tick does not advance to fluid or later phases before completion. A
  deterministic regression reserves the only CPU permit, proves an owner
  command completes before release, rejects duplicate admission, and then
  commits all 256 due button ticks. The optimized `-O3` gate applied the batch
  in `1,666 us`. This proves simulation-owner responsiveness, single authority,
  and same-tick phase ordering. The representative interactive natural-load
  client gate is closed below.
- [x] Make animal and hostile movement visually alive. Ground wanderers now
  retain a deterministic 3-7-block destination until arrival, pause for a
  per-entity interval, and choose independent destinations instead of replacing
  one-block targets on a shared period. Moving goals turn body and head at
  bounded rates; collision resolution preserves that goal rotation instead of
  snapping it to the clipped velocity. Animals in love follow a nearby mate
  during courtship and return to wandering after breeding; zero-speed hostile
  melee still faces its target immediately. Deterministic tests cover
  independent multiblock targets, retained paths, pause without drift or
  rotation jitter, bounded turning, explicit hostile facing, courtship,
  obstacle pathing, full-block livestock climbs, exhausted-path retargeting,
  and old saved wander state. The formerly failing cow and chicken wire
  breeding tests now pass. In an embedded 26.1.2 client gate, a separately
  identified natural sheep, pig, and cow each produced pushed motion with
  `0.34-0.39`-block horizontal samples, non-zero yaw changes, and no vertical
  rise. That client sample confirms publication, while the deterministic tests
  establish independent timing, pauses, and turn limits. The server emitted no
  tick-budget or disconnect warning during the gate.
- [x] Extend entity collision context beyond players: powder-snow-walkable mob
  tags and falling blocks must use their vanilla dynamic powder-snow shape
  without weakening the exact-state fingerprint fence. Physics queries now
  distinguish the exact 26.1.2 walkable tag (rabbit, fox, silverfish, and
  endermite) and falling blocks. Tagged mobs receive full support only while
  above powder snow; short-falling blocks receive the block's full base shape;
  ordinary entities keep sinking. All entities whose accumulated fall distance
  exceeds 2.5 blocks use the earlier vanilla 0.9F landing shape. Fall distance
  is retained by the ECS physics state and reset on landing. The dynamic branch
  runs only after the embedded shape table accepts the exact block-state
  fingerprint, and a mismatched custom state retains the conservative
  full-cube fallback. Deterministic tests cover tag routing, query projection,
  fall accumulation/reset, above/inside behavior, both falling-block branches,
  long-falling ordinary mobs, and the fingerprint mismatch.
- [x] Keep menus, inventory actions, block events, and attacks responsive under
  natural mob/chunk load. The biased entity-owner select used to check an
  overdue 50 ms tick before its pushed command notification, allowing sustained
  over-budget ticks to starve player transactions. Command readiness now wins
  before the ticker. Dense real-client movement remains connected after the
  fair-cohort and exact-index fixes above. A fresh agent-run 26.1.2
  `playable-12` gate on the optimized dev profile then completed three natural
  block breaks with visible drops, maximum-count inventory crafting,
  crafting-table placement/opening, natural pig combat and pickup, chest
  placement/opening, and a normal container transfer in 22 seconds. The world
  had nine natural entities and streamed new chunk rings during the run. The
  client stayed in play, every action converged, and the server emitted no
  tick-budget, packet-dispatch, reliable-command, or disconnect warning. This
  closes the representative ordinary-play responsiveness smoke gate; it is not
  a per-action latency SLO or a broad overload soak. Evidence is
  `.analysis/real-client-runs/responsiveness-o3/20260723T103459Z-real-client-playable-loop-4vVxYV`.
- [x] Make an autonomous MCP survival pass reliably reach the first crafting
  table without operator commands or deterministic scenarios. A fresh real
  26.1.2 client found a natural jungle log, navigated to it, mined and collected
  it, crafted planks and a crafting table through ordinary container clicks,
  placed the table on observed clear ground, and opened its crafting screen at
  full health. The first attempt exposed the decision agent repeatedly calling
  `minecraft_connect` while already in play, which created duplicate login
  attempts and made the later disconnect look like a respawn/navigation
  failure. Same-address reconnect is now an idempotent no-op; switching servers
  requires an explicit disconnect. A clean agent rerun completed the same
  natural wood-to-table path without operator setup. The subsequent unscripted
  run advanced authoritative `game_time` from `1581` to `25652`, crafted through
  a stone pickaxe, explored and mined, survived one deliberate
  disconnect/reconnect, and had no crash, timeout, reliable-command drop, or
  reproducible gameplay blocker. Six natural deaths were respawned through the
  ordinary client path; one narrow-shaft entrapment death was not reproduced
  and remains a watch item rather than a closed movement claim.
- [x] Accumulate vanilla-style per-chunk `InhabitedTime` while players keep a
  chunk active. The tick owner uses vanilla's strict 128-block chunk-center
  range around non-spectator players, counts each spawning chunk once per game
  tick, batches resident mutations every 20 ticks, and flushes an inactive
  chunk immediately. Missing resident chunks retain their delta for retry and
  are loaded without generation during shutdown. Focused coverage proves
  short-lived chunks receive only their active ticks and that accumulated time
  survives Anvil flush/reopen.
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

The 2026-07-23 sheep-grazing checkpoint passed all 1,707 `mc-net` tests,
workspace clippy with warnings denied, formatting, and `xtask code-health`.
Independent review requested stale all-or-nothing timer coverage and an exact
tick-attribution assertion; both were added and passed. The workspace test run
reached an unrelated existing persistence harness failure:
`place_dirt_persists_through_flush_to_disk` consumed a block update for
`(0,84,0)` while waiting for `(0,85,0)`. The grazing-focused and crate-wide
gates remain green; this is not recorded as a green workspace-test gate.

The 2026-07-22 tree/decorations checkpoint passed the full workspace test suite,
workspace clippy with warnings denied, the external worldgen harness,
formatting, and `xtask code-health`; independent review reported no findings.
The revision-8 fresh-world client visual pass is recorded in the completed
broader terrain item above.

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
2 persists the ore profile and the current worldgen revision fences
mixed-profile worlds.
