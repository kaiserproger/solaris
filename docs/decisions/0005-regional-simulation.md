# ADR 0005 - Regional simulation ownership

**Date:** 2026-07-16  
**Status:** Accepted; production authority for moving entities  
**Supersedes:** ADR 0004's single entity writer

## Decision

Moving entities use regional single-writer ownership. The world is partitioned
into fixed 8-by-8-chunk `RegionKey` values using Euclidean division. Each active
region has exactly one `RegionLease` and one lane-owned `EntityStore`. A lane may
own several regions; a region never executes on two lanes at once.

Mutable ECS state stays on its owner lane. Immutable tick input, compact
semantic results, and versioned publications may cross lanes. Cross-region work
uses deterministic ordered transactions. Region keys, leases, ECS references,
and owner handles remain internal and are not plugin API.

This is a modular-monolith boundary, not a second server or a distributed
consensus design.

## Current authority

The regional ECS owns entity identity, UUID uniqueness, lifecycle, retained
state, goals, paths, physics inputs, kinematics, combat state, vehicle links,
item and experience state, and entity persistence snapshots.

The coordinator owns only shared topology and exceptions:

- `EntityId` and UUID routing to the current lease;
- lease/epoch changes and lane reconfiguration at completed boundaries;
- deterministic multi-owner prepare, commit, finalize, rollback, and recovery;
- cross-region transfers and interactions;
- save and publication fences;
- exceptional entities or operations that cannot complete inside one owner.

Session visibility, tracker state, chunk indexes, and outbound queues are
post-commit publication state. They do not authorize entity mutation.

## CPU work admission

`ChunkPipelineResources` keeps one fixed shared CPU-worker semaphore, sized
once. Foreground physics, pathing, random/scheduled block planning and regional
fanout use `cpu_capacity()`, not the autoscaler's background target.

Autoscaling changes `prepare_limit()` only: the existing background chunk
prepare-request admission and stream fanout. It does not shrink the shared CPU
semaphore or introduce a priority queue. Already admitted work is not cancelled.
Chunk-queue pressure retains capacity to drain pending preparation; entity-owner
reconfiguration still uses the existing drain fence.

Background preparation starts and recovers at at most `max(cpu_capacity() - 1, 1)`.
This leaves headroom from background work on multi-worker configurations without
reducing the shared foreground ceiling or dropping jobs. Other foreground work
can still occupy that headroom; it is not a scheduled-planning deadline guarantee.
Single-worker configurations retain serial admission.

## Session critical sections

Natural despawn captures the complete natural-candidate set under the session
mutex, then releases that mutex for the six-field despawn projection.
Owner access and lifecycle context stay with the operation. Apply reacquires
the session mutex and refreshes eligible player positions and natural membership;
full current snapshots and conditional removal still fence entity changes.

Chunk unload updates references for every actually removed chunk, then scans
and publishes visibility once against the complete removed-chunk set. It does
not delete authoritative entities or bypass ordered outbound backpressure.

## Current execution path

The ordinary entity tick is intentionally direct:

```text
immutable tick inputs
  -> owner lane: local selection, AI/goals, bounded pathing, ECS mutation
  -> compact goal motion + semantic IDs + region leases
  -> central hostile/breeding/villager transactions that must occur here
  -> owner lane: reread current ECS, exact collision physics, ECS mutation
  -> compact committed tracking motion + exceptional fallback IDs
  -> publication adapter after the regional fence remains current
```

Owner lanes process their regions in deterministic `RegionKey` order. Goal
selection and application remain on the same lane, so the owner-local resolver
may trust that uninterrupted fence; coordinator-driven goal batches retain full
checkpoints. Physics uses a fresh post-goal/post-transaction ECS read and checks
sampled chunk identities immediately before mutation.

Living, powder-snow-walkable living, and aquatic living entities stay on the
local path while they remain in the same chunk and their world snapshot is
current. Chunk crossings, vehicles, projectiles, items, experience, falling
blocks, stale world reads, and unsupported kinds use the existing central
exception path. The local path never changes an entity category to avoid work.

Owner-local collision and pathing reuse the existing immutable
`mc_data::collision_shapes::vanilla_collision_class` table used by central
physics. Empty/full-cube states avoid per-cell binary shape decoding. Canonical
state compatibility and entity-dependent powder-snow rules remain before the
fast path; complex geometry and unknown-state fallbacks retain their behavior.
Every block read and world/publication fence remains. There is no per-entity
block cache or duplicate class table. Startup warms shared classes and canonical
pathing facts before simulation begins.

Published movement is compact. Full snapshots are created only for semantic
operations that require them. A stale publication fence is revalidated against
current owner state before any wire-visible result is emitted.

Chunk-view removal publishes a compact batch of entity IDs per departing chunk,
using the existing vanilla `RemoveEntities` packet. Dense natural populations
must not turn one view change into hundreds of reliable queue entries and
disconnect an otherwise responsive client. Visibility removal is still reserved
under the session lock and delivered through the same ordered reliable lane;
player removals and singleton entity lifecycle publications retain their order.
The queue bounds, population policy, collision admission and authoritative
entities do not change. Only client visibility is removed, and packet encoding
consumes the ID vector without cloning full entity snapshots.

Natural despawn reads every tracked natural entity through `EntityDespawnProjection`:
ID, UUID, type name, position, lifecycle and last-damage tick. Other simulation
readers retain their existing projection. Both kinds share deterministic batching,
committed-state checks, owner-lease validation and returned-location validation
in focused projection modules; there is no separate fast-path authority.
No full retained snapshots or persistent snapshot caches are built for entities
the despawn scan keeps. Categories, player eligibility, distances, idle clocks,
damage resets and deterministic rolls retain their rules. Existing persistent
types skip the unused contract lookup, and missing IDs clear their stale clocks.

Only a removal candidate requires a full snapshot. UUID, type, position,
lifecycle and damage clock must still match before full-state conditional removal
commits. Owner state remains authoritative; publication snapshots never classify
persistence. The owner read runs outside the session mutex as described above;
the apply phase still holds it. This does not eliminate every session stall.

Conditional snapshot replacement batches that keep regions and passenger links unchanged
use owner preparation as the full-state comparison fence. The coordinator keeps
identity/uniqueness, position, claim, routing and committed-state checks, but
does not first fetch another full batch of the same snapshots. Cross-region or
passenger-topology changes retain that preflight before planning index updates.
All-participant preparation, rollback and durable commit ordering are unchanged.
Grazing timer actions are emitted only when their entire update batch succeeds;
they are already a subset of that batch and need no second ID-set filter.

Grazing reads still route every loaded sheep ID in one selected-snapshot batch.
The owner reads the current ECS timer before constructing a full snapshot for
IDs outside the possible idle-start phase. `Some(0)` still enters timer cleanup.
The existing 50-tick baby phase includes every 1,000-tick adult start; the
unchanged planner uses the snapshot's actual age for the final start decision.
This immutable per-request selection is not a population or persistence cache.
Filtered reads use the coordinator and do not publish partial results into the
complete-read route cache. Lease/commit checks and full-state timer-batch CAS
remain unchanged; ordinary complete snapshot reads retain their direct path.

Single conditional replacements now use the same topology criterion to avoid
rebuilding passenger indexes through a full-population snapshot for in-place
state changes. Their existing coordinator expected-state read and owner
preparation remain; relocation or passenger-link changes still validate the
complete graph. This also applies to the existing conversion entrypoint.
The grazing planner borrows the retained owner snapshot vector directly rather
than repacking each large snapshot into a second vector and duplicating age.

## Cross-region protocol

Every routed operation carries the expected lease. Stale leases reject without
mutation.

Multi-owner operations acquire admissions in deterministic lane order and use
one recorded decision:

1. prepare validates leases, identities, expected state, and the complete write
   set without exposing a partial result;
2. commit records and applies the decision in deterministic order;
3. finalize makes the result externally publishable;
4. rollback restores the prepared snapshots when rejection is still known;
5. an indeterminate commit outcome stops the runtime.

Transfers preserve entity ID and UUID. Source and destination never expose the
entity simultaneously. Vehicle/passenger groups move atomically. Save barriers
capture all lanes at one completed phase and retain sequence watermarks needed
for replay.

Simulation saves freeze the immutable dirty-world plan and world-journal cut
while owning the shared `WorldStorage` mutex. They release that guard before
entity/player capture; the simulation owner still excludes subsequent commands.
World decisions accepted after the frozen cut remain beyond that save's
checkpoint acknowledgement.

### Journal durability

The owner-approved gameplay contract is write-behind: a successful mutation is
accepted into the bounded RAM journal queue, not acknowledged as durable storage.
A crash may lose the unflushed tail. Queue pressure delays admission; it never
discards accepted work.

World-chunk and entity decisions use one world-owned `JournalWriter`, queue and
failure signal. Their existing recovery formats remain distinct. The writer
groups up to 64 queued requests, persists reserved ID bounds before either log
can reference them, then syncs the appended batch. Reservations no longer
serialize and replace the accumulated world journal.

Full saves wait for the accepted queue prefix. Every dirty chunk flush plan
also carries the journal barrier, including background and pressure flushes;
this prevents a region file from overtaking its WAL. Clean shutdown drains the
writer, and its lease lasts until all journal owners release it. Any writer
failure wakes the runtime failure observer and append-order waiters; subsequent
mutations and save barriers fail closed. A failure affects both logs because
they belong to the same world's persistence transaction stream.

## Invariants

- One region, one lease, one lane, one mutable ECS authority.
- Reconfiguration occurs only at a completed boundary and increments the
  relevant epoch/version fence.
- Same-region AI and physics never round-trip through the coordinator.
- Goals and physics remain separate because hostile attacks, breeding, and
  villager population/defence may commit between them.
- Goal publication uses post-goal state; physics rereads and validates that state
  rather than trusting a pre-transaction full-population payload.
- Physics samples the complete swept collision footprint. Missing chunks fail
  closed. No entity is treated as grounded or stationary to skip physics.
- Direct pathing still validates finite values, world height, and loaded chunks.
  Terrain-pathing entities additionally use canonical collision shapes.
- Publication follows commit and checks the regional version fence.
- Cross-region ordering is deterministic and replayable.
- Worker or coordinator loss is fatal when commit outcome cannot be proved.
- Waits are message-driven; elapsed time is never evidence of completion.

## Performance evidence

The fixed 100,000-entity workload is the acceptance gate: 50 active players,
40,000 villagers, 60,000 mixed animals, every entity selected each tick, and
8,450 retained chunks.

The latest optimized release-profile run at
`.analysis/bench/living-world-100k-compact-goal-capture/baseline.log` completed
all 1,200 measurement ticks with 100,003 entities at activation and every
active entity selected. It reported tick p50/p95/p99/max of
83.673/91.805/95.431/103.005 ms. Dominant p95 phases were goals 33.197 ms,
physics 32.834 ms, and dispatch 13.581 ms. Owner-local goal apply now emits
only semantic vectors and changed motion while it mutates ECS; the prior full
pre-goal motion vector, full post-goal candidate vector, and mismatch reread
were removed. Matched instrumentation measured owner critical-path p95 at
26.561 ms versus 27.691 ms and owner CPU-sum p95 at 129.636 ms versus
139.719 ms. Whole-tick p95/p99 did not improve in the single matched runs.
The fixed p95 <= 50 ms and p99 <= 60 ms gate still fails, so the architecture
is functionally current but not performance-ready.

No throughput claim may replace this fixed full-population tick-latency gate.
Short runs, reduced populations, extra cadence, and isolated kernels are only
profiling evidence.

The 2026-09-07 debug natural-despawn probe used unchanged populations of
128/512/1024 entities, five warmups and forty measured ticks for farm animals,
hostiles, aquatic mobs, nonpersistent ground animals and mixed populations.
At 1024 entities, homogeneous-population medians fell by 20.3–27.0%; the exact
results are retained in
`.analysis/codex-logs/owner-field-5617830-2026-09-06/despawn-cost/controlled-comparison.json`.
This isolated stationary-population probe does not establish live tick
percentiles, the fixed large-world gate, or elimination of session lock stalls.
The rejected two-pass ground-animal trial is recorded beside them in
`compact-ground-trial.json`: it doubled reads for nonpersistent ground animals.

The subsequent grazing probe kept all 128/512/1024 sheep selected through
idle, active and mixed timer workloads, with five warmups and forty samples
on the same four CPUs. At 1024 active sheep, removing duplicate coordinator
validation reduced median/p95 from 52.756/54.448 ms to 42.323/46.913 ms.
Exact decrements and whole-batch stale rejection remain required. Idle snapshot
reads are still material and did not improve consistently; these figures do
not establish whole-server latency. Measurements and a rejected ownership
cutover are retained under
`.analysis/codex-logs/owner-field-5617830-2026-09-06/sheep-grazing/`.

The idle-read follow-on compared filtered and unfiltered query dispatch with
identical candidate preparation, test instrumentation and nine debug workloads.
At 1024 idle sheep, median/p95 fell from 15.785/16.453 to 3.708/4.188 ms; mixed
median fell from 19.439 to 13.691 ms. The all-active median rose from 41.866 to
43.339 ms (+3.5%), a retained tradeoff rather than an across-the-board win.
The final owner route still recorded a 51.527 ms grazing warning; warning-only
samples do not establish throughput or elimination of live contention. Exact
measurements, unchanged three-seed replay receipts and remaining limitations:
`.analysis/codex-logs/owner-field-5617830-2026-09-06/sheep-grazing-idle/checkpoint.json`.

The active-cost follow-on rejected projection-then-snapshot composition: its
1024-sheep idle/active medians were 7.563/51.269 ms against 3.284/43.995 ms for
the filtered read. Reusing the snapshot vector improved all nine matched cases;
1024 active median/p95 became 40.985/41.673 ms. The final uninstrumented run
measured idle/active/mixed medians of 3.243/41.090/12.695 ms.
Instrumented owner-route attribution localized the live peak to grazing start,
not grass snapshots. After topology-preserving single CAS, recorded start
p95/max fell from 25.171/58.016 ms to 1.067/8.395 ms. These are instrumented
stage samples, not whole-server percentiles. That post-change replay failed
the unchanged degradation gate on an 11.894 ms chunk-prepare lock wait; it is
not a graphical acceptance pass. Final gates and preserved failures are in
`.analysis/codex-logs/owner-field-5617830-2026-09-06/sheep-grazing-read-cost/checkpoint.json`.

Save-lock attribution then found recurring `save_barrier` world-lock holds up
to 21.707 ms; entity capture, not dirty-world planning, dominated that interval.
After transferring the owned guard into save capture, none of eight matched
saves reached the 1 ms wait/hold trace threshold. World planning and cut capture
took 0.314–0.625 ms; entity capture still reached 25.294 ms outside that mutex.
The deterministic regression also checkpoints and reopens the WAL to prove
that world decisions accepted during entity capture remain recoverable.
Final unchanged three-seed graphical routes pass, but scheduled-block warning
samples still reach 154.404 ms and the inspected owner frame displays 10 FPS.
This closes the measured save lock scope, not whole-server latency or visual
acceptance. Temporary clocks were removed; evidence and remaining contention:
`.analysis/codex-logs/owner-field-5617830-2026-09-06/streaming-commit-stalls/checkpoint.json`.

The background-only admission cutover reduced scheduled-planning CPU-admission
p99 from 37.654 ms to 0.110 ms in one matched owner-route pair (8,134 and 8,114
plans). Actual planning p95 stayed at 0.155/0.160 ms. The remaining 95.362 ms
admission maximum occurred before the first logged background reduction; this
is not proof of a stable whole-tick deadline. The uninstrumented L2 and three
unchanged graphical routes pass. Clocks were removed; exact measurements,
remaining session contention and the limited visual evidence are retained in
`.analysis/codex-logs/owner-field-5617830-2026-09-06/scheduled-block-phases/checkpoint.json`.

The session-lock follow-on measured unload visibility p95/max at
11.263/42.781 ms before and 0.918/4.043 ms after batching, with 22 unload calls
in each owner-route run. Despawn projection still costs about 11 ms at p95,
but no longer holds the session mutex. Apply work remains material; this is
not a claim of lower total despawn CPU cost. A regression proves that a nearby
player can join during projection and prevents the formerly eligible despawn.
Matched samples, final gates and retained limitations:
`.analysis/codex-logs/owner-field-5617830-2026-09-06/session-lock-phases/receipt.json`.

The headroom follow-on separated semaphore notification from executor resumption:
98.516 ms of the worst 98.597 ms admission sample preceded notification.
Leaving one worker out of the background ceiling reduced matched admission
p99/max from 2.084/98.597 ms to 0.016/3.020 ms; none of 7,112 candidate
acquisitions polled pending. The 8,174/7,112 plan samples and finished stream
windows contain different realized work, so equal throughput is not established.
Final workspace tests and three unchanged graphical routes pass, as do final
formatting, strict Clippy and code-health. Earlier failed receipts remain failed.
The owner route still reaches 155.537 ms whole tick and 118.228 ms unattributed
time in warning samples; this does not close the 50/60 ms gate or all scheduled
work. Probe source, exact receipts, test migrations and limitations:
`.analysis/codex-logs/owner-field-5617830-2026-09-06/scheduled-stall-followup/receipt.json`.

The narrow-despawn follow-on separated owner projection time from decision work.
Six-field reads and the earlier persistent-type check reduced matched elapsed
percentiles and normalized per-candidate cost. Realized populations and route
durations differ; this is not equal-throughput or process-CPU evidence. Worst-case
despawn and whole-tick time did not improve in that comparison. Final L2 component
gates and the unchanged three-seed graphical routes
pass after import/test cleanup; original failed receipts remain failed.
Full measurements, the independent review's scope, exact source delta and limits:
`.analysis/codex-logs/owner-field-5617830-2026-09-06/unattributed-tick-followup/receipt.json`.

Periodic planning now lives in `natural_spawn_26_1_2/periodic.rs`. Its collision
geometry memo is indexed by the immutable input projection slice, local to one
category call, and initialized only after terrain admission. Individual AABBs
are computed only when collision short-circuiting reaches them. Candidate order,
accepted-box checks, counters, identity, cadence and publication fences do not
change; this is not a persistent cache or a second authority.
With identical instrumentation and owner-route setup, friendly-due planning p95
fell from 63.929 to 9.318 ms (18/17 attempts). Realized populations and work differ.
Whole-tick p95/p99 increased, despite a lower maximum; this is not a whole-server
latency or equal-work CPU claim. Final L2 component gates and all three graphical
routes pass. The stale ownership-rule failure remains retained alongside its
mechanical path repair, independent review scope and visual limitations:
`.analysis/codex-logs/owner-field-5617830-2026-09-06/periodic-planning-followup/receipt.json`.

Owner-local physics now lives in `regional/owner_lane/physics.rs`. Before
refreshing goal-only publication entries, it excludes IDs already replaced by
committed physics motion. Wrong-lane rejection remains before that exclusion;
remaining entries retain their lease, lifecycle and current-kinematics checks.
The merge, world fence, fallback and downstream publication fence are unchanged.
Matched pre-extraction tail p95 fell from 1.617 to 0.159 ms and worker p95 from
9.433 to 8.509 ms (28,548/29,238 nonempty samples). Realized work differs; these
are elapsed times, not CPU or equal-throughput claims. Worker maximum rose from
19.944 to 21.033 ms, and the earlier 43.163 ms outlier did not recur in either
instrumented run. Final canonical correctness and three graphical routes pass
on the extracted, uninstrumented source. The focused regression preserves
publication and stale-state behavior without request-count pins:
`.analysis/codex-logs/owner-field-5617830-2026-09-06/regional-commit-followup/receipt.json`.

The shared-classification follow-on compared 3,693,246 identical query/snapshot
pairs, alternating execution order and checking equal physics results.
Sampler construction plus integration averaged 12.330 → 7.448 µs (39.6% lower
elapsed time); both orders improved. An identical-implementation control showed
0.90% aggregate label bias. Small raw-state/metadata cache experiments and
duplicate derived flags were discarded in favor of the existing shared table.
These are paired elapsed measurements, not CPU or whole-server throughput.
Pathing changed after that comparison; catalog-wide geometry equivalence,
focused behavior checks, final correctness and three graphical routes passed.
The final owner warning-only tick maximum remains 148.525 ms, with dispatch,
preparation and block costs still open. Original shorter water observations and
broad terrain acceptance are not cleared. Source, review, controls and limits:
`.analysis/codex-logs/owner-field-5617830-2026-09-06/physics-sampling-followup/receipt.json`.

The vehicle-read follow-on keeps graph policy in `entity_vehicle.rs` and reads
live ECS identity, lifecycle and optional vehicle/passenger components. Pending
batch snapshots join that compact graph for the same lifecycle, unique-owner
and cycle checks. No retained graph/cache or new authority exists. Removal uses
the canonical ECS passenger unlinking without a redundant EntityStore scan.
Passenger lookup shares the same compact projection; mount/dismount/input and
transaction/fence semantics are unchanged.

On the same instrumented seeded route, unfenced owner-apply median/p95 fell
from 10.152/18.321 to 4.373/10.872 ms. All 3,738/2,575 inputs in that cohort
committed; differing work counts preclude equal-throughput or CPU claims.
A separate compact fenced batch applied 92/130 inputs and remains recorded,
not relabeled as universal acceptance. The external smoke covered 45 graph
cases, 18 removals and 360 paired lookups; the permanent regression preserves
valid shared chain tails and atomic cycle rejection.
Final native correctness, build and all three graphical routes pass, alongside
one independent read-only optimization review. The owner tick still reaches
119.981 ms and negative-seed dispatch 98.495 ms in warning-only samples.
The broad tick target and isolated earlier water observation remain open.
Source, measurements, final receipts and removed probe sources:
`.analysis/codex-logs/owner-field-5617830-2026-09-06/entity-dispatch-followup/receipt.json`.

## Remaining migration

- Meet the fixed 50/60 ms gate without reducing active work, changing gameplay
  order, or adding a second scheduler/cache/authority.
- Remove temporary load-benchmark phase instrumentation after the final accepted
  profile.
- Continue moving only genuinely cross-region policy out of root orchestration;
  delete obsolete coordinator APIs as their last callers disappear.
- Keep world/block ownership outside this ADR until a separate measured and
  correctness-fenced migration requires it.

## Rejected alternatives

- One thread per region or parallel mutation inside one region.
- A second mutable ECS, mirrored authoritative snapshot map, or category-based
  side authority.
- Cohort, cadence, LOD, natural-despawn cache, grounded-body skip, or zero-
  velocity skip presented as a performance fix.
- Reordering or fusing goals and physics across intervening gameplay commits.
- Speculative asynchronous physics, shared mutable world sampling, or retry on
  outcome unknown.
- Operator worker percentages and benchmark-only workload changes.
- Generic schedulers, queues, caches, journals, and compatibility adapters added
  beside the existing owner lanes and transaction journal.

## Consequences

Separated regions execute concurrently without a global ECS mutex. Ordinary
local work has a short direct path; coordination cost is paid only for topology,
transactions, recovery, and exceptional behavior. A crowded lane remains
single-writer and deterministic. The remaining performance gap must be closed
inside these correctness fences or explicitly renegotiated; it cannot be hidden
by doing less simulation.
