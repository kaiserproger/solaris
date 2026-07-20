# Playable Loop

Goal: make Solaris feel playable for one 20-minute vanilla-client session.

Default manual profile: `playable.toml`.

Read this file plus `ACTIVE.md` for playable work. Do not read
`docs/NEXT_SESSION.md`, `docs/VALIDATION_LEDGER.md`,
`docs/VALIDATION_COVERAGE_AUDIT.md`, or `docs/REPLACEMENT_READINESS.md`
unless the owner explicitly asks for readiness or ledger work.

## Non-Goals

- Vanilla replacement readiness.
- M100 numerator progress.
- Full parity.
- Redstone.
- Vehicles.
- Broad plugin API expansion before the baseline loop is stable. Once common
  gameplay is green, the first production plugin slice comes before
  optimization and rare hardening.
- Autoscale.
- Broad ledger updates.

## Target Loop

1. Vanilla client joins quickly enough.
2. Player can walk/jump/collide without rage bugs.
3. Break/place common blocks works without ghost/desync.
4. Drops/pickup/inventory/hotbar work enough for wood -> tools.
5. Crafting table and basic recipes work.
6. Save/restart/rejoin preserves player, inventory, and edited blocks.
7. One 20-minute real-client session has no crash, disconnect, or
   catastrophic tick stalls.

## Allowed Cuts

- View-distance 4.
- Pregenerated, superflat, or island world.
- Peaceful only.
- Starter kit.
- Deterministic drops.
- No mobs until P0-P4 are good.
- No water unless movement/feel is good.

## Navigation

- Use `rg`/`rg --files` first.
- Use CodeGraph MCP when targeted graph context beats reading broad files.
- Good CodeGraph questions: callers of block mutation, inventory truth,
  chunk-send/light-update emitters, world/session lock holders, and blast
  radius of chunk streaming changes.
- Refresh the index with `codegraph sync .` after edits before relying on
  graph answers.
