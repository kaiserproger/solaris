# ADR 0003 - Runtime world lock architecture

**Date:** 2026-06-01
**Status:** Accepted for legacy paths; staged supersession by ADR 0004
**Context:** M68.d architecture drift note

## Context

`PROJECT_SPEC.md` describes a target server architecture with clear ownership
boundaries between networking, simulation, persistence, and world mutation. The
current M68 codebase has intentionally drifted while milestones prioritized
client-visible gameplay and wire compatibility:

- `WorldHandle` is `Arc<tokio::sync::Mutex<WorldStorage>>`.
- Each play connection owns an `InteractionState` with session-local inventory,
  carried item, active container, pending break/use state, shield state, and a
  per-connection light cache.
- Many interaction handlers briefly lock world storage to read or mutate blocks,
  block entities, and scheduled ticks.
- Long-running broadcast, packet writes, entity visibility dispatch, and most
  inventory mutation are kept outside the world lock.

This shape is not the final single-writer simulation model from the project
spec, but it is the architecture Solaris has shipped through M68.

## Decision

Keep the current shared world lock model for the near-term milestone track. Treat
it as an explicit transitional architecture, not accidental technical debt.

Acceptable world-lock usage today:

- Short storage reads for interaction target checks, block facts, block entity
  state, collision samples, water overlap, and spawn/material probes.
- Short storage mutations for player-authored block edits, block entity updates,
  scheduled ticks, and persistence-backed container state.
- Building a local snapshot under the lock, dropping the guard, then writing
  packets or dispatching visibility commands.
- Relight planning that locks only while reading chunks or applying already
  chosen edits.

Code should avoid:

- Holding `world.lock().await` across network writes, session broadcasts, sleeps,
  recipe work that does not need storage, or expensive scans that can use a
  snapshot.
- Mutating `InteractionState` inventory/container state while also performing
  unrelated world I/O unless the operation must be atomic from the client action
  perspective.
- Introducing additional global mutable state that competes with `WorldHandle`
  without documenting ownership.

Remain single-writer or main-loop owned:

- Per-session connection state in `InteractionState`.
- `SessionRegistry` visibility and outbound command ownership.
- Entity lifecycle dispatch decisions before they become storage persistence.
- Packet encode/write ordering for one client connection.

Future milestones may replace `WorldHandle` with a simulation actor or command
queue, but that is a separate concurrency redesign. Until then, cleanup should
make lock spans smaller and more obvious rather than pretending the final model
already exists.

ADR 0004 starts that redesign. Its typed simulation commands supersede this
ADR only for domains explicitly migrated behind `SimulationHandle`; all other
world, session, container, and entity paths remain governed by ADR 0003 until a
later slice transfers their authority.

## Consequences

Positive:

- Documents the real M68 architecture so new cleanup work does not chase the
  original spec blindly.
- Gives reviewers a concrete rule for world-lock changes: short critical
  sections are acceptable; lock-spanning I/O is not.
- Lets gameplay cleanup continue without forcing an M39-scale runtime rewrite.

Negative:

- Solaris still has coarse world-storage serialization during concurrent player
  interactions.
- Some interaction handlers still mix protocol, inventory, world I/O, and
  visibility dispatch; M68 cleanup reduces this but does not eliminate it.
- The eventual actor/single-writer design will need migration work and tests for
  ordering-sensitive interactions.

## Implementation Notes

- `crates/mc-net/src/play.rs` is the main drift point: `InteractionState` owns
  per-connection state and locks `WorldHandle` for interaction-driven storage
  work.
- `crates/mc-net/src/server.rs` defines `WorldHandle` as the shared async mutex
  over `WorldStorage`.
- M68.b and M68.c are examples of acceptable transitional cleanup: they clarify
  control flow without changing world ownership.
- Dirty-cache pressure has one production persistence authority: the
  server-owned `DirtyFlushCoordinator`. Chunk preparation publishes a
  coalesced request and waits for the exact accepted worker action; a stream
  generation change wakes and cancels stale waiters. It must not start a
  competing full flush. Tests without the server worker may use the bounded
  eight-chunk fallback, but that fallback is not a second production owner.
