# Prompt 03B Atomic Bow Release Plan

Quality label: `stabilization`.

## Scope

Commit one arrow debit, one bow durability change, and one projectile spawn in
one session-fenced simulation-owner turn. The connection retains draw/release
validation and socket writes, then mirrors only the returned inventory.

Projectile flight/hit authority is already simulation-owned. Generic combat,
damage/death, active-use start/cancel, cursor/container state, pose, and save
barriers remain separate Prompt 03B work.

## Tasks

- [x] Add RED owner conservation, stale-state, duplicate-release, bow-break,
  requester-loss, and stale-session tests.
- [x] Add a bounded `CommitBowRelease` command with exact bow and arrow slots.
- [x] Commit inventory and projectile creation under one owner turn and
  dispatch projectile visibility before responding.
- [x] Cut release handling over to the returned inventory and delete the
  production split `SpawnArrow`/local debit/durability path.
- [x] Prove exact TCP arrow debit, bow damage, ack, motion, and despawn packets.
- [x] Run focused gates, replay/soak, embedded MCP where applicable, and the
  full baseline.

## Evidence

- Five owner tests cover conservation, stale bow/arrow/session rejection,
  duplicate release, requester loss, and bow break. Inventory replacement and
  projectile creation happen under the same session-registry owner turn.
- Production release handling submits `CommitBowRelease`, mirrors the returned
  inventory, and writes returned slot changes. The separate production
  `SpawnArrow`, local arrow debit, and local bow-durability helpers are gone.
- The exact TCP test observes arrow count `3 -> 2`, bow damage `0 -> 1`, release
  ack, projectile spawn, non-zero motion, relative movement, and despawn.
- `mc-net` passes 521 library tests and `block_edit` passes 66 tests. Checked
  replay passes on two fresh Solaris servers. The short four-active-plus-one-
  paused-reader soak passes with five transaction samples.
- `cargo test --workspace`, clippy with `-D warnings`, format check, and
  `xtask code-health` pass. No dedicated bow scenario exists in the embedded
  MCP catalog, so this slice has exact wire-client evidence but no MCP bow run.
  This is stabilization evidence, not a vanilla-parity claim.
