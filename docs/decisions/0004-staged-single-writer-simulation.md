# ADR 0004 - Staged single-writer simulation boundary

**Date:** 2026-07-11  
**Status:** Accepted; superseded by ADR 0005 for region-owned entities

## Decision

`SimulationOwner` remains the ordered authority for player/session aggregates,
world mutations, containers, block entities, and transactions that span those
domains. Moving entities are no longer owned by one global simulation writer;
ADR 0005 assigns each entity to one regional ECS owner.

Every simulation command has a monotonic enqueue sequence and a typed result.
The bounded command channel wakes the owner directly. Queue closure or capacity
rejection happens before mutation. Once mutation begins, requester or socket
progress cannot decide whether the mutation committed.

A command that cannot determine its commit outcome is fatal. Solaris stops the
owning runtime rather than retrying, guessing, or publishing a result that might
not match authority.

## Current authority

`SimulationOwner` owns ordering and commit policy for:

- player inventory, cursor, experience, survival, death, and connection-fenced
  mutations used by migrated commands;
- packet-authored world edits and their block-state or mutation-token
  preconditions;
- shared container, furnace, campfire, and sign transactions already routed
  through typed commands;
- cross-domain transactions whose state does not share one regional entity
  owner;
- save barriers and persistence snapshots for its remaining domains.

Conditional block-state and opaque block-entity commits are headless:
`WorldStorage::apply_block_edits_conditionally` prepares all required positions
and delegates to the resident commit kernel. Missing/out-of-bounds positions or
stale state/token preconditions reject the batch before any edit is published.
Single-region execution retains its fast path; multi-region storage edits reuse
the existing staged source fence and atomic publication boundary.

`WorldStorage::commit_opaque_block_entity_conditionally` loads the chunk, then
checks state/token and publishes NBT under the existing resident region lock.
The net-side check/write split and unconditional opaque storage setter are gone.
Campfire use publishes inventory/cooking state only after that commit accepts;
hopper persistence captures its expected state/token from one chunk snapshot.

The storage facade publishes dirty world state, not a disk durability receipt.
Durability-backed scheduled-block transactions still persist before publication;
normal saves retain their existing dirty-flush and journal fences. Inventory,
entity/drop effects, command ordering and socket publication remain outside this
world-only operation.

Player inventory publication uses the local, session-free
`PlayerInventory::try_update` operation: plan against an immutable inventory,
then replace it only on success. Player actions retain their inventory, cursor
and menu-input preconditions under the existing player-state lock. Script
inventory deltas and Loader item grants plan from canonical
`PlayerPersistedState.inventory`, not the potentially stale connection
projection; that projection is refreshed only after a successful commit.

`RegionalOwnerRuntime` and its lane-owned `EntityStore` instances own entity
identity, lifecycle, retained state, AI, motion, combat mutations, vehicles,
regional transfers, and entity persistence snapshots. `SessionRegistry` and
published snapshot maps are indexes or projections, not a second entity
authority.

The session, connection task, plugin host, persistence worker, and outbound
transport never receive mutable ECS references.

## Current execution path

A normal tick has these boundaries:

1. Accepted `SimulationCommand` values run in enqueue order. Session-authored
   commands revalidate the current `SessionId` immediately before mutation.
2. Entity commands are routed to the current regional lease. Same-owner work
   mutates that owner directly; multi-owner work uses ADR 0005's ordered
   transaction protocol.
3. Regional owner lanes apply local goals. Central domains that intentionally
   interact between goals and physics then commit their work.
4. Regional owner lanes reread current ECS state, run exact local physics, and
   commit kinematics.
5. Semantic results and compact movement facts are published only after commit.
   Encoding and socket writes run after every authority lock or lease is gone.
6. A save barrier captures one completed authority boundary. Disk IO follows
   from immutable snapshots.

Bounded dirty-only saves advance a world-local region/chunk cursor when selecting
a batch, wrapping after the last position. Repeatedly dirtied early chunks cannot
starve later positions. Selection advances even when mutations invalidate a
pending batch; region-version and dirty-generation fences remain unchanged.

Startup requests the existing full checkpoint, including the simulation save
barrier. Unlike pressure-only flushing, it must persist the warmed spawn snapshot
even after players begin mutating chunks. It uses the normal checkpoint's
serialized capture, snapshot installation and journal watermarks; it does not
relax pressure-flush generation checks or add another save path.

The command queue is admission and ordering, not an entity tick scheduler.
Ordinary entity AI and physics do not traverse it.

Chunk generation, preparation and simulation planning share bounded CPU
admission. Chunk-queue pressure includes requests still awaiting execution, not
only completed results. It reduces producer rates and view limits without
reducing the CPU capacity needed to drain those requests. Tick-time and memory
pressure reduce admission gradually; shutdown draining still reduces capacity
to one immediately. The application policy lives in `server/runtime_control.rs`;
there is no second executor, inline planning bypass or operator thread setting.

Adaptive view/rate limits and work budgets use monotonic, continuous pressure
and recovery windows, independently reset after each one-unit step. Both policy
durations are seconds, normalized to at least 60; a step requires strictly more
than that duration. Recovery also requires 20% tick-time headroom. Source recovery
rechecks the latest tick and memory observation rather than manufacturing a
healthy sample. Coalesced producer notifications retain an aggregate-zero
recovery barrier before pending pressure states, including recovery followed by
reactivation before consumption. Isolated slow-client shedding cannot establish
continuous overload. Queue and memory admission fences and explicit draining
remain immediate; delaying adaptive tuning does not delay those safety bounds.

Resident and dirty byte usage are updated with the existing chunk-publication
counters. Publication measures the replaced chunk before releasing its snapshot
and the resulting chunk after mutation, applying byte deltas for insert,
replacement, growth/shrinkage, clean/dirty transitions and removal. Storage
admission and statistics consume these totals instead of rescanning every
resident chunk. The heap estimator, budgets, save-health checks and cross-region
publication fence are unchanged; there is no parallel resident cache or new lock.

Scheduled-block plans run on the shared admitted CPU workers. The serial commit
path reuses its first snapshot-based plan because no earlier group in that batch
has committed; it does not reacquire CPU merely to produce the same plan again.
Every later group replans against current snapshots so repeated regions observe
earlier changes. Existing state/token and due-prefix checks still reject stale
plans without consuming their work. The plan type and synchronous planning live
in the scheduled-block domain; CPU admission and blocking-worker dispatch remain
in play orchestration. The domain does not own an asynchronous execution path.

Full chunk-light computation shares those CPU permits. Direct-sky boundary
seeding records each column's open-sky bottom, then queues only its bottom edge
and the intervals below adjacent columns' bottoms. This replaces a measured
full-volume six-neighbour scan; propagation, opacity, missing-neighbour handling
and publication fences do not change. Bounds are local to one computation, not
a retained cache. Full-source propagation comparison guards the optimization.
If direct sky no longer forms a top-open interval, replace this seeding rule;
there is no legacy scan fallback or runtime switch. Measurement and client
evidence live in the `streaming-light` checkpoint linked from `docs/MEMORY.md`.

## Invariants

- One mutable authority exists for each field at any instant.
- Queue-full, queue-closed, stale-session, stale-lease, and failed-precondition
  outcomes do not mutate state.
- Cancellation is checked before mutation. Cancellation after an accepted
  commit cannot turn the commit into rejection.
- Composite transactions validate every expected player, world, container, and
  entity value before changing any of them.
- Cross-domain locks use their documented order and are never held across an
  owner wait, packet encoding, or socket progress.
- Publication is a post-commit projection. A stale projection fence causes
  revalidation or fail-closed omission, never speculative publication.
- Save and shutdown wait for exact completion notifications. Wall-clock sleeps
  and timeout-based success are forbidden.
- Durable replay uses recorded decisions and watermarks. It does not infer a
  commit from partial messages.

## Remaining migration

The staged boundary is not a claim that all world state has one owner. Current
remaining work is limited to real caller cutovers:

- remove legacy session/world mirrors when their final readers have moved;
- move remaining server-origin block and aggregate mutations behind their
  owning transaction boundary;
- shrink `SimulationCommand`, `SimulationHandle`, and `SimulationOwner` whenever
  a command category has no production callers;
- keep active save entry points ordered while global multi-domain disk commits
  remain non-atomic.

Each cutover removes the old helper, mirror, alias, and test path in the same
change. Solaris has no compatibility obligation to superseded internal APIs.

## Rejected alternatives

- A second mutable entity store or session-owned entity mirror.
- Treating outbound delivery as an authority acknowledgement.
- Retrying an outcome-unknown transaction.
- Polling commands on tick cadence or sleeping for lock availability.
- Holding the global session lock while waiting for a regional owner.
- A generic event bus, service layer, or compatibility shim without two current
  implementations.
- Moving ordinary regional AI or physics back through `SimulationOwner`.

## Consequences

Ordering remains explicit where gameplay state spans domains, while ordinary
entity work scales through regional ownership. The cost is a visible boundary
between authoritative commit and publication plus deterministic protocols for
multi-owner work. That cost is accepted because it prevents duplicated
ownership and ambiguous recovery.
