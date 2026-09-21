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

Receipt-bearing server-owned block-edit batches cover settlement structure
portions and resident harvesting, mining, cutting, and planting. They reuse
`ApplyBlockEdits`' regional lane rather than a domain-specific command or saga:
the submitter captures exact mutation-token preconditions and resolves the
bounded `before-build` question before enqueuing the finite block portion. It
prepares the storage batch (including material consumption or resident cargo and
progress) and encodes its receipt before submission; that preparation has no
durable effect until the regional decision is acknowledged. A cancellation is
delivered with the submitted approval and the regional worker refuses it before
block mutation or world-journal reservation. The worker checks the supplied
preconditions and rechecks the captured zone fence before it conditionally
applies/stamps the blocks, appends the chunk after-images and encoded receipt as
one world-journal decision, then publishes. Only the acknowledged decision
permits storage projection and `mark_inventory_projected`; reopening projects
that same receipt before new settlement or resident work. Ordinary server-owned
edits without a receipt retain the canonical staged path, including campfire
eviction and reactivity.

Structure footprint change detection is content-scoped, not journal-scoped: a stage
fence compares a digest of the block states strictly inside the reserved footprint and
fails closed while a covering chunk is unloaded. Keying it on the covering chunks'
durable journal position was wrong: the world's own scheduled-block-tick transaction
advances that position for work no player did, so a build paused as `site_changed` on
its own surroundings. The deliberate narrowing is that an edit elsewhere in the same
16x16 chunk - digging beside the reserved box - no longer pauses the operation, while
any block change inside the footprint still does.

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

## Warehouse inventory transfer (landed 2026-09-14)

Settled during implementation, recorded here because it constrains the receipt:
the run allocates the decision id and the response carries it, so the receipt is
encoded before that id exists. A player endpoint's receipt fence revision IS the
post-commit `inventory_operation_revision`, which `PlayerInventoryRecovery` and
`load_player_state` force to be the journal decision id (its monotonic guard
makes any other numbering skip a replay write), and a submitter pre-reservation
would break `WorldChunkJournal::record_reserved_decisions` contiguity: a run
reserves one block per run and cannot wait for a foreign pending id without
deadlocking the owner task that must process the deposit. A warehouse transfer's
`Transfer` result therefore names the container's resulting fence (binding
revision plus the hash of its planned slots) and not the actor's, whose fence is
re-read by a query; naming a pre-commit revision would hand the plugin a fence
that can never match.

## Warehouse inventory transfer (design, 2026-09-14)

A warehouse is a bound authored container of a placed settlement structure. The
plugin never names a coordinate: core mints the handle and `bind_warehouse`
(`crates/mc-net/src/script/storage/settlement.rs:1251`) binds it to a durable
`DurableWarehouseBinding`; reading it is already a canonical owned-inventory
snapshot (`:1342`). Writing it must be one server-owned simulation command,
because a deposit spans the three domains this ADR keeps on one ordered
authority: the world container, the player inventory, and the plugin operation
receipt.

The first write attempt was built on a bare `WorldMutationView` container
mutation and was reverted: it neither advanced the container's `chest_state_ids`
fence nor published `ChestSlots`, so a concurrent menu commit could plan from
the pre-transfer contents. That is a second authority, and it is why the write
path is a designed cutover rather than a container-local fix.

Required shape:

- The composite rides the existing regional chest command as
  `CommitChest` with a server-owned flag, not as a new standalone command: that
  command already owns the mutation view, the container state-id fence, the
  `ChestSlots` publication and the journal, and a standalone variant would
  duplicate enum, `kind`, `command_requires_world`, region, result, response,
  metrics and handler arms for the same composite.
- The command carries the container's position, its expected block entity (27
  slots) and expected `state_id`, the player's expected/updated inventory plan,
  and the **encoded** plugin operation receipt
  (`PreparedStorageBatch::encode_world_inventory`).
- The receipt rides the run's ONE group append: the chest job result must carry
  the stamped after-images plus the encoded batch, so the journal needs a group
  append that takes images plus an optional encoded batch, with the existing
  `record_reserved_snapshot_groups` and `record_reserved_inventory_decision`
  delegating to it (this append **does not exist yet**), because one decision id
  takes exactly one append
  (`crates/mc-net/src/play/world_inventory_journal.rs:48`).
- Decision id ownership is the first implementation decision, not a detail: the
  run allocates its own consecutive ids
  (`crates/mc-net/src/play/simulation/regional_mutation.rs:226`), so either the
  reservation is filtered to envelopes that need a decision and the submitter
  reserves this one, or the response returns the allocated id. The submitter
  cannot write the player recovery after-image or mark the decision projected
  without it.
- Inside the owner turn, in this order: validate the player fence
  (`inventory_recovery_required`, expected inventory, expected carried item) and
  the container fence (the `chest_state_ids` value plus
  `commit_chests_conditionally(expected, updated)`, which rejects a stale
  after-image); stamp the container's chunk for the reserved id and take its
  after-image (`WorldMutationView::stamp_chunks_for_world_journal`,
  `crates/mc-world/src/resident.rs:715`); append the ONE world-journal decision -
  chunk after-image plus the encoded receipt; advance `chest_state_ids` and
  publish `ChestSlots` to the container's viewers; then respond with the decision
  id and outcome.
- The server-owned commit is a second entry point beside
  `ChestTransaction::commit` (**not yet written**): the same composite as the menu
  path without the `actor_has_open_view` fence, with the actor receiving the
  published slots instead of a menu ack. Every other fence - container state id,
  expected inventory, expected carried item - is unchanged.
- The submitter then projects only what the response acknowledges: install the
  decoded plugin storage batch, write the player recovery after-image
  (`PlayerInventoryRecovery::recover`), publish the player's
  `AuthoritativeInventory`, and only then `mark_inventory_projected(id)`.
- A stale fence, an absent container or a rejected chest commit never mutates:
  the command answers a typed refusal and appends the decision with no
  participant, mirroring the stale path of `BlockEdits` (`crates/mc-net/src/play.rs:8540`).

Rejected alternatives:

- Reusing the *menu* semantics of `CommitChest`
  (`crates/mc-net/src/play/session/transactions.rs:87`) unchanged: the menu path
  requires `actor_has_open_view` and fences the player against a menu plan, and a
  plugin deposit has no open menu. The command shape is reused, its
  session-authored semantics are not.
- A new standalone command variant: the same composite as the chest run, but it
  duplicates that run's mutation view, fence, publication, journal and outcome
  plumbing.
- Two decisions - container first, receipt second: a crash between them leaves
  items inside a durable container with an unspent receipt, or a spent receipt
  without the transfer. That split is what the one-decision rule exists to
  prevent.
- Taking a `WorldStorage::mutation_view()` on the script thread: the reverted
  attempt, and a second writer.

The cutover removes the `Warehouse` arm of the endpoint gate in
`crates/mc-net/src/play/session/owned_inventory_endpoint.rs` and routes it to
this command. The reading endpoint stays as it is; there is no second item
ledger and no plugin-supplied coordinate.

As landed, one further rule joins this section: a receipt-bearing `CommitChest`
is admitted only to a journaled regional run
(`command_needs_world_journal`), and the menu path refuses one rather than
committing a container whose receipt has nowhere to go. A refused composite
still appends its reserved decision with no participant, so the run's single
append stays contiguous, and an unstampable container chunk fails the run's
append rather than publishing a receipt recoverable without its container.

The append-before-publication order is enforced, not merely structural. The
regional run arms a test-only probe
(`crates/mc-net/src/play/simulation/regional_mutation.rs`) before it appends,
and the observation is taken inside the publication path itself, where the
container's `ChestSlots` command leaves the run
(`crates/mc-net/src/play/session/outbound.rs`): the journal records the append
state at that moment. `server_owned_warehouse_deposit_appends_before_publishing_and_recovers_both`
fails with `[(decision_id, false)]` when the run publishes before appending, and
with no observation at all when the publication does not pass through that path,
so the rule cannot silently regress into publishing a container whose
after-image is not journaled.

## Warehouse inventory transfer, worker principal (landed 2026-09-15)

The same composite serves a deposit whose second participant is not a player
session. A worker's `Haul` work order names a bound warehouse as its
destination; its other half is the worker's own canonical record, which lives in
the plugin ledger and not in the world. Two rules make that reuse safe rather
than a second authority:

- **One container half, one ordering.** `commit_container_half`
  (`crates/mc-net/src/play/session/transactions.rs`) holds the state-id fence,
  the conditional `commit_chests_conditionally`, the state-id bump and the
  `ChestSlots` publication for *both* the menu path and the server-owned path;
  only which actor the publication excludes and how each names a refusal stay
  caller-specific, so the publication order cannot drift between them.
- **The actor is optional, the receipt is not.** `CommitChest` carries
  `actor_session: Option<SessionId>` and `player: Option<Box<ContainerPlayerPlan>>`.
  A deposit with no player participant is enqueued through
  `enqueue_with_fence(None, …)` exactly like every other server-owned command;
  the session-authored menu shape is refused by the validator when it is missing
  its session or its player plan. The session and the player plan travel
  together in every shape the validator admits
  (`actor_session.is_some() == player.is_some()`), so a plan can never move a
  player's items past the state that fences and after-images them; the
  transaction asserts the same invariant against the state it actually holds.
  The second participant is the record change inside the encoded receipt, so the
  container's after-image and the worker's cargo are durable together or not at
  all - the batch's `order` change is applied by the same projection
  (`append_inventory_projection`) the player's recovery after-image uses, and a
  receipt-bearing batch is still admitted only to a journaled regional run.
- **One deposit tail, one ordering.** `InventoryRuntime::commit_prepared_deposit`
  (`crates/mc-net/src/script/storage/world_inventory.rs`) owns everything after a
  caller has prepared its batch: encode the receipt, fence and commit the
  container, project the ledger frame, acknowledge the decision. The player's
  inventory and a worker's record differ only in how their batch was prepared and
  in whether a player participant travels with it, so both callers keep their own
  fencing and planning and share this tail; the alternative - a resident entry
  point that re-implements the sequence - is exactly the drift this section
  exists to prevent.

A worker haul also carries a planning rule worth naming, because it decides
whether a settled storage state can progress at all: the deposit moves what fits
and leaves what does not, slot by slot. A container that cannot take the worker's
first stack but can take a later one deposits the later one rather than refusing
the whole cargo and stalling the job on the same stack every cycle; only a cargo
that fits nowhere answers `capacity`.

One consequence is recorded deliberately: a haul is directed. `Haul { source,
destination }` is no longer reordered by `ScriptResidentWorkOrder::canonicalize`
(which is gone); a directional pair is not a set, and the previous swap turned a
deposit into a withdrawal.

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
