# Solaris Durable Memory Index

This is the short continuity index for long `/goal` runs. It records current
head and routes detail to its canonical owner. Historical checkpoint prose is
kept in [`archive/status/2026-07-19-memory.md`](archive/status/2026-07-19-memory.md)
and is not startup context.

## Current Checkpoint

- Date: 2026-07-19.
- Branch: `dev/M100-client-agent`.
- Latest checkpoint: `02cf22a` (`refactor: split gameplay and core domain
  modules`).
- The worktree may contain unrelated owner files and local artifacts. Inspect
  exact ownership before editing; never clean or stage them by accident.
- Last recorded full workspace tests, workspace all-target strict Clippy, fmt,
  code-health `0 fail / KEEP`, and diff-check passed at `24223dc`. The later
  `02cf22a` checkpoint and current entity wave still need a fresh full baseline.
- Ignored oracle/load/benchmark rows remain explicit. Real-client,
  performance, concurrency, and soak gates did not run for that checkpoint.

## Current Head

### Core And Ownership

- `play.rs` is 12,589 lines, `session.rs` 1,507, and `simulation.rs` 15,488.
  The migration is staged, not complete.
- `simulation/queue.rs` owns bounded admission, accounting, pushed wakeup,
  batching, shutdown, and channel construction.
- `simulation/regional_mutation.rs` owns the existing regional block/container
  mutation lane behind explicit imports and code-health tripwires. The parent
  still owns classification, batching, world access, lighting/publication, and
  `SimulationOwner`.
- `EntityStore` uses the ECS runtime and the old vector comparison state,
  `Shadow*` API, aliases, and `shadow-compare` feature are deleted. The current
  cutover is still moving gameplay-significant temporal state and projectile
  transactions out of `mc-net` side maps, so broad sole-authority readiness is
  not yet established. ADR 0004/0005 are the authority source of truth.
- Runtime work control has no operator worker-percentage knobs. Capacity is
  derived once; pushed measurements and bounded admissions drive allocation.
- Serverbound protocol collections/strings/blobs have a complete bounded
  allocation audit, symmetric encode limits, and no-partial-output tests.
- Production worldgen now consumes explicit `ChunkGeometry` for terrain, ores,
  structures, and biome assignment. Extreme valid geometries use checked/wide
  arithmetic, and the default Overworld path has a deterministic serialized-NBT
  fingerprint. The algorithm remains Solaris-owned rather than Mojang
  NoiseRouter parity.
- Lua API 0.6 has bounded DTO/files/batches and one-shot host admission. The
  attested `mc-net` router plus storage/menu/zones/colonies adapters are active
  implementation work, not a completed production surface.
- Production and test waits must remain event-driven. Timeouts only fail stuck
  work and never prove success.

### Playable And Client-Visible

- Stair facing/half, slab top/bottom, adjacent matching-slab merge, and
  waterlogging follow the inspected local 26.1.2 rule. The focused executable
  gate must be refreshed after the concurrent ECS slice; stair neighbour shapes
  remain open.
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
2. Refresh focused and real-client wall torch, stair, and slab placement gates.
3. Continue reducing `simulation.rs` through explicit ownership boundaries;
   avoid moves that retain `use super::*` or duplicate authority.
4. Advance regional ownership/ECS only with exact CAS, WAL, publication, and
   cross-region failure fences.
5. Finish the attested Lua storage/menu/zones/colony production adapters.
6. Broaden playable progression by the Pareto rule before polishing rare parity
   edges.

## Canonical Routes

| Need | Read |
| --- | --- |
| Playable/client behavior | `docs/playable/README.md`, then `docs/playable/ACTIVE.md` |
| Architecture/ownership | `docs/decisions/README.md`, then the exact ADR |
| Detailed core internals and pitfalls | `docs/CORE_INTERNALS_FOR_OWNER.md` |
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
