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

## Not Claimed

- Other dimensions, portals, structures, full biome parity, villages/trading,
  redstone parity, bows/arrows, shields, boats, minecarts, full recipe-book parity,
  weather parity, or M39 lock-free ownership hardening.

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
