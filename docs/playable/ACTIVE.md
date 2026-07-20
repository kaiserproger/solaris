# Active Playable Task

This file contains only the current playable queue and recent evidence. The
previous detailed log is preserved in
[`../archive/status/2026-07-19-playable-active.md`](../archive/status/2026-07-19-playable-active.md)
for targeted lookup.

## Target

Keep the normal 26.1.2 client stable through a useful survival session, then
broaden the loop beyond wood -> tools -> restart. Optimize for common gameplay,
multiplayer correctness, and visible failures before rare parity edges.

The baseline loop remains:

```text
join -> move -> gather -> craft -> build -> fight/farm -> save/rejoin
```

This is Playable Spike Mode. Do not turn focused playable evidence into M100
replacement-readiness claims.

## Current Queue

1. Make P44 select or observe livestock that actually encounters a one-block
   rise, then rerun it. Do not treat flat-terrain wandering as a climb failure.
2. Replace the debug-seeded P47 stonecutter setup with earned survival input,
   then add rejected invalid input. The existing run proves the menu path only.
3. Keep terrain navigation and item pickup stable across farmland, fences,
   slabs, stairs, one-block rises, and chunk boundaries without guessed jumps.
4. Add the next high-value survival content slice only after the above red
   client-visible paths are understood.

## Recent Evidence

- Checkpoint `feba79a` passes full workspace tests, workspace all-target strict
  Clippy, fmt, code-health `0 fail / KEEP`, and diff-check. The `block_edit`
  target also passes both parallel and sequential runs with 94/94 tests.
- Ordinary wall torches have registry-backed tests for four horizontal facings,
  standing `UP`, rejected `DOWN`, and partial support. Raw TCP proves one debit
  after accepted update/ack and unchanged held-stack resync before rejected ack.
- Stair facing/half, slab top/bottom, matching-slab merge, and waterlogging use
  the inspected local 26.1.2 rule. Stair neighbour-shape selection remains open.
- The regional mutation extraction is architecture-only and makes no gameplay
  or performance claim.
- Latest P44 artifact is
  `.analysis/real-client-runs/20260720T120018Z-real-client-playable-loop-UJtsgc`.
  Sheep and chicken passed, including chicken yaw. The selected cow moved 2.69
  blocks on a flat Y=78 surface but never encountered a rise, so the scenario
  failed its 0.8-block climb condition. The prior artifact
  `.analysis/real-client-runs/20260718T111008Z-m94-regression-pack-4X1iF7`
  already observed a real-client cow climb of 1.0 block. P44 therefore needs a
  deterministic climb candidate before another run can be strong evidence.
- The unrestricted P44 run exposed a 3.47-second entity-journal checkpoint
  stall. The journal now releases the regional commit mutex after queueing the
  ordered replacement and before waiting for disk completion; the focused
  durable-mutation concurrency regression passes. This is not a broad
  performance claim.
- P47 artifact
  `.analysis/real-client-runs/20260720T122329Z-real-client-playable-loop-Dbzfoj`
  passed stonecutter placement, menu open, normal take (1 input to 2 slabs),
  close/reopen conservation, and shift-click (3 inputs to 6 slabs). The outer
  runner returned degraded because startup emitted 350 ms and 52 ms slow-tick
  warnings; the client scenario itself exited 0. Its setup used three
  `giveAndSelect` debug commands, so this is real-client wire/menu evidence,
  not earned-survival gameplay evidence. Earned setup and rejected invalid
  input remain open.
- P48 artifact
  `.analysis/real-client-runs/20260720T124754Z-real-client-playable-loop-l8eWbc`
  passed the no-debug, no-op real-client building scenario. The client earned
  wood, stone, a furnace, charcoal, torches, and matching planks; crafted
  stairs and slabs; then proved wall-torch facing, stair facing/half, bottom
  and top slabs, matching-slab merge, exact inventory debits, and rejected
  bottom-slab wall-torch support without a debit. The scenario and driver
  exited successfully and produced a valid screenshot. The outer gate remains
  degraded because `server.log` contains slow-tick warnings, including one
  342 ms entity-physics tick and one 271 ms entity-goals tick, so this is
  gameplay evidence rather than a clean combined gameplay/performance gate.

## Manual And Agent Gates

Default playable server:

```sh
cargo run --bin mc-server -- --config playable.toml
```

Use the embedded client MCP for reproducible agent-run observations when the
scenario exists. Record whether a result is owner-run, agent-run, prepared
only, or not run. Screenshots may support a visual finding, but world/protocol
state should come from structured client observations when available.

## Stop Conditions

- Do not update readiness or validation-ledger rows in Playable Spike Mode
  unless the owner explicitly requests readiness work.
- Do not call parity from unit or Solaris-only wire evidence.
- Stop hardening a rare edge once dominant risk is proved and the next common
  gameplay blocker is more valuable.
