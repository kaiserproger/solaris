# Playable Loop

Goal: make Solaris feel playable for one 20-minute vanilla-client session.

Default manual profile: `playable.toml`.

Read this file once, then use `ACTIVE.md` as the **only mutable source of
truth** for playable status, the current checkpoint, recent evidence, and the
next action. Do not duplicate playable progress into `docs/spark-team/`,
milestones, readiness ledgers, `docs/NEXT_SESSION.md`, or archive files.
`docs/spark-team/` is campaign machinery and must not be changed or committed as
part of ordinary playable checkpoints.

Do not read `docs/NEXT_SESSION.md`, `docs/VALIDATION_LEDGER.md`,
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

- Use one CodeGraph call for symbols, callers/callees, mutation paths, and
  blast radius when available.
- Use bounded `rg`/reads for docs, config, logs, generated artifacts, and stale
  files. Do not run both discovery paths by default.
- Good CodeGraph questions: callers of block mutation, inventory truth,
  chunk-send/light-update emitters, world/session lock holders, and blast
  radius of chunk streaming changes.
- Refresh the index with `codegraph sync .` after edits before relying on
  graph answers.
