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

1. Rerun the P44 livestock movement/performance pack without the old CPU cap.
   Inspect chicken yaw and rare slow ticks, and keep cow/sheep climbing checks.
2. Run a real-client building check for wall torches, stairs, and slabs. Record
   placement state, rejected support behavior, inventory debit, and reconnect.
3. Prove the stonecutter menu with a real 26.1.2 client, including selection,
   normal take, shift-click, close/reopen, and rejected invalid input.
4. Keep terrain navigation and item pickup stable across farmland, fences,
   slabs, stairs, one-block rises, and chunk boundaries without guessed jumps.
5. Add the next high-value survival content slice only after the above red
   client-visible paths are understood.

## Recent Evidence

- Checkpoint `24223dc` passes full workspace tests, workspace all-target strict
  Clippy, fmt, code-health `0 fail / KEEP`, and diff-check.
- Ordinary wall torches have registry-backed tests for four horizontal facings,
  standing `UP`, rejected `DOWN`, and partial support. Raw TCP proves one debit
  after accepted update/ack and unchanged held-stack resync before rejected ack.
- Stair facing/half and slab top/bottom use the inspected local 26.1.2 rule.
  Slab merging and stair neighbour-shape selection remain open.
- The regional mutation extraction is architecture-only and makes no gameplay
  or performance claim.
- Latest P44 artifact is
  `.analysis/real-client-runs/20260718T111008Z-m94-regression-pack-4X1iF7`.
  Cow and sheep passed; chicken failed smooth yaw. Its old two-CPU performance
  result is stale now that new runs may use normal CPU capacity.
- Stonecutter focused server tests pass, but the real-client menu gate remains
  open.

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
