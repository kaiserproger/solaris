# Replacement Readiness

Solaris can be evaluated as a scoped vanilla 26.1.2 overworld-survival server,
not as a bit-perfect vanilla clone.

Replacement readiness is a stabilization/release claim, not a draft
implementation claim. Use the hard DoD in
[DEFINITION_OF_DONE.md](DEFINITION_OF_DONE.md): scoped mechanics need at
least 80% vanilla-observable coverage, documented divergences for the
rest, cargo/harness/oracle/client evidence, and measured performance or
concurrency behavior where relevant.

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

## M77 Ledger Summary

M77 freezes the M100 denominator in
[VALIDATION_LEDGER.md](VALIDATION_LEDGER.md). This document no longer keeps a
broad `Ready Scope` list: broad login, chunk, lighting, block, farming, loot,
combat, persistence, multiplayer, performance, or ops claims remain
`stabilization` claims until the exact ledger row has normal-path runtime tests
plus vanilla oracle or real-client evidence.

## Evidence-Backed Partial Scope

- Login/config/play, chunk streaming, lighting, block edits, persistence,
  inventory, containers, mobs/combat, drops, XP, and multiplayer visibility have
  focused implementation and test coverage from earlier milestones, but still
  require current oracle/client and soak evidence before they count as broad
  M100 coverage.
- Beds set respawn points. A bounded draft sleep path skips a lone player from
  Solaris-defined night to the next morning through the existing `SetTime` path,
  but client-visible clock-map/time-sync behavior and sleep skip save/restart
  persistence are not proven. Multiplayer sleep quorum, weather, and
  client-visible bed animation parity are not claimed. Weather is currently
  disabled: Solaris does not start rain/thunder or send weather transitions.
- Signs support regular plain-text editing through Solaris harness coverage;
  hanging signs, styled/filtered/clickable text, waxed semantics,
  sounds/statistics/game events, and visual/manual parity are not claimed.
- Crafting table, chest, barrel, furnace, smoker, and blast furnace paths have
  partial container/menu support. Barrels use single-container storage;
  smokers and blast furnaces share the furnace-family runtime. Barrel animation,
  sounds/events, furnace lit-state, hopper/comparator automation, and exact
  state-machine parity are not claimed.
- Recipe loading and runtime support are partial for furnace, blasting, smoking,
  and campfire variants. Recipe-book/window sync and broader recipe execution
  need M80/M87 validation.
- Campfire cooking has usable partial support for held-input cooking, visible
  `Items` block-entity updates, cooked item drops, and pickup. In-flight
  persistence, automation, smoke/sound/particles, and exact ejection behavior are
  not claimed.
- Loot/drop support is scoped to deterministic one-stack block/entity drops from
  embedded fallback data or a configured local sidecar simple subset. Full
  vanilla loot execution, random rolls, predicates, fortune/silk touch, looting,
  equipment drops, exact XP, sounds/particles/statistics/events are not claimed.
- Farming and plants have partial deterministic Solaris support for common crops,
  nether wart, stems, sweet berries, cocoa, saplings, sugar cane, cactus, and
  bamboo. Exact vanilla RNG, soil/moisture/light/support survivability, loot RNG,
  collision/damage, particles/sounds/statistics/events, and several
  harness/client/oracle paths remain open.
- Bows/arrows and shields have partial local combat behavior. Exact projectile
  physics/metadata, durability, axe-disable, shield pose/timing/angle,
  attribution, effects, and broader damage-source parity are not claimed.
- M37/M42 provide earlier metrics and lock-pressure evidence, but the M100
  low-end/balanced/high-end performance, concurrency, and autoscale gates remain
  non-green. M77 generated-world validation proved functional spawn-window
  generation/streaming, but also exposed a performance blocker: the
  view-distance-8 live stream emitted all 289 chunks only after 17.3s and logged
  repeated tick-budget plus `chunk_prepare` lock warnings.

## Blocked Or Unknown For M100

- Real-client automation/manual evidence is blocked until M78/M94 produce an
  approved real vanilla 26.1.2 client gate.
- Systematic vanilla oracle scenarios are blocked until M79 inventories and
  promotes usable captures. Protocol bots remain harness evidence, not client
  evidence.
- Water/swim feel and the M40/M41 frozen-world/manual regression route remain
  blocked until rerun green or explicitly reclassified.
- Online-mode/session auth, public-safety defaults, duplicate-name handling,
  permissions, chat policy, and malformed-action fail-closed behavior need M89.
- Boats, minecarts, common survival stations beyond the currently supported
  crafting/furnace-family paths, and redstone-lite automation need M81/M83/M87
  decisions or implementation.
- Crash recovery, LZ4 Anvil compression, persisted light arrays, dropped-item
  lifecycle, long multiplayer soak, and autoscale behavior need M88-M96 evidence.
- Generated-world join/chunk streaming must be fixed or explicitly carried as
  non-release-ready debt before M100: no unexplained 150ms+ runtime ticks,
  `chunk_prepare`/`save_all_flush` warnings, or full-window latency misses.

## Accepted Non-Goals And Divergences

- Other dimensions, Nether/End parity, full portal travel, structures, villages,
  trading, full biome parity, modded clients, plugin APIs, custom datapacks
  beyond the local sidecar, and resource-pack-specific behavior are outside the
  M100 core MVP unless a later ADR changes scope. Solaris now prefers the
  `minecraft:overworld` dimension type when present and otherwise falls back to
  the first configured dimension for degraded/test data; it does not implement
  portal transfer.
- Full redstone computer parity, pistons, observers, quasi-connectivity, and
  exact redstone update order are non-goals for M81 redstone-lite.
- Full vanilla worldgen and bit-perfect loot/RNG/sounds/particles/statistics/game
  events are accepted divergences unless M95 identifies a specific omission as a
  normal-survival blocker.
- Transparent shared-world horizontal sharding is not part of autoscale scope.

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

Until a real-client automation path exists, this gate is owner-run by
default. An MCP server or equivalent harness may take it over only if it
drives a real vanilla 26.1.2 client and records reproducible client-side
observations, not just server logs or protocol-bot success.
