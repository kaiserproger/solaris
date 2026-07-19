# Single-Writer Simulation Boundary Design

**Quality label:** `stabilization`.

## Scope

This design is the first independently revertible Prompt 03 slice. It creates
the typed network-to-simulation boundary and transfers authoritative item/XP
claim mutation. It does not migrate spawn, combat, blocks, containers, world
storage, persistence, or ECS ownership yet.

## Options Considered

1. Replace `SessionRegistry` with a complete world actor now. This reaches the
   target quickly on paper but combines every ownership risk and removes the
   legacy runtime as a replay oracle.
2. Hide a command queue inside `SessionRegistry`. This needs less plumbing but
   keeps callers unable to distinguish reads from authoritative mutation and
   makes accidental direct mutation easy.
3. Use an explicit cloneable `SimulationHandle` and one non-cloneable
   `SimulationOwner`. This adds deliberate wiring through server/play state but
   makes authority, queue pressure, and shutdown ownership visible. Use this
   option.

## Components

`crates/mc-net/src/play/simulation.rs` owns the bounded channel contract,
sequence allocation, metrics, typed request errors, and the non-cloneable
receiver. `SessionRegistry` retains the transitional entity store and provides
one owner-only command application entrypoint. `BoundServer` creates the pair,
moves the owner into the entity ticker, and clones the handle into each play
connection. `InteractionState` uses the handle for item and XP claims.

## Tick Data Flow

1. A network task reads a nearby immutable entity snapshot and computes the
   maximum item count its connection-owned inventory can accept.
2. `SimulationHandle::claim_item_pickup` or `claim_experience_pickup` performs a
   fail-fast bounded enqueue and awaits its oneshot outcome.
3. Before goals/physics, `SimulationOwner` drains up to 256 commands, sorts by
   sequence, and invokes the matching owner-only `SessionRegistry` operation.
4. A successful outcome contains the claimed value and visibility dispatches.
   A lost race returns `Ok(None)`; queue rejection returns a typed error.
5. The network task applies inventory/XP and writes packets only after success.

## Correctness Rules

- One accepted item/XP entity can produce at most one complete claim; partial
  item claims preserve the authoritative remainder.
- Queue-full, queue-closed, already-cancelled, and shutdown-rejected commands do
  not mutate entity state. Cancellation racing after apply but before response
  delivery remains open until player inventory/XP joins the owner transaction.
- Commands in one owner batch apply in ascending sequence order.
- The queue and per-tick work are bounded by constants, not unbounded spawning.
- Network code has no production call to the legacy direct item/XP claim API.
- Visibility packet dispatch remains outside the session lock and owner phase.
- Shutdown rejects unapplied work before entity persistence is snapshotted.

## Validation

Unit tests cover capacity, ordering, budget carry-over, cancellation, shutdown,
and telemetry. A dual-path test replays the same ordered item/XP claims through
legacy test-only helpers and the owner, comparing normalized entity, reward,
and dispatch outcomes. The existing concurrent pickup task regression is
migrated to a running owner and remains a real connection-state path. Prompt 02
checked replay, short soak, P4/P42 real-client gates, lock metrics, and the full
workspace baseline guard later Prompt 03 slices.

## Explicit Debt

Entity spawns, arrows, melee damage, death rewards, block edits, containers,
scheduled ticks, and `WorldHandle` remain legacy after this slice. Passing this
slice proves the boundary and item/XP authority transfer only.
