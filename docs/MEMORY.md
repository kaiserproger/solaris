# Solaris Durable Memory Index

This is the short continuity index for long `/goal` runs. It records current
head and routes detail to its canonical owner. Historical checkpoint prose is
kept in [`archive/status/2026-07-19-memory.md`](archive/status/2026-07-19-memory.md)
and is not startup context.

## Current Checkpoint

- Date: 2026-07-19.
- Branch: `dev/M100-client-agent`.
- Latest checkpoint: `24223dc` (`feat(mc-net): add wall torches and isolate
  regional mutation`).
- The worktree may contain unrelated owner files and local artifacts. Inspect
  exact ownership before editing; never clean or stage them by accident.
- Full workspace tests, workspace all-target strict Clippy, fmt, code-health
  `0 fail / KEEP`, and diff-check passed for `24223dc`.
- Ignored oracle/load/benchmark rows remain explicit. Real-client,
  performance, concurrency, and soak gates did not run for that checkpoint.

## Current Head

### Core And Ownership

- `play.rs` is 13,079 lines, `session.rs` 1,502, and `simulation.rs` 15,488.
  The migration is staged, not complete.
- `simulation/queue.rs` owns bounded admission, accounting, pushed wakeup,
  batching, shutdown, and channel construction.
- `simulation/regional_mutation.rs` owns the existing regional block/container
  mutation lane behind explicit imports and code-health tripwires. The parent
  still owns classification, batching, world access, lighting/publication, and
  `SimulationOwner`.
- Regional entity ownership and the ECS shadow/cutover path exist, but neither
  the regional migration nor ECS transition is complete. ADR 0004/0005 are the
  authority source of truth.
- Runtime work control has no operator worker-percentage knobs. Capacity is
  derived once; pushed measurements and bounded admissions drive allocation.
- Production and test waits must remain event-driven. Timeouts only fail stuck
  work and never prove success.

### Playable And Client-Visible

- Stair facing/half and slab top/bottom follow the inspected local 26.1.2 rule.
  Slab merging and stair neighbour shapes remain open.
- Ordinary torches place as wall torches on horizontal conservative full-cube
  supports, remain standing on `UP`, and reject `DOWN` or known partial
  supports. Irregular sturdy-face parity and neighbour break cascades remain
  open.
- Stonecutter server behavior has focused coverage; a real 26.1.2 client menu
  gate remains open.
- The embedded client MCP provides reusable connection, observation, movement,
  interaction, and scenario tooling. Read `docs/AGENT_TOOLING.md` before
  changing it; protocol bots do not replace the real-client gate.

### Known Runtime Evidence

- Latest P44 artifact:
  `.analysis/real-client-runs/20260718T111008Z-m94-regression-pack-4X1iF7`.
  Cow and sheep passed. Chicken moved but failed smooth yaw with a 79.2-degree
  minimum delta. The old two-CPU run reported 24 slow ticks, about 69.7 ms
  average slow tick and 102.7 ms maximum. This is diagnostic, not green.
- The owner has removed the CPU cap for new runs. Do not treat the old bounded
  result as current unrestricted performance evidence.

## Active Risks

1. Rerun the focused real-client movement/performance pack without the old CPU
   cap and inspect rare stalls, not only aggregate medians.
2. Prove wall torch, stair, and slab placement in the real client.
3. Continue reducing `simulation.rs` through explicit ownership boundaries;
   avoid moves that retain `use super::*` or duplicate authority.
4. Advance regional ownership/ECS only with exact CAS, WAL, publication, and
   cross-region failure fences.
5. Broaden playable progression by the Pareto rule before polishing rare parity
   edges.

## Canonical Routes

| Need | Read |
| --- | --- |
| Playable/client behavior | `docs/playable/README.md`, then `docs/playable/ACTIVE.md` |
| Architecture/ownership | `docs/decisions/README.md`, then the exact ADR |
| Current M100 milestone | `docs/milestones/M100.md` |
| Readiness claim | `docs/DEFINITION_OF_DONE.md` and `docs/VALIDATION_LEDGER.md` |
| Protocol | ADR 0002 and local protocol tools |
| Client MCP | `docs/AGENT_TOOLING.md` and the client-agent README |
| Server Lua API | `docs/PLUGINS.md` |

## Update Rules

- Replace stale current-head facts; do not append a wave-by-wave diary.
- Put architecture decisions in ADRs and playable observations in
  `docs/playable/ACTIVE.md`.
- Keep raw run output under `.analysis/` and out of commits.
- Use archives only to recover a specific old fact.
