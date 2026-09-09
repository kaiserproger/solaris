# ADR 0006 - mc-net module boundaries

**Date:** 2026-07-18  
**Status:** Accepted; staged extraction remains active  
**Related:** ADR 0004 and ADR 0005 own mutation ordering

## Decision

`mc-net` is a modular monolith split by vertical gameplay domain. Dependency
flow is one-way:

```text
Play packet decoder
  -> typed domain request
  -> session/simulation/regional authority
  -> semantic committed result
  -> publication adapter
  -> outbound transport
```

Root files route work. They do not own new gameplay rules. A focused module owns
its state machine, validation, and concrete request/result types. Traits or
service layers are introduced only when two current implementations need them.

Publication adapters translate committed semantic facts into recipient-specific
commands. An outbound queue or socket never decides whether gameplay committed.

## Current boundaries

- `play.rs` decodes packets and coordinates connection-local state. Domain rules
  belong below it.
- `play/simulation.rs` owns the remaining cross-domain simulation command DTOs
  and ordering glue from ADR 0004. It is not the moving-entity authority.
- `play/session/entity_owner.rs` adapts session operations to regional owner
  calls. It may retain bounded read/projection state required by real callers,
  but must not become a second mutable entity store.
- `play/session/entity_simulation.rs` coordinates the tick's domain order and
  converts committed regional motion into publication work. Same-region AI and
  physics live in `mc-entity` owner lanes.
- Combat, hostile behavior, passive mobs, villager population/defence, movement,
  pickups, containers, survival, and outbound transport use focused sibling
  modules for their rules.
- `mc-entity` owns entity ECS behavior and transaction semantics;
  `mc-physics` owns collision/movement kernels; `mc-world` owns world storage and
  immutable read snapshots; `mc-data` owns registry-derived gameplay facts.
- `mc-server::startup_data::StartupData` owns startup source selection,
  gameplay-table validation and immutable data assembly. The CLI receives one
  bundle before terrain/world preparation. Derived block facts and existing
  Loader block/light extensions stay in that assembly; no table loading is
  deferred until after world creation. Per-table provenance wrappers are gone.
- Conditional block-state commits, including cross-region storage commits,
  belong to `mc-world`. `play/block_edit_commit.rs` projects committed
  block-state results rather than validating and writing those states.
  Opaque block-entity writes also enter the resident state/token-fenced commit
  through `WorldStorage`; the unconditional opaque setter is removed.
  Player inventory, entity/drop effects, journal coordination and propagated
  light publication remain in their existing owners.
- `play/inventory.rs` owns the pure prepare-then-publish inventory operation.
  Player actions, script inventory deltas and Loader item grants share it.
  Session authorization, cursor/menu guards and packet/drop publication remain
  in their existing adapters; the inventory model stays local to `mc-net`.
- Held-item gameplay reads the selected hotbar slot from the shared authoritative
  player state. `InteractionState` has no selected-slot mirror: mining, attacks,
  item use, shields, arrows and drops consume owner selection. The packet adapter
  still clears connection-local pending actions after a successful selection
  commit; rejected selections preserve the previous owner state.
- Window-0 clicks, recipe-book crafting, offhand swaps and command grants prepare
  inventory/cursor candidates without mutating connection projections. The shared
  inventory commit builds its expected-state fence from the unchanged client
  baseline and publishes only the owner's committed or rejected snapshot.
  Missing drop support and owner errors no longer require projection rollback.
  Other container handlers still need this cutover.
- `mc-script::ScriptBoundary` owns the addon event/command transport. Player
  command roots and custom-payload channels share one route authority and
  lifecycle. Configuration/Play adapters feed its bounded event queue;
  admitted payload commands use the existing reliable per-session outbound
  lane. Reserved Loader control channels require typed, permission-checked
  commands rather than raw payload capabilities.
- `mc-script::commit_events` owns the required committed-event queue, bounded
  admission, delivery-failure notification, and drain accounting. `mc-net`
  translates authoritative player/entity commits into `ScriptEvent` snapshots
  and coordinates forwarding and shutdown; it does not own the queue policy.
  The shared monitor creates its queue endpoints and owns the failure signal;
  Tokio channels and receive errors stay private to `mc-script`.
- `xtask code-health` enforces selected ownership and dependency edges. It is an
  architecture tripwire, not behavioral evidence.

`play.rs`, `session.rs`, and `simulation.rs` still contain orchestration and some
legacy behavior. Their existence does not prove a domain is unextracted; their
callers and data ownership do.

## Publication boundary

A gameplay operation publishes only after the authoritative commit completes.
The result crossing into publication is the smallest semantic or compact fact
required by clients.

Committed script events have one delivery policy: required. The unused
best-effort branch, delivery envelope, and duplicate abandonment counters are
removed. Admission failure or abandonment notifies shutdown without rolling back
gameplay that already committed; the server still owns the forward timeout.

For regional movement:

1. owner lanes commit ECS kinematics;
2. a regional version fence accompanies compact `EntityTrackingMotion` values;
3. the session publication adapter verifies or refreshes that fence;
4. published snapshot projections and tracker state are updated;
5. recipient discovery uses published visibility/session indexes;
6. outbound commands are reserved and sent without an ECS lease or gameplay
   lock.

Visibility state, tracker shards, and published snapshots are projections. They
may reject stale work, but they may not authorize or undo ECS mutation.

## Invariants

- Domain modules do not depend on connection-loop internals merely for
  convenience.
- Packet decoding, gameplay policy, authoritative mutation, publication, and
  transport remain distinct steps.
- No packet encoding or socket write occurs while holding a gameplay lock,
  session registry lock, or regional owner lease.
- Publication DTOs contain owned values. Recipient fanout cannot borrow mutable
  authority.
- Entity mutations route through the regional owner; session projections never
  become a fallback authority.
- Cross-domain player/world transactions route through `SimulationOwner` until
  their owning ADR moves them.
- A moved caller deletes its obsolete helper, alias, cache entry, re-export, and
  compatibility path in the same cutover.
- Focused `*_tests.rs` siblings own substantial tests; aggregate roots do not
  grow new inline test modules.
- Architecture checks never substitute for gameplay, wire, persistence,
  restart, or performance evidence.

## Remaining migration

- Continue shrinking root orchestration only when a real vertical caller can
  move with its state and tests.
- Remove unused `SimulationCommand`, `SimulationHandle`, `SimulationOwner`, and
  `EntityOwnerAccess` methods after all callers migrate; do not keep aliases.
- Replace remaining broad `use super::*` imports when touching their domain,
  rather than performing unrelated formatting churn.
- Keep publication inputs compact and post-commit; avoid rebuilding full entity
  snapshots for ordinary movement.
- Move remaining server-origin world/container behavior only with the complete
  transaction and persistence boundary.

## Rejected alternatives

- A generic event bus, repository/service layer, or dependency-injection graph.
- Splitting files while retaining the same ownership and backedges.
- Returning ready-made network commands from gameplay or ECS code.
- Waiting for socket delivery as a commit receipt.
- Duplicating rules in the packet driver and authority adapter.
- A second entity cache or category store used as mutable authority.
- Broad root rewrites, compatibility shims, and speculative abstractions without
  current callers.
- Calling a module move a performance improvement without the fixed workload
  evidence.

## Consequences

The module graph follows gameplay ownership instead of connection-loop history.
Most changes have a concrete domain owner and a narrow publication boundary.
Some root orchestration remains; it is accepted until a complete caller cutover
can delete code rather than add another layer.
