# ADR 0005 - Regional simulation ownership

**Date:** 2026-07-16
**Status:** Accepted, staged migration
**Supersedes:** The single synchronous writer rule in `PROJECT_SPEC.md` only
for domains explicitly migrated to regional ownership. ADR 0004 remains in
force for simulation commands and domains still owned by its coordinator.

## Problem

Solaris now has one ECS-authoritative `EntityStore`, but the store is still
behind one global mutex. Recent work shortened common holds and removed several
`SessionRegistry + EntityStore` critical sections. That improves latency, but
it does not provide true multicore simulation: unrelated herds, projectiles,
and players in distant parts of the world still serialize on the same store.

The current `PROJECT_SPEC.md` says that one thread is the sole world writer.
This ADR replaces that rule with one writer per region through a staged
production migration.

## Decision

This ADR accepts the regional target and its staged migration rules. It does
not claim that the migration is complete: current production uses persistent
regional entity-owner lanes, while coordinator metadata, cross-domain
transactions, and several publication paths remain centralized. Each later
authority move must retain the fencing and acceptance gates below and update
this ADR's implementation status.

Partition loaded chunks into fixed 8 by 8 chunk regions. A `RegionKey` uses
Euclidean division, so negative chunk coordinates have the same stable
boundaries as positive coordinates.

Each active region has exactly one owner at a time. Owners run on a bounded set
of worker lanes. A lane may own many regions, but no region may run on two
lanes. Once regional lanes exist, the existing autoscaler will choose their
count from current CPU and runtime pressure; there will be no operator
worker-percentage setting.

Ownership is identified by `(RegionKey, RegionEpoch)`. Every routed command,
worker result, migration, and save snapshot carries the expected epoch. A stale
epoch is rejected without mutation. Reassigning a region is allowed only at a
completed phase boundary: the old lane publishes its final snapshot and tail,
the coordinator increments the epoch, and only then may the new lane accept
commands.

Region-owned state is not protected by a shared simulation mutex. Network,
plugins, IO, and other regions send bounded commands. A blocked owner waits on
the exact channel or barrier notification that can advance it. Timeouts only
fail a stuck operation.

The first authority move covers moving entities only. Sessions, connections,
static registries, chunk storage, and block entities remain outside regional
ECS ownership until separate evidence justifies moving them.

## Tick Phases

Every simulation tick has explicit phases:

1. The coordinator routes accepted commands to the region that owns the
   affected entity or position.
2. Each lane drains its regions in `RegionKey` order. Commands inside a region
   retain their global enqueue sequence.
3. Regions run local AI and physics against immutable chunk snapshots plus a
   one-chunk read halo.
4. Regions emit migrations and cross-region intents. They do not mutate a
   neighbour directly.
5. The coordinator sorts those messages by tick, command sequence, source
   region, and entity id, then publishes the next phase.
6. Visibility and wire events are emitted only after accepted migrations are
   installed at their destination.

The phase boundary is a push-driven barrier. The coordinator waits for one
completion message from every scheduled lane, not for elapsed time or an
arbitrary tick count.

## Cross-Region Rules

- Entity ids and UUIDs survive migration.
- Every migration has a stable `TransferId` containing the tick, source region
  epoch, and entity id. The source prepares one immutable entity snapshot and
  keeps authority while the destination validates its epoch and reserves that
  transfer id. Neither side exposes the entity twice.
- The coordinator writes one deterministic commit decision at the phase
  boundary. A committed transfer removes the source snapshot and installs the
  destination snapshot as one ownership transition. A rejected or absent
  decision leaves the entity at the source for the next tick.
- Prepare, commit, and acknowledgement are idempotent by `TransferId`. Recovery
  replays the commit decision; it never guesses from which messages happened
  to arrive before a crash.
- Combat, pickup, mounting, and breeding commit in the target entity's region.
  The command carries the observed actor/session generation and position. The
  owner rechecks lifecycle, distance, and permissions before mutation.
- Projectiles crossing a boundary migrate before their next local physics
  step. Segment collision at the boundary uses the captured halo so a target
  is neither skipped nor hit twice.
- Save barriers collect immutable snapshots from every lane for the same
  completed phase. Disk IO remains outside lane ownership.
- Entity checkpoint acknowledgement is memory-only. The saved checkpoint's
  lifecycle and owner-sequence watermark makes older append-only WAL records
  replay-safe, so checkpoint cleanup removes their exact identities from the
  in-memory pending set without queueing a rewrite or `fsync`. New durable
  mutations therefore append directly instead of waiting behind checkpoint
  compaction. Normal journal shutdown compacts the retained pending set before
  stopping and joining the single FIFO writer. A failed shutdown compaction may
  leave replay-safe old records on disk but cannot invalidate already durable
  appends.
- Disconnect and shutdown close queues, reject unapplied player commands, and
  wait for exact lane completion before the final save snapshot.

## Migration Strategy

### Pre-R1: Ownership and fencing model

Implement and test region boundaries, deterministic lease order, phase tokens,
epoch fencing, and stale command/result rejection without touching production
authority. This is correctness scaffolding, not routing or a speedup.

### R1: Routing without authority duplication

Add `RegionKey` and deterministic entity-to-region routing. Run all regions on
one lane using the current ECS authority. Replay output must be bit-identical
to the unpartitioned path. This stage is structure evidence, not a speedup.

### R2: Independent regional stores

Move entities into region-owned stores. Start with two separated loaded
regions and no cross-region interactions. Run their AI and physics on two
lanes, then compare snapshots, semantic events, persistence, and wire order
against R1.

Current implementation: `RegionalEntityStore` keeps independent physical stores
with global entity-id, UUID, and location indexes. Epoch leases fence access,
and exact per-lane acknowledgements fence phase completion. Vehicle/passenger
references remain co-located. `FollowTarget` may cross a region boundary and
uses an immutable target snapshot captured for that goal batch.
`SessionRegistry` owns the persistent regional entity owner runtime directly.
Physical `EntityStore` values live only on their assigned owner lanes; the
coordinator retains ownership indexes, transfer metadata, and phase sequencing,
not a second mutable entity store.
The first R3 primitive uses stable `TransferId` prepare/decision/apply records
inside the coordinator-owned store. Source authority remains visible until
commit, commit preserves id and UUID, reject leaves the source unchanged, and
all three operations are idempotent during their phase. Completed phases clear
their transfer records. This is not yet a durable prepare/commit/ack journal
and cannot recover a decision after process loss.
Regional physics application now updates same-region kinematics directly and
turns boundary motion into a prepared transfer carrying position, rotation,
velocity, and on-ground state. The destination still remains invisible until
the coordinator commits that transfer. A vehicle and its passenger chain use
one transfer record, reserve every member, and move through batch remove/insert
as one ownership change. A deterministic top-level vehicle leader prevents
physics input order from changing the group delta. Reject and rollback release
or restore every member together.
The store also exposes global id/UUID lookup, deterministic id-ordered
snapshots and simulation visitors, indexed breeding/sheep visitors, and
phase-fenced point mutations for velocity, animal state, goals, and damage.
Goal ticks now prepare active ids per physical region, resolve pathing outside
the stores, and apply the region batches with aggregate statistics. Results
are fenced by store authority, phase, and unacknowledged lane, so a foreign,
stale, or late batch cannot mutate a store. Cross-region follow batches also
carry the complete target and follower input snapshots; movement, migration,
identity replacement, or goal changes between prepare and apply reject the
batch before mutation. Production now resolves independent regional pathing
batches concurrently after releasing the authority mutex. The ticker resolves
one batch inline; additional batches run on Rayon's persistent pool and each
holds one non-blocking permit from the shared autoscaler CPU budget. Idle
regions do not enter the parallel path. The same permit-backed lanes now apply
the resolved region batches concurrently after one authority/lease/source/input
preflight. If any input is stale, no region mutates; otherwise each worker owns
one disjoint mutable store and aggregate statistics merge after the exact scope
barrier. This proves concurrent goal compute and mutation, not a measured
speedup.
The runtime controller changes persistent owner-lane count only when shared CPU
admission changes. Its per-tick `Hold` path never sends a reconfiguration
command or clears selected read routes; drain remains an explicit forced
transition to one lane.
The network owner may reuse its already-selected active snapshot batch for
goal preparation through an opaque authority/version token. The regional actor
accepts the token only while it still owns the same authority and global
version and only when the batch completely covers the requested ids; otherwise
it performs a fresh coordinator read. Any owner mutation invalidates the local
cache. Apply remains independently fenced by exact lane snapshots and CAS, so
prepare reuse is a read-amplification optimization rather than borrowed
authority or a mutation lease.
Every active goal entity now carries its complete prepared snapshot through
resolve. Apply rejects the regional result if any input changed, so non-pathing
goals such as Idle and AquaticWander cannot overwrite newer motion or
lifecycle state. Physics apply likewise returns accepted authoritative
kinematics after local mutation or boundary migration; session publication,
chunk indexes, visibility, and movement packets consume those states instead
of speculative worker steps.
The owner CAS now returns the exact committed kinematics batch. The network
adapter projects that acknowledged batch into its short-lived access cache and
performs one current-state read before publication, instead of acknowledging a
boolean, rereading the owner immediately, invalidating that result, and reading
the same entities again. A stale all-or-nothing CAS returns an empty batch.
Movement publication now copies accepted kinematics and prior tracker state,
then builds the wire plan without holding the session registry. Recipient
discovery reads an `ArcSwap` index rebuilt only when a session connects or
disconnects. Each entry references that session's own immutable `ArcSwap`
visibility set, so ordinary movement ticks do not traverse sessions or
visibility edges under the global registry mutex. Visibility writers publish a
replacement set only after reserving the corresponding ordered spawn/despawn
command; movement therefore cannot reserve an earlier sequence for a newly
visible entity. Commit still reacquires the registry, compares the tracker
state with the copied value, and rechecks current visibility before publishing.
The same snapshot boundary now copies player positions and tracker inputs, then
releases both ECS access and the session registry for pickup-distance filtering
and movement-plan computation. Pickup admission, chunk/visibility mutation,
and the final tracker/visibility commit still reacquire the registry.
This removes recipient discovery and packet planning from the global critical
section without allowing a stale plan to overwrite newer state. Chunk
visibility mutation, tracker CAS, and outbound session publication are still
centralized; this is not a fully lock-free or fully regional publication path.
Goal input snapshots exclude dead sessions before hostile target selection.
Attack-time filtering remains a second authority fence, so a dead player is
neither followed by a new goal tick nor damaged by a stale attack candidate.
Goal apply now also exposes a narrow typed projection for the active physics
set. It reads sorted alive kinematics from the lane-owned ECS stores after the
successful CAS and avoids a second full-snapshot materialization on the common
path. A stale or empty batch returns no typed projection; the network caller
uses a rare current full-state fallback to rebuild active membership and
physics metadata coherently. Cross-region movement of a prepared local input
is rejected before lane mutation. Only ids present in the fenced goal input set
use typed projection; active grazing entities excluded from goal CAS always use
the current full-state path. Owner/lane/journal errors remain errors. This is
an incremental ECS boundary, not removal of the full-snapshot CAS input or
direct shared access to lane worlds.
Local physics application now also mutates independent physical region stores
concurrently inside one fenced production phase. The coordinator keeps
boundary transfers serial, including atomic vehicle/passenger migration, while
the caller and permit-backed Rayon tasks apply same-region kinematics. Batches
smaller than 257 states stay inline, matching the existing physics-compute
threshold, and autoscaler scale-down to one CPU disables extra workers. The
global authority mutex still excludes unrelated point mutations during this
phase, so this is concurrent regional mutation but not final region ownership.
The actor-side cached kinematics path groups multi-entity updates into one
`SetKinematicsBatchIfCurrent` mutation per affected region. If every cached
standalone route belongs to the same owner lane, that lane commits the batch
directly, so independent lanes retain cross-call concurrency. A multi-lane or
uncached update falls back to the coordinator's equivalent regional grouping.
This prevents the ECS `PhysicsApply` schedule from traversing a region once per
entity while retaining the single-entity low-latency path and the batch CAS
fence. Deterministic tests count one physics schedule run for 76 same-region
entities, one per region for same-lane and multi-lane updates, zero on stale
rejection, and prove journal-failure rollback restores the complete batch.
Collision-backed canonical pathing facts are initialized synchronously before
the entity ticker is spawned. The prewarm returns a non-zero readiness value
that is moved into the ticker task, so first-use table construction cannot land
inside a simulation tick. Startup logs expose the initialized state count.
Cross-region batch spawn and restore now share an all-input preflight and
publish global indexes only after every physical store accepts its group.
EntityStore batch restore inserts all snapshots before rebuilding vehicle
links, so forward passenger references survive and invalid passenger graphs
fail instead of being silently sanitized. References to an entity already in
transfer are also rejected before insertion.
Vehicle/passenger groups now migrate atomically, while followers stay in their
own region and consume fenced remote target snapshots. Regional goal compute
and dense local physics mutation now use autoscaled parallel paths, but real
region-owned command workers, removal of the global authority mutex, durable
recovery, cross-region interaction fanout, and measured throughput evidence
remain before a full multicore claim.

The persistent owner-lane runtime uses bounded push queues to wake long-lived
workers that physically own
multiple `EntityStore` values. A coordinator consumes `RegionalEntityStore`,
keeps only ownership/global indexes/transfer metadata, and moves the physical
stores to deterministic lanes without duplicating authority. Reads use exact
request/reply messages. Mutation phases use prepare, commit, and finalization;
if any lane rejects prepare or commit, ready or committed peers receive abort
or rollback before the coordinator closes the phase. Every lane applies in
`(RegionKey, sequence)` order and advances a coordinator-supplied sequence
watermark, including empty batches, so old commands cannot replay after region
reassignment. Failed startup returns the handed-off stores, empty cutover
starts the requested owner lanes, unfinalized shutdown rolls back, and
coordinator shutdown drains every lane before returning full or explicitly
partial recovered state. Tests cover reverse arrival, same-phase corrected
retry, cross-phase replay rejection, commit rollback, cross-lane stale-lease
abort, clean round-trip, empty-world region install/spawn, and startup recovery.
Lua villager binding discovery is an owner command, not a session-snapshot
scan. A validated radius of at most 64 blocks intersects at most four 128-block
regions; the coordinator sends every relevant lane request before receiving
results, and each lane scans only the requested stores. Selection requires an
alive exact `minecraft:villager`, a full three-dimensional squared-distance
match, and a deterministic entity-ID tie-break. The same coordinator turn
installs a bounded opaque claim and reverse entity index, so concurrent callers
cannot bind one villager twice. Claims expire after 600 simulation ticks and
are purged by the pushed lifecycle-epoch command, with no timer, polling, or
wall-clock wait. Claims are ephemeral and are deliberately omitted from world
persistence and owner shutdown snapshots.
New regions use a stable least-loaded-lane assignment. The worker creates the
empty physical store after an exact install message, and authoritative spawn
publishes coordinator ID/UUID/location indexes only after prepare, commit, and
finalization succeed. Removal uses the same protocol, returns its exact
snapshot, restores it on rollback, and drops global indexes only after
finalization. Insert rollback also restores the physical store's ID allocation
watermark, and lane preflight rejects duplicate inserted IDs or UUIDs across
regions before mutation. Batch reads fan requests out to every owner before
waiting for exact replies, merge by entity ID, and validate coordinator
ID/UUID/location indexes. Conditional animal updates carry complete expected
snapshots into each owner; one stale parent aborts the cross-lane phase with no
partial cooldown, while a retry from fresh snapshots commits both lanes.
Same-region kinematics use the same complete-snapshot fence. Standalone region
crossings conditionally remove from the source owner and insert the updated
snapshot at the target owner in one coordinator phase; location changes publish
only after finalization. A stale source aborts an already-prepared target with
no duplicate or movement. Vehicle/passenger and referenced-goal crossings are
rejected until their group protocol moves to owner lanes. Every touched store
also checkpoints pending and published semantic event queue lengths. Rollback
restores state first and then truncates speculative events,
so insert/remove/damage rollback cannot leak plugin or wire-visible output.
Damage uses complete-snapshot CAS and returns the authoritative post-finalize
health/lifecycle result; stale damage is a zero-mutation rejection. This runtime
also prepares AI goal work on the persistent owners and applies resolved goal
batches through the same atomic phase. Complete follower and remote-target
snapshots fence commit; rollback restores kinematics and truncates speculative
semantic events, so one stale region cannot publish partial movement.
Save barriers send the expected finalized sequence watermark and complete lease
set to every physical owner before collecting immutable snapshots. A lane with
pending work, a different watermark, or a different lease epoch rejects the
barrier. Restore batches preserve identities and local vehicle graphs in one
phase. Vehicle crossings remove the complete exact-snapshot group from the
source and insert a leader-delta-adjusted group at the target; coordinator
locations change only after finalize and ordinary rollback restores the source
graph plus event checkpoints.
Callers reach the coordinator through a bounded actor handle. Each typed command
has an exact reply channel; current commands cover reads, restore, atomic point
and herd spawn/remove, animal/goal/item CAS, conditional physics, damage, goal
prepare/apply, save, and shutdown.
Goal pathing resolves outside the actor and only the fenced result returns for
apply. The actor owns coordinator metadata and the coordinator continues to fan
work to physical regional owners. Actor startup
does not transfer coordinator ownership until thread creation succeeds, and
joined shutdown returns the recovered regional store. This removes the need
for a caller-side store mutex once the complete production command surface is
routed through the handle.
The handle also provides ID-filtered reads, lane status, and complete-snapshot
conditional remove. Conditional remove
updates coordinator ID, UUID, and location indexes only after owner finalize;
stale input leaves both the physical store and indexes unchanged. Owner runtime
construction preserves the configured entity-ID allocation watermark needed by
the production server protocol range. Selected reads reject disagreement with
coordinator location or UUID indexes.
`SessionRegistry` owns this runtime directly without a
`Mutex<RegionalEntityAuthority>`; direct and combined session guards carry a
cloned owner handle rather than a borrowed store. Complete
snapshot CAS protects partial pickup and removal. Split owner/session
publication rechecks exact snapshots before updating the published projection,
so delayed player push, breeding, grazing, or hostile-arrow output cannot
overwrite a newer owner mutation. ID-filtered reads are grouped into one
request per lane, breeding uses owner-maintained indexes, UUID checks use the
coordinator index, and physics reuses a batch-prefetched snapshot set.
Owner lanes now support live scale-up and scale-down. At an idle owner-command
boundary, the source lane detaches the physical store, the coordinator advances
the region lease epoch, and the target lane installs that same store. A failed
target install restores the store to the source under a newer lease. Retiring
lanes are joined only after every region has moved. The runtime control-plane
pushes its changed CPU admission limit to `SessionRegistry`, so chunk admission
and owner-lane count change from the same autoscale decision; production startup
uses that same automatic CPU limit instead of a separate worker percentage.
Push pressure uses one fixed-size coalescing state cell, not an event backlog.
Each chunk stream owns separate queue-saturation and first-chunk-SLA tokens.
Queue pressure changes at the profile's `queue_pressure_percent` threshold.
First-chunk pressure is measured from stream creation or replan to the first
successful chunk packet write and compared with `target_first_chunk_ms`; tick
observations do not guess it. Completion, write failure, replan, and `Drop`
recover only that stream's active tokens exactly once.

The shared cell tracks current source counts plus one pending peak for each
pressure kind. This remains fixed-size while preserving a short
`active -> recovered` transition when both edges occur before the consumer
runs: the peak is delivered first and the current recovery second. One stream's
recovery therefore cannot clear another stream's queue or first-chunk pressure.
State mutation, pending flags, peaks, and the `Notify` wake happen under one
mutex, and the receiver registers its notification before checking the state.
This closes both the full-to-drained parking race and terminal-edge loss without
polling or an unbounded overflow queue. Slow-client shed events coalesce in a
separate fixed pending flag and are pushed from the outbound pressure
notification path.

The controller retains active queue and first-chunk pressure until matching
source recovery. Ordinary zero-depth tick observations therefore continue the
pressure hysteresis while any token remains active; recovery of one kind keeps
the other kind active. Only event-driven recovery of the last source permits
healthy observations to begin scale-up hysteresis.

`RuntimeControlHandle` has no production decision-only `observe`,
`observe_work`, or `request_drain` method. Its crate-internal mutation surface
is one `apply(RuntimeControlOperation, applicator)` transaction. The operation
is a tick observation, pushed pressure signal, completed work observation, or
drain request; the result is a typed autoscale or work-budget outcome. The
handle owns the controller mutex while it derives that outcome, snapshots the
complete proposed controller state, invokes the applicator, and records the
applicator result. CPU admission and entity-owner lane reconfiguration therefore
linearize with the decision that requested them. A drain that linearizes first
causes every later observation to produce `Hold`, but the `Hold` still passes
through the applicator while the same mutex is held. No pre-drain decision can
apply permits or reconfigure lanes after the drain application.

The applicator returns `RuntimeControlApplyError::Rejected` only when it made no
externally visible change or restored its prior resources. That outcome restores
the exact prior controller state and permits an exact retry. If CPU admission or
lane application may have partially completed, the applicator returns
`ControlledStop`; the controller restores its prior policy state, records
`application_stop_reason`, and rejects every later mutation without calling an
applicator. An applicator panic follows the same rollback and fence before the
panic resumes. The caller must turn `ControlledStop` into process shutdown; it
must not retry an outcome-unknown resource change. Test-only decision helpers
remain under `cfg(test)` for isolated policy tests and are absent from production
and downstream public APIs.

Focused in-module regressions cover exact rejection rollback for throughput and
work budgets, drain followed by an applied `Hold` with equal CPU/lane targets,
two concurrent observations with callback ordering inside the controller lock,
outcome-unknown fencing, and panic rollback. Compile-fail examples on
`RuntimeControlHandle` cover the removed public decision-only methods. These
are API and controller-ordering checks, not gameplay, soak, performance, or
replacement-readiness evidence.

On 2026-07-20,
`cargo test -p mc-net --lib control_plane::tests -- --nocapture` passed all 32
focused tests, and `cargo check -p mc-net --tests` completed with five existing
dead-code warnings outside `control_plane.rs`. Production `mc-net` check and
strict Clippy are not evidence for this slice yet: the separately owned
`server.rs` still calls the removed decision-only and compatibility methods and
must move to `apply` before those gates can compile the non-test target.

Mutation phases now derive their participant set from non-empty lane batches.
A local mutation no longer sends empty prepare/commit/finalize barriers through
every configured owner lane. Prepare and commit are still sent to all touched
lanes before the coordinator waits for any reply, preserving concurrent
cross-region execution and atomic rollback. Global mutation sequences are
therefore sparse per lane; the save barrier accepts a finalized local
watermark below the coordinator watermark and still rejects a lane that is
somehow ahead. Focused ownership and coordinator regressions prove that an idle
lane receives no prepare request and that a later save remains stable. This
removes the all-lane tax from local work, but independent mutation commands are
still serialized by the single coordinator actor.
Point and ID-filtered reads now share a coordinator-published direct-lane
cache. A warm read sends one non-blocking request to every selected lane
without entering the actor. Every route carries the exact entity UUID and
region lease; stale or missing routes fall back through the coordinator and
refresh. A cold point read publishes the route for its next caller. Lanes reject
selected reads while a phase is pending or committed but not finalized. Each
owner lane publishes its own monotonic state version. Ordinary point and
ID-filtered reads capture the versions of only the lanes they touch and accept
the fanout only when those versions remain unchanged. A writer in another lane
therefore cannot force an unrelated read back through the coordinator. This
still prevents one result from combining pre-commit state from one lane with
post-commit state from another. Versioned reads used by referenced multi-entity
goal validation carry the same exact lane-version vector instead of a global
writer counter. Direct fanout is limited to 16
concurrent batches, leaving at least 48 slots in each 64-message owner queue for
prepare/commit/finalize traffic. Reconfiguration clears cached lane senders,
and a region crossing invalidates routes before the mutation reply. This
removes the actor from repeated point and selected reads. Warm item-stack CAS
and cached animal-state CAS batches whose exact leases all resolve to one lane
now enter that owner lane directly. Cross-lane animal batches retain the
coordinator's atomic multi-owner protocol. A lane-local admission lock orders
same-lane phases while distinct lanes remain independent; the handle
uses the shared atomic phase/sequence allocator, records the exact post-state
through the production journal, rolls back safe journal failures, and
fail-stops on unknown outcomes before finalize. Save, reconfiguration, journal
clear, and shutdown take the exclusive side of the mutation gate. Global index
changes and cross-region commands remain coordinator-owned. Cold coordinator
reads take shared topology plus their resolved owner admissions, so they cannot
publish lane state before journal durability or after a safe rollback. Cached
ordinary reads are lock-free with respect to distinct lane commits and validate
only their touched lane versions. Referenced goal CAS takes the shared side of
the topology gate,
locks only its selected owner lanes in lane-id order, and validates that exact
version vector before commit. Direct writers in unrelated lanes can continue;
coordinator-owned index changes, actor fallbacks, and reconfiguration still
wait for the topology gate.
The direct helper validates every `(id, UUID, lease)` under the read side of the
mutation gate before releasing the route cache, rejects duplicate IDs, reserves
one sequence per mutation, and journals the complete post-state set as one
decision. Referenced mutations additionally validate every selected entity,
including non-mutated targets, while holding the corresponding lane admissions.
This order prevents reconfiguration, migration, or a target mutation from
making a validated route stale before prepare.
Coordinator fallbacks for lane-local animal state, goals, item stacks,
velocities, damage, and effects now follow the same lock order: shared topology
gate, then successfully resolved touched owner admissions in ascending lane id.
A malformed ownership-to-lane route retains the exclusive topology fence. A
valid fallback stalled on one lane therefore no longer holds the exclusive
topology gate or blocks direct work in unrelated lanes. Spawn, remove,
position/region changes, full snapshot replacement, save/reconfigure, and other
global-index operations remain on the exclusive side.
Cold point and ID-filtered actor reads use the same shared topology and ordered
touched-lane admissions. Full snapshots still use shared topology plus every
owner admission. Goal preparation holds only shared topology: each owner-lane
queue orders its local AI read against local mutations. Snapshot and goal-read
messages use their lane admission only while they are enqueued, then release it
before owner computation. Exact goal-input snapshots and leases reject stale
plans at apply. A slow lane therefore does not hold admissions or stop direct
work in another lane.
Goal selection publishes the exact current simulation-active entity IDs through
`ArcSwap`. Breeding runs after that publication, performs one selected-ID
regional read, and filters the ECS animal state that actually needs a tick.
Unobserved regions neither join the owner request nor age their animals. The
former coordinator and owner-lane all-world breeding snapshot commands were
deleted; breeding planning still runs without retaining session state or owner
admission.
Item lifetime expiry no longer performs a full entity snapshot during every
physics publication turn. Item creation and restore add the entity id to a
simulation-tick deadline index; an expiry turn reads and removes only due ids,
bounded by the existing sweep budget. This removes the steady all-lane
admission and keeps restored items on their original `spawn_tick` deadline.
Removal cancels the live deadline, duplicate scheduling is idempotent, and
stale queue entries do not consume the live-item sweep budget.
Due removal and visibility publication still use the centralized session
registry, so this is a removed global read fence rather than a claim that item
lifecycle is fully regional.
Prepared-goal apply uses shared topology plus the admissions resolved from its
active goal inputs, follow-target sources, lease/batch regions, and any requested
post-apply kinematics IDs. Its multi-lane prepare/commit/finalize remains atomic
across participating lanes, while direct mutations in unrelated lanes continue
independently.
Hostile goal planning now compares the computed goal with the goal already in
the simulation view. Equal wander, follow-position, or idle goals are removed
before the owner call, and an empty diff sends no command. This reduces the
high-frequency coordinator path without weakening referenced-target validation
or multi-entity atomicity for the changed subset.
Production damage now reuses the snapshot already read by the session and sends
`DamageIfCurrent` through the same cached single-lane protocol. The helper
returns the exact post-state captured under lane admission before journal and
finalize, so a concurrent later hit cannot leak into the earlier
`EntityDamage`. Lethal damage reports `Despawning`; physical removal and global
location/UUID index changes remain a separate coordinator-owned command.
The coordinator now maintains the vehicle topology needed for exact kinematics
validation: `vehicle -> passenger` and `passenger -> vehicle`. Partial
kinematics reads only the connected vehicle components containing requested
entities, grouped by owner lane, instead of cloning every entity in the world.
A dense batch that already covers the whole world keeps the simpler
all-snapshot path. Plain batch spawn no longer scans existing stores; only a
batch that introduces vehicle links pays for full graph validation. Per-lane
request counters prove west-only kinematics does not read east and a plain west
spawn batch reads neither existing lane. Vehicle spawn, migration, removal,
and restore tests cover topology-index maintenance. The bounded debug benchmark
now reports both dense and active-subset modes. Earlier runs were constrained
to logical CPUs `0,1`, which are SMT siblings on this host and therefore do not
constitute multicore evidence. They measured dense p50/p99 `45.6/49.4 ms` with
one lane and `40.5/45.3 ms` with two. Moving 512 entities in two of eight
regions measured p50/p99 `11.5/12.3 ms` and `10.3/11.1 ms` respectively.
Standalone kinematics that stays inside its current region now skips the
coordinator's duplicate snapshot read and submits one exact-CAS mutation and
one global sequence per touched region instead of one per entity. The owner
lane still preflights the whole regional batch before applying it, and the
existing prepare, durable journal decision, commit, rollback, and finalize
protocol is unchanged. Vehicle topology, passengers, and region crossings
continue through the full path. On the same bounded SMT-sibling benchmark,
the final path measured dense p50/p99 `19.052/20.926 ms` with one lane and
`16.614/18.067 ms` with two; the 512-active case measured `4.755/5.131 ms`
and `4.340/4.883 ms`. This is a current-head run-to-run reduction of the common
batch cost, not multicore evidence. The benchmark now rejects Linux affinity
that exposes fewer distinct physical cores than owner lanes. On physical cores
`0,2`, the final path measured dense p50/p99 `19.388/21.276 ms` with one lane
and `11.996/19.823 ms` with two; the 512-active case measured
`4.901/5.716 ms` and `3.503/4.769 ms`. This proves lane-level multicore gain for
one batched kinematics command. The elevated parallel p99 was measured while a
game was competing for CPU under `nice`; combined chunk/entity throughput,
oversubscription, and a quiet-host p99 load gate remain required before a
full-server scaling claim.
An ignored diagnostic benchmark now separates raw ECS apply, direct coordinator
transactions, and the production actor path for the same 512-entity batch. On
physical cores `0,2`, current-head debug p50 measured `1.418 ms` raw ECS,
`4.017 ms` through a directly called coordinator, and `3.420 ms` through the
actor. The inversion between direct and actor runs under competing host load
shows that actor enqueue is not the dominant scaling limit; removing the actor
would discard sequencing ownership without recovering the transaction cost.
Single-participant mutations now fuse owner preflight and apply into one
message. The coordinator still records the durable decision before finalize,
and journal failure still rolls the committed lane back; unknown append outcome
still fail-stops. Multi-lane mutations retain separate prepare and commit
barriers so no lane applies before every participant has validated its input.
On physical CPU `0`, the fused 512-entity actor path measured p50/p95/p99
`4.491/4.560/4.885 ms`. This is a bounded current-head debug result, not a
quiet-host production throughput claim.
Worker loss after commit but before every lane acknowledges finalization still
requires the durable decision journal described by this ADR. Local undo can
roll back surviving lanes after ordinary rejection, but it cannot recover a
physical store from a dead worker. Therefore a channel close or worker panic
during commit/finalize is a fatal runtime condition, not a recoverable rejected
phase, and blocks production cutover until journal replay exists. Expected
startup and shutdown errors remain recoverable: returned partial state is
pruned so ownership, location, and UUID indexes name only stores and entities
that were actually recovered.
The coordinator now has the exact journal insertion point and a narrow backend
contract. After every participant reports a successful commit, it captures only
the touched entities as complete upsert snapshots plus removed IDs and records
that decision before sending the first finalize. A record failure rolls every
applied lane back and returns `Journal`; successful finalize asks the backend to
clear that phase. The default backend is disabled and adds no snapshot traffic.
Production persistent worlds now use a versioned JSON backend containing the
complete owner-state delta, including custom attributes, goals, vehicle links,
and animal state. Each replacement file is flushed, `sync_all`ed, atomically
renamed, and followed by a directory sync. Startup overlays pending decisions
on the last entity save, restores the merged owner state, and only then clears
the recovered phases. Existing age and pickup-delay metadata survive snapshot
replacement. A bind-restart regression proves exact state replay and durable
acknowledgement. This closes the process-restart journal gap, but synchronous
durability cost and the remaining fail-fast adapter path still require p99 and
fault-injection evidence.

### R3: Migration and boundary interactions

Implement entity transfer, projectile crossing, pickup, combat, mounting, and
breeding across boundaries. Each behavior needs an exact boundary regression
and deterministic replay before the global `EntityStore` is removed.

### R4: Regional world mutation

Only after entity ownership is stable, move chunk and block-entity mutation to
the same region owners. Multi-region structures and transactions use ordered
region messages; they never acquire two region stores at once.

Chunk streaming now receives the already-created `WorldReadView` and
`WorldMutationView` from `ConnectionWorld`. An already-resident chunk is read,
lit, conditionally published, and encoded without acquiring the global
`WorldStorage` mutex. Light publication first compares every neighbourhood
source token, then installs only while the same resident snapshots remain
current. There is no constructor `try_lock` fallback: disk misses, generation,
LRU admission, pressure flush, and other storage work keep the global writer,
while the resident delivery path has its owner handles before work starts. A
push-driven regression creates the stream while that writer is already held
and requires packet delivery before releasing it; its timeout is failure-only.

The first production R4 path covers random block ticks whose complete read and
edit footprint stays inside one 8 by 8 chunk region. Planning uses immutable
published snapshots. Commit uses exact block-state and mutation-token
preconditions under the resident region lock, including leaf-neighbour tick
scheduling and light-change metadata. Persistent worlds reserve a world-journal
decision, mark the touched resident chunks with that decision, append the full
post-mutation chunk images, and clear the pending fence before any client or
entity side effect is published. Journal failure requests controlled shutdown;
the path never retries an outcome-unknown mutation through `WorldStorage`.
Tests prove completion while the global world writer is held, exact stale-plan
rejection, and restart-readable journal state. Cross-region plans still use the
coordinator fallback. Grouping independent regional plans under one ordered WAL
append, then dispatching those groups to independent lanes, is the next R4
step; no full-server multicore claim follows from this first path.

The same resident transaction now covers the common sheep-grazing block edit.
The planner deduplicates competing sheep by food-block position in deterministic
candidate order. Entity completion and wool regrowth happen only for edits
present in the accepted resident outcome, after journal durability and block
publication. A held-global-writer regression covers the action tick. A batch
whose footprint crosses a region boundary keeps the ordered coordinator path;
atomic cross-region world/entity transactions remain future R4 work.

Scheduled fluid processing also uses region ownership for its common path.
The coordinator no longer drains due ticks before planning. It selects an exact
per-chunk due prefix from immutable snapshots, and the resident region verifies
that prefix together with all block state/token preconditions. One mutation
then consumes the prefix, applies flow edits, schedules follow-up fluid and leaf
ticks, and stamps every touched chunk with the same journal decision. Stale
preflight leaves the due queue unchanged. A journaled restart test proves the
flowing block and future tick queue are recovered together while the global
writer is held. A footprint crossing an 8 by 8 boundary keeps the old
coordinator commit, which now exact-claims every due prefix before mutation.
This removes both global dequeue and commit from one-region fluid work; it does
not yet run independent regions concurrently in one tick pass.

Scheduled block processing now uses the same exact resident queue-and-block
transaction when a due batch contains only buttons, leaves, or stale entries.
The transaction consumes the immutable-snapshot due prefix, applies exact-CAS
edits, schedules adjacent leaf ticks, and journals the resulting chunks before
publication. Stale preflight leaves the queue untouched. Hopper tick backfill
also updates resident chunks without the global writer. Hopper-only
same-region work now consumes each due tick in the same regional transaction as
its hopper, chest, furnace, and follow-up tick changes. Persistent worlds stamp
and fence every changed chunk under one decision before dispatching container
updates. All hopper commits in one scheduled pass share one decision and one
append. A rejected resident preflight records a durable empty decision for its
reserved journal ID before any coordinator fallback, preventing reservation
holes from poisoning later appends. Comparator-containing batches and hopper
transfers crossing an 8 by 8 boundary still use the exact coordinator claim and
ordered block-entity path. Button, leaf, and stale-entry batches that cross an
8 by 8 boundary use an ordered resident-owner transaction without reacquiring
the global `WorldStorage` writer. Prepare records immutable expected-present
snapshots and expected-absent optional neighbour chunks, then builds every
post-state image in unpublished memory.

Reservation, push-driven append-turn waiting, source verification, durability,
and publication run in one synchronous blocking-worker closure. Aborting or
dropping the async caller cannot cancel that closure after reservation. Before
the WAL append, the transaction takes exclusive resident mutation admission;
ordinary region mutations take shared admission. It rechecks every expected
present snapshot by identity and rejects every absent-to-present neighbour race.
The transaction owns the exclusive publication admission guard through source
verification, WAL append, and publication. Readers and ordinary mutators block
on the shared side of that `RwLock`; OS lock wakeup, not polling, resumes them
after the writer unlocks. After append success, publication makes the generation
odd, installs each owner while holding exactly one region lock, updates the read
and scheduled-tick views, returns the generation to even with release ordering,
and unlocks. An incomplete publication guard marks the state fail-stopped and
restores even parity before unlocking; every later reader or mutator rejects
that state, so an unwind cannot expose a partial publication as reusable state.
Because durability precedes installation, these chunks need no pending-LSN
flush fence and there is no installed state to roll back.

A stale or missing source appends a durable empty decision. Snapshot validation
or encoding errors are typed known-before-append failures; they return through
the journal snapshot API without unwinding. A known pre-append or append failure
must append that empty decision before the reservation is released. After that
closure is durable, the scheduled-block handler returns a rejected no-op and
continues. Failure to close is fail-stop. An outcome-unknown append never
publishes live state, poisons the journal, and requests controlled shutdown.
Restart repair and replay inspect the WAL bytes and choose exactly the recorded
outcome; runtime code never guesses whether the attempted decision reached disk.

Active furnace ticks retain their full `(block state, furnace snapshot)` CAS
under the resident region lock and now stamp every changed chunk with one
decision shared by the simulation pass. The coordinator appends the final
unique chunk images once, then clears their flush fences and dispatches viewer
updates. Stale furnaces are replanned from a fresh resident pair only after the
current wave, including an empty decision when nothing applied, is durable.
This preserves conflict semantics without one fsync per furnace or guessed
retry timing.

Active campfire cooking now shares that resident journal-wave boundary. The
campfire session lock protects the in-memory cooking transition while a
synchronous resident block-state/token CAS installs the matching opaque block
entity and decision fence. One final append covers every changed campfire chunk
in the pass before viewer NBT updates and cooked item spawns are published.
Cold chunks are not loaded for ticking. This makes the world-side cooking state
recoverable and bounds WAL appends per pass; it does not yet make the later
cooked item entity spawn exactly-once across process loss.

Ordinary scheduled block passes now preserve their global due order as
contiguous regional waves instead of submitting one batch that becomes
`CrossRegion` as soon as two independent regions are active. Same-wave groups
share one journal decision and final append. Distinct region groups fan out
across autoscaler CPU permits; groups assigned to the same worker lane keep due
order. Repeated region order such as `A, B, A` stays sequential, and each group
is replanned from the state published by the preceding commit so it still
finishes in the current server tick. A group's complete planned edit footprint
still has to fit one region; otherwise the preceding resident wave is made
durable, the group crosses the ordered coordinator barrier, and only then does
the next resident wave begin. The coordinator path uses the same ordered
pre-stamp, flush-fence, append, and recovery protocol as other block edits. This
removes the global writer from the multi-region common case without weakening
same-region order or boundary atomicity. Autoscaler scale-down to one CPU keeps
the pass inline.

Random-tick planning now partitions the common mutation path before commit.
Contiguous groups retain global sample order and original indexes for
deterministic seeds. Each later group is planned from the state published by
the previous commit. A conservative four-block boundary belt separates known
neighbour scans, while exact edit/precondition ownership preflight remains the
authority for wider families. Cross-region plans first make the preceding
resident wave durable, then stamp and append their coordinator snapshots under
one reserved decision before publishing side effects. Before mutation, the
coordinator stamps the prospective edit and leaf-tick footprint with exact flush
fences and never decreases a newer chunk LSN. If pre-stamp observes a newer
decision, the old empty decision is closed and the global writer is released;
the retry waits for the journal append cursor through a notification, then
locks and revalidates from unchanged coordinator state. Checkpoint poison and
writer closure wake waiters; ordered prefix appends retire earlier reservations
without polling. Drops are accepted only from the plan whose source edit
committed. Interior plans for separated regions share one journal decision and
append. Distinct owner regions now execute concurrently under autoscaler CPU
admission; scale-down to one CPU keeps the existing inline path. Candidate
indexes preserve deterministic RNG order, while repeated owner regions and any
boundary group stay sequential and replan from published state. Every worker
prospectively stamps and fences its loaded edit and leaf-neighbour footprint
before mutation. A worker panic is contained per job: all lanes drain, every
stamped post-state is appended, and only then does the pass fail-stop without
publishing block deltas or drops. This removes the global writer and sequential
regional commit from the independent random-tick common case without weakening
ordered boundary semantics.

Changed entity goals without an entity reference now use the same cached
same-lane durable commit path as item and animal state mutations. A successful
full snapshot read publishes exact owner routes while holding exclusive entity
mutation admission; the later direct batch revalidates ID, UUID, lease, and
expected snapshot under shared admission before committing and journaling its
post-state. Duplicate IDs, cache misses, multiple owner lanes, and goals that
reference another entity still fall back to the coordinator. In particular,
`FollowTarget` keeps coordinator validation and atomicity across the follower
and target owners.

Dirty high-water handling is deliberately bounded: each pass drains at most 64
dirty chunks, then relies on producer-pushed tail convergence instead of
turning pressure handling into an unbounded checkpoint. Full checkpoints remain
the interval, disconnect, shutdown, and explicit save paths. Focused tests pass;
the P44 runtime gate has not yet been rerun for this slice.

Movement fanout uses an adaptive gate rather than unconditional dispatch: it
selects the fanout path only when `S * M > 2E`, and `M = 0` sends no fanout
work. Three focused tests pass. This is a bounded work-selection change, not
runtime throughput evidence; a performance rerun remains pending.

Entity simulation admission now reads a published atomic live-session count.
Registration, persisted death state, survival death/respawn, and unregister
publish a generation while changing the authoritative session state. A
transition to zero live players pushes hostile-goal reconciliation through the
regional owner after releasing `SessionRegistry.inner`. If another player
transition happens during that work, the stale generation replans from the
newly published player set. Positive transitions do not block login on entity
journal work; the next simulation event performs ordinary target selection.
Empty/all-dead steady-state ticks therefore take neither the session mutex nor
an entity-owner snapshot, without leaving disconnected or dead targets behind.
Active-player selection and movement publication still retain their documented
centralized metadata boundaries, so this is not full world regionalization.

The `wide` SIMD experiment remains non-promoted. Its kernel median gain was
`7.86%` and its full-path median gain was `0.72%`, both below the 10% promotion
threshold. The scalar production path and its existing correctness fences stay
in force.

## Acceptance Gates

- Single-lane replay is bit-identical for entity snapshots, semantic events,
  persistence, and wire ordering.
- Two separated busy regions execute concurrently and improve measured dense
  throughput without increasing p99 tick time or losing entities/events.
- Negative coordinates and every 8-chunk boundary route deterministically.
- Migration conserves entity count and identity through restart.
- Cross-boundary projectile, combat, pickup, vehicle, and breeding tests have
  no duplicate or lost mutation.
- Autoscaler scale-up and scale-down wake lanes through notifications and do
  not move a live region between owners mid-phase. Commands and results from a
  previous region epoch are rejected.
- Shutdown drains every lane and produces a zero-dirty final save in the
  bounded multiplayer gate.

## Non-Goals

- One thread per region.
- Parallel mutation of one hot region.
- Configurable worker percentages or manual subsystem budgets.
- A second mutable ECS authority kept in production.
- Claiming a speedup from routing scaffolding or a single-lane test.

## Consequences

Separated player groups can use multiple cores without contending on one ECS
mutex. One crowded region remains single-writer and deterministic; later hot
region splitting is allowed only if profiling shows it is necessary. The main
cost is an explicit one-phase protocol for cross-region work and a revised save
barrier. This is larger than further lock trimming, but it removes the global
serialization ceiling instead of polishing it.
