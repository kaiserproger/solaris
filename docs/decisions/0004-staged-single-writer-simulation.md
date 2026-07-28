# ADR 0004 - Staged single-writer simulation boundary

**Date:** 2026-07-11
**Status:** Accepted, staged migration

## Context

`PROJECT_SPEC.md` requires one synchronous game-state writer. ADR 0003 accepted
`WorldHandle` and `SessionRegistry` locking as a transitional architecture.
Prompt 02 now supplies deterministic contention, conservation, persistence,
duration, and focused real-client gates that can guard an ownership migration.

The current entity ticker already provides a natural 20 TPS phase owner, but
network tasks still call entity claim and mutation methods directly. Replacing
the whole runtime or introducing ECS at this point would discard the working
runtime as an executable oracle.

## Decision

Introduce an explicit `SimulationHandle`/`SimulationOwner` pair backed by a
bounded Tokio MPSC queue. Every accepted command receives a monotonic sequence
at enqueue time. The owner awaits `Receiver::recv`; command arrival wakes it
immediately, it drains at most 256 commands from the 1024-entry queue, sorts the
batch by sequence, applies it synchronously, and sends typed outcomes to
requesters. The 20 TPS clock still drives game time, goals, and physics, but it
is not a polling interval for packet-authored commands.

The first authority transfer covered item, experience-orb, and grounded-arrow
claims. The next bounded extension moved player melee combat and command
summons through the same owner. Bow release now uses a composite owner command:

- network tasks may discover nearby candidates and calculate inventory space;
- they must request the authoritative claim through `SimulationHandle`;
- only `SimulationOwner` may remove or partially mutate the item/XP entity;
- the owner dispatches visibility events before replying, and the network task
  only mirrors the successful typed inventory/XP outcome for packet encoding;
- melee damage, knockback, lethal removal, and planned item/XP rewards commit in
  one owner command;
- `/summon` and projectile creation require `SimulationAuthority`; direct
  production mutation helpers for those paths no longer compile;
- bow ammunition, durability, and projectile creation commit together after
  exact selected-bow and arrow-stack validation.

An accepted command has no arbitrary response timeout. Queue-full and
queue-closed requests fail before mutation. On shutdown the owner closes the
receiver, rejects queued-but-unapplied commands, and then exits. Cancellation
is checked immediately before mutation. A requester can still disappear in the
small interval after apply and before oneshot delivery; eliminating that final
window requires each composite action to include every affected player field.
Migrated pickup, use, drop, and survival/death commands now do so; remaining
window-0/crafting, active-use, and pose paths are explicit Prompt 03 debt. Active
saves now use the ordered barrier described below.

Queue depth, maximum depth, enqueued, processed, full, closed, cancelled, and
maximum batch counters are read-only runtime telemetry. World commands also
expose unavailable and mutation-failure counters. Commands already queued at a
periodic tick run before entity goals and physics; commands arriving between
ticks run from the channel wake without advancing game time.

The first world extension routes CAS-protected survival break and placement
commits through `ApplyBlockEdits`. Packet tasks still calculate immutable edit
plans and capture state plus per-position mutation-token preconditions. For a
batch containing world commands, the owner enters the FIFO wait queue of the
Tokio world mutex. Unlock wakes it directly; it then validates all preconditions
and commits the batch while it owns the world guard. There is no timed retry or
tick polling. A missing world is a typed fail-closed outcome; the requester
preserves inventory/tool state and resends the authoritative block state.
Creative, unconditional interaction, scheduled/random/fluid, falling-block
landing, block-entity, and persistence mutations remain legacy.

The follow-up world extension routes every production call to
`apply_visible_block_edit_batch`, including doors/toggles, buckets/cauldrons,
hoe/plant/bonemeal edits, falling-block starts, and farmland trample, through
the same owner. Commands without explicit preconditions are applied in owner
sequence order. Queue/world rejection returns no applied edits and sends the
requesting client authoritative cached block states instead of panicking or
debiting inventory/tool state. This does not invent CAS evidence: survival
break/place retains state plus mutation-token preconditions; the other plans
remain ordered but unconditional until their planning snapshots carry explicit
tokens.

The first shared-container extension routes chest and furnace click commits
through typed owner commands. A connection task reads a consistent
world-snapshot/viewer-version pair, applies the click to speculative
connection-local inventory/cursor plus an immutable block-entity snapshot, and
submits expected and updated world, inventory, and cursor snapshots plus an
optional throw/drop plan. Under the world guard, the owner compares the block
entity, viewer version, registered inventory, and cursor while the session lock
keeps the world write, player replacement, viewer-version advance, and item
entity creation in one turn. Rejected commands return authoritative world and
player snapshots. Accepted commands publish peer container/drop visibility
before responding, so requester loss cannot split or hide the commit. Furnace
ticking, hopper/server-origin container writes, window-0/crafting clicks, and
global persistence remain staged.

The packet-authored block-entity extension routes sign text and campfire item
insertion through typed owner commands. Sign commits compare the current block
state and per-position mutation token before replacing opaque NBT. Campfire
commits additionally compare the complete cooking snapshot and hold the
campfire-state mutex while writing persistent NBT under the world guard, so
the two representations commit or reject together. Held inventory is debited
only after owner acceptance; rejection resends the authoritative block. The
server-origin campfire cooking tick/drop, furnace tick, hopper transfers, and
save IO remain legacy, so this is packet-authority evidence rather than full
block-entity or world authority.

Every packet-authored simulation command is fenced by the connection's
monotonic `SessionId`. Production packet code receives a session-bound handle;
an unbound handle cannot enqueue a player command. Immediately before dispatch,
the owner rejects a fence that is no longer active and records a dedicated
stale-session counter. This prevents queued work from a disconnected client
from mutating migrated state after reconnect under a new session. Detached
server-owned work, such as passive herd spawning, remains deliberately
unfenced. The check is a Prompt 03B prerequisite; authority moves only for the
explicit transactions listed below.

Passive herd admission and world-time changes now share this owner ordering.
Player-authored time changes carry the active session fence; console, startup,
and other trusted server-authored changes use an explicit unfenced server API
that still enqueues the same typed command. There is no production direct time
setter. Consecutive detached herd commands retain the two-command background
admission limit but may commit their chunks through one UUID-deduplicated
regional batch and publish only after that batch succeeds. Same-chunk producers
wait on one completion claim and receive the winning enqueue result. A journal
failure known not to have committed restores the exact herd/pending claim and
releases admission for retry; an unknown outcome consumes the claim and leaves
the regional owner fail-stopped so the UUID-stable spawn cannot be duplicated.
Time transition and pending-hostile claim are linearized under the session
registry lock, which is released before regional owner or journal work.
`TimeSet` is also an ordering barrier for earlier detached herd commands: the
owner admits at most two of those commands in the current background batch,
keeps the barrier prefetched until every lower sequence is processed, and
leaves later herd commands deferred behind it.

Committed herd publication has a narrow batch fast path. It applies only after
the authoritative durable batch succeeds and while the session registry lock
fences visibility state. The path installs every entity index and published
snapshot first, groups publications by chunk, traverses the session table once,
and then emits packets in the original entity-major order. Its structural cost
is `sessions x unique batch chunks + actual dispatches`, instead of one full
session scan per entity. Other entity spawns keep the generic single-entity
publisher as the fallback. Remove this fast path if a general publication
planner subsumes it, or if runtime measurements show that its grouping cost is
not beneficial; the structural reduction alone is not performance evidence.

Physics prepare reads are detached from the session registry lock. The runtime
prefetches the versioned CAS input first, reads item-expiry and prior motion
state from ECS, then takes session state only to plan publication. After commit,
it reads authoritative post-state again under the publication lock and derives
the published snapshot projection from that state. Those reads are intentional
correctness fences: returning only the accepted input can publish stale motion
after a newer commit, and vehicle-group migration can move a passenger to a
different authoritative position than its requested state. They may only be
removed after owner commit and session publication share a monotonic token that
orders physics, removal, migration, expiry, and packet-visible dispatch.

Goal apply has a typed post-commit projection fast path. The regional owner
returns only sorted alive `EntityKinematics` for the requested active ids after
the successful goal CAS, so the common network ticker path no longer
materializes complete entity snapshots merely to construct physics queries. A
stale or empty batch returns no projection. The caller then performs a rare
current full-state read and rebuilds lifecycle, active-area membership, AABB,
physics kind, and kinematics from that one state. Transaction, lane, and
journal errors still fail instead of falling back. Regional preflight also
rejects a prepared local input that has moved to another region; it cannot be
silently omitted from the lane expected-set. The full prepared snapshots and
lane CAS remain the correctness fence. The projection request contains only
ids in that fenced goal input set. Active entities intentionally excluded from
goal CAS, currently grazing sheep, always use the current full-state path and
are merged back in stable entity-id order. Remove the narrow projection when
ECS consumers read typed components directly across the owner boundary.

The physics-kind projection also carries dynamic powder-snow collision
capability. The exact 26.1.2 walkable-mob tag and falling-block identity are
resolved while the entity owner state is available, then consumed by the
snapshot sampler without another entity lookup or lock. ECS motion accumulates
downward travel and resets it on landing; the typed query carries that retained
distance so the sampler can apply vanilla's leading long-fall 0.9F branch
before the falling-block and walkable-mob branches. The sampler applies these
dynamic shapes only after the embedded collision table accepts the exact
block-state fingerprint; unknown or altered states keep the conservative
material fallback.

The first Prompt 03B player transaction originally moved item-entity claim and
inventory credit into one session-owner operation. The first public-alpha run
showed that the regional entity actor could spend 38-79 ms committing its
journal while `SessionRegistryInner` and `PlayerPersistedState` were held. The
production path is now a staged exact transaction instead:

1. copy the current item snapshot, player position, and complete registered
   inventory state without retaining either session or player guards;
2. install a runtime-only owner claim token through a checkpoint-only regional
   full-snapshot CAS; the stack itself remains unchanged;
3. recheck the current session identity/range, release the session guard, and
   commit the complete player inventory only if mode, survival state, inventory,
   and selected slot still match the plan;
4. resolve that token through one checkpoint-only owner command against the current
   entity snapshot, preserving newer kinematics while applying the exact
   remainder/removal;
5. publish the immutable resolved entity result under one short session guard.

A stale session or player plan resolves the same token back to an available full
stack without overwriting newer motion. The regional owner treats the token as an
exclusive item reservation: pickup, merge, remove, stack replacement, lifecycle
replacement, and damage fail closed, while kinematics may continue and are preserved
at resolution. Claim installation, rollback, and finalize do
not append independent regional journal decisions: production saves capture both
player and entity state through the simulation-owner `SaveBarrier`, and the direct
snapshot path is used only after that owner has drained. A crash before the next
barrier therefore replays both pre-pickup states; a completed barrier contains both
post-pickup states. The runtime token is also omitted from serialized checkpoints.
Visibility is emitted only after both resource commits. Requester loss after
application still cannot lose the credit or its entity event. The connection only
mirrors the returned inventory
snapshot for wire encoding. Until all inventory actions migrate, a deliberately
named legacy sync hands the connection-owned inventory mirror to the registered
snapshot immediately before this transaction; it is transitional debt, not a
claim that the full player aggregate is owner-controlled.

World-lock contention is resolved by the mutex's unlock notification. The owner
awaits that exact event and applies the original command once; requesters do not
guess when the lock might become available and do not schedule blind retry
commands. Unit coverage holds the mutex, proves the owner future is pending,
releases it, and observes one commit with one queue entry.

Experience-orb removal and player XP credit use the same Prompt 03B transaction
shape as item pickup. The owner locks the registered player snapshot while it
removes the orb, applies the exact positive value to `XpState`, and emits the
take/despawn visibility events. The connection only mirrors the returned XP
snapshot and writes `ClientboundSetExperience`. Requester loss and stale
sessions therefore cannot remove an orb without preserving its credit. A named
legacy XP sync remains while the connection mirror exists. Survival-command XP
changes and death reset now move through the player survival transaction below.

Grounded-arrow pickup also commits removal and inventory credit together. The
owner validates that the entity is a stationary grounded arrow, plans one
arrow against the registered inventory capacity, then applies both aggregates
or neither. Visibility dispatch is owner-side and the connection only mirrors
the returned inventory snapshot. Claim-only arrow removal remains test-only.

Timed survival breaking now uses one composite owner command for the complete
block CAS set, held-tool durability, and deterministic item-drop creation. The
connection still performs reach/mining-time validation and plans edits and
drops from one world snapshot, but the session-bound handle supplies the actor
generation. The owner bounds the payload, validates every edit token plus the
registered selected stack before mutation, then applies world edits, the
persisted inventory replacement, and item-entity spawns during one owner turn.
Peer block deltas are queued before drop visibility and before the response;
the requester only mirrors the returned inventory and writes its block, ack,
slot, and light packets. Stale tokens/sessions/stacks, queue rejection, and
sustained world-lock pressure therefore leave block, tool, and drops unchanged.
Lighting, fluid scheduling, falling-block follow-up, campfire cleanup, and an
explicit save barrier remain staged and are not covered by this transaction.

Survival placement now commits the complete block edit set and one-item debit
from the selected stack in one session-fenced owner transaction. The
connection still plans reach, face, orientation, collision, and multi-block
shape. The plan captures a state/token precondition for every target, door, and
cactus-cascade edit while it holds the world snapshot, then submits those
preconditions plus the exact selected stack. The owner validates and commits
world and persisted inventory together, publishes peer block deltas before
responding, and returns immutable block and inventory snapshots for requester
packets. Sign UI, lighting, hopper scheduling, and an explicit save barrier
remain staged follow-up work.

Food completion now uses a session-fenced owner command for the exact selected
stack and exact survival snapshot. The owner holds the registered player lock
while it validates and replaces inventory plus hunger/saturation, then returns
immutable snapshots for `SetSlot` and `SetHealth`; requester loss cannot split
the debit from the food credit. Use duration is measured in simulation ticks,
and owner tick advance wakes the connection through a `watch` notification.
Use start/cancel is still connection-local, so this is completion authority,
not yet full active-use or player-aggregate authority.

Bow release now validates the exact selected bow, exact supported hotbar or
offhand arrow stack, and active session before one owner turn debits one arrow,
damages or breaks the bow, and creates the projectile. Projectile visibility is
dispatched before the response; the connection only mirrors the returned
inventory and writes slot packets. The old production `SpawnArrow` command and
connection-local debit/durability path are removed. Draw start/cancel remains
connection-local, and projectile hit handling remains a separate owner phase.

Selected-item drop now validates the active session, selected hotbar slot, and
exact held stack before one owner turn debits either one item or the full stack
and creates the matching item entity with its owner pickup block. Visibility is
dispatched before the response, and the connection only mirrors the returned
inventory. Shared chest/furnace throws use the composite container transaction;
window-0/crafting throws and disconnect settlement use the player inventory
owner command, while their connection wire mirrors remain staged.

Player survival transitions now validate the exact registered survival,
inventory, cursor, and XP snapshots before one owner turn replaces health,
hunger, XP, and damage-related inventory state. A transition into death clears
the post-damage inventory and cursor, creates every matching item entity,
creates the recoverable XP orb, and resets XP under the same owner lock. A
duplicate or stale transition therefore cannot duplicate drops. Fall,
campfire, projectile, hostile, starvation, admin, exhaustion, and survival
respawn paths submit this command and mirror its immutable result. The cursor is
now represented in the registered aggregate. Chest/furnace clicks validate and
replace it through their composite owner transaction. Window-0 and crafting
clicks use the player inventory owner command; non-empty 3x3 input is compared
and replaced in that same turn. Close/disconnect clears the owner projection
while returning its stacks, and a stale connection projection is rebuilt from
the rejected command's authoritative response. The projection is runtime-only:
playerdata/restart recovery and cancellation cleanup are not yet closed.
Active-use start/cancel and other transient container inputs remain staged.

Active save entrypoints now enqueue a session-independent save barrier. In its
ordered owner turn, the barrier captures immutable player, entity, world-time,
and simulation-tick snapshots after every lower-sequence command. Disk IO uses
those returned snapshots instead of rereading live session state. A dedicated
post-drain save API covers final shutdown after the owner channel has closed.
The barrier also records the dirty-chunk count at that owner turn. If a journal
fence excludes any dirty chunk from its flush plan, save waits for the exact
producer notification that releases or fails that fence and then captures a
new complete owner snapshot. It never acknowledges a partial plan based on a
retry count or elapsed time.

Dirty region writes produce temporary files outside world ownership. Before a
normal background flush replaces one whole Anvil region, it validates every
resident snapshot represented by that image. If any represented chunk changed
while the image encoded, that region is skipped before filesystem installation
and remains dirty for a later plan; stable independent regions in the same
batch continue. Only a filesystem version mismatch is a `StaleRegion`. The
synchronous wrapper replans resident conflicts within a bounded attempt budget
and reports a typed failure if the region never stabilizes. An active barrier
save instead installs the exact planned region image; chunks changed after the
barrier remain dirty in memory and are covered by their later journal records.
The final post-drain save must reach zero dirty chunks. A remaining
journal-pending chunk is an invariant error because no producer remains to
release it. Region installation itself is still serialized with resident
publication. Directory sync runs on a blocking worker after releasing resident
and world locks; a short finalization turn then marks only snapshots that still
match as clean. This is an ordered world and simulation snapshot, not a global
transaction across playerdata, entity data, region files, and every journal.

Prompt 04 introduced a standalone `bevy_ecs 0.18.1` runtime inside
`EntityStore`. The dependency is pinned below the 0.19 line because Solaris is
on Rust 1.94 and Bevy ECS 0.19 requires Rust 1.95. Default features are disabled
and only `std` is enabled; chunks, block entities, connections, registries, and
Tokio handles remain outside ECS.

Prompt 04 initially kept the legacy `EntityStore` as authority and compared its
state and ordered semantic events with ECS in the same owner turn. That
temporary comparison authority and its feature have now been deleted. ECS is
the sole entity authority; vanilla oracle tests and focused domain tests replace
the deleted in-process comparison path.

The migration evidence included all then-current `mc-entity` tests, all 531
`mc-net` library tests, a deterministic mixed-family persistence/restart replay,
an explicit 72,000-tick accelerated replay with no divergence, and a debug
1,000-entity density report. Those historical measurements justified the
authority transfer but do not describe a retained runtime mode.

Prompt 05 transferred production authority to ECS for item/XP entities,
projectiles/falling blocks, passive and hostile mobs, command-summoned
entities, and vehicles. Every spawn and persistence restore now calls the direct
ECS API. `EntityStore` is ECS-only and has no legacy spawn/restore path.
Network, visibility, collision, combat, pickup, and persistence code read
immutable snapshots reconstructed from ECS; stable runtime ids, UUIDs, and wire
ordering remain unchanged. Vanilla captures and domain oracle tests provide
comparison evidence without retaining a second authority or comparison feature
in production or test builds.

`EntityStore` is held by its own measured mutex beside `SessionRegistryInner`,
not inside it. Goal scheduling, pathing preparation, and physics-query
construction release the session mutex before touching ECS. Cross-state
transactions still take session then entity locks in that one order. A
concurrency test holds the session lock and proves an entity snapshot read
completes without waiting for session release. Runtime logs report entity-lock
pressure separately.

Authoritative AI and position integration run as filtered Bevy ECS systems.
The first implementation rebuilt complete snapshots in each hot pass and
measured `12,138 us/tick` for 1,000 entities over 200 debug ticks. Direct ECS
queries reduced it to `482 us/tick`; the same-host legacy SoA result was
`163 us/tick`. The remaining measured blocker is debug Bevy schedule/query
overhead, not snapshot cloning. It remains explicit input to the SIMD and
autoscaling prompts rather than a hidden performance claim.

Prompt 05 focused evidence currently includes 42 passing `mc-entity` tests
(two explicit long/benchmark gates ignored by default), all 534 `mc-net`
library tests, ten concurrent transaction tests, nine persistence tests, the
detached-lock test, six mob wire tests including two-client visibility, two
falling-block wire tests, and the bow/arrow wire test. The retained legacy SoA
code has no production caller and exists only as the Prompt 04 test oracle and
density baseline; it is not a second runtime authority.

Fresh real-client evidence covers P21 natural hostile combat and P39
two-client movement. Both runs passed the in-client scenarios and the final
artifact validator. A P38 two-client item handoff also passed all four bridge
phases, but its diagnostic artifact exposed entity physics waiting behind the
chunk pipeline CPU semaphore. Entity physics now takes only immediately
available worker permits and computes other batches on the simulation owner;
it never queues the tick behind background chunk work. The repeated P39 run
then validated without a slow-tick warning.

Prompt 06 has started with two interaction-side scheduling paths. Button
release ticks are planned with the block edits and scheduled by the owner only
when the mutation-token-checked edit commits. Fluid ticks caused by applied
player edits are sent as a bounded detached command; waiting for queue capacity
uses the channel's wakeup, and the connection task no longer locks the world to
schedule them. Scheduled/random tick execution and other server-origin world
mutations remain staged.

The first server-origin relight slice covers random ticks, scheduled block and
fluid ticks, and falling-block landing. Each path captures immutable chunk
sources under the world guard, computes incremental light and wire data after
releasing it, then publishes only when every captured `Arc<Chunk>` is still
current. A source conflict falls back to a full current-world recompute while
holding the guard, so stale light is never sent. The mutation passes themselves
and packet-authored owner command relight remain under the shared world guard.

Server-origin furnace ticking is also owner-entered. Discovery and the common
recipe/timer calculation use the producer-published furnace view. The commit
phase rechecks authoritative state under the shared world guard but now releases
that guard after each independent furnace instead of retaining it across the
whole active set. A conflict recompute and each individual commit still happen
under the shared guard; this bounds one hold but is not world sharding.

The hopper-to-campfire path no longer publishes runtime cooking before world
persistence. It builds the candidate while holding the cooking-state lock,
persists the candidate, and publishes it only on success; failure therefore
does not tell the hopper to debit its source slot. The final hopper block-entity
write and campfire persistence are still separate world mutations, so the
general cross-block transaction remains staged.

The session implementation is now organized into private direct-child modules
for `container_views`, `entity_owner`, `entity_simulation`, `outbound`,
`pathing`, `prepared_chunks`, `sleep`, and `transactions`, with `tests` as a
direct child; the parent remains about 8,697 lines. This is an extraction of the
existing `SessionRegistry` boundary, not a new authority or threading boundary.
During integration, the living-entity physics path was corrected to derive yaw
and head yaw from the collision-resolved horizontal velocity, commit that
rotation through the entity owner, and use the returned owner-applied rotation
for wire-state comparison and movement publication. The direct-child regression
test checks both the persisted authoritative rotation and the outbound movement
rotation.

Current focused evidence is `mc-entity` `179 passed/7 ignored`, `cargo check`
and strict Clippy for that crate, plus passing `cargo check -p mc-net --tests`.
No final full-workspace validation has run for the completed extraction. No
readiness claim follows from this checkpoint.

The sole-ECS persistence hardening slice now treats a regional journal entry's
`(phase, sequence watermark, lifecycle epoch)` as its durable cleanup identity.
An append with unknown outcome remains pending in memory, recovered entries keep
their exact identity, and checkpoint acknowledgement removes only the identity
captured by that checkpoint. Entity checkpoint format 3 also persists head yaw,
the current goal, vehicle state, the lifecycle epoch, and the regional sequence
watermark; save and load reject missing, dangling, cross-region, multiply-owned,
self-referential, or cyclic vehicle graphs.
Full-snapshot ECS CAS rebuilds both vehicle indexes before publishing a valid
replacement and leaves both ECS and indexes untouched on stale or invalid input.
The simulation lifecycle clock now advances the regional owner epoch before
lifecycle mutation, and restore reinstates the checkpoint's epoch and sequence
boundary. Decision creation reads that owner epoch. The regional sequence is a
global monotonic watermark: advancing the lifecycle epoch never permits replay
below the checkpoint's sequence watermark. Replay still filters decisions from
an older lifecycle epoch and same-epoch decisions at or below the checkpoint,
so pre-checkpoint removals or timer values cannot overwrite post-checkpoint
state while a newer same-epoch decision still replays. A later-epoch decision
at or below the checkpoint sequence is invalid recovery data rather than a new
sequence domain.

Every applied WAL upsert is semantically preflighted before replay publishes a
checkpoint: its entity type must exist in the canonical 26.1.2 registry, and
its living and active-effect state must be accepted by the ECS runtime
conversion. Invalid WAL therefore returns typed invalid data before ECS restore,
without partial publication or a panic. Replay projects persisted age and pickup
delay from each restored ECS snapshot's retained ticks at the final replay
lifecycle boundary; new valid WAL-only spawns remain valid format-3 checkpoint
input and repeated replay remains idempotent.

Required arrow and item retained state is installed in the same ECS spawn
transaction as identity and kinematics. Entity physics and item expiry read ECS
snapshots; the published snapshot map is only a wire projection rederived after
an accepted ECS mutation or invalidated on removal. The former test-only entity
access and command-spawn compatibility wrappers are removed, and tests enter
through the same owner lock and authority-gated spawn APIs as production.

## Staged Authority

1. Item, XP, and grounded-arrow claim/removal, plus timed survival
   break/tool-damage/drop creation.
2. Moving-entity lifecycle and combat mutations. Player melee, its kill rewards,
   command summons, bow projectile spawn, passive herd spawn, lifecycle clock,
   goals/physics application, arrow-hit resolution, and persistence restore are
   migrated. Generic/player item and XP spawn plus falling-block/world coupling
   remain staged.
3. Player-authored block mutations and scheduled world results. All visible
   packet-authored block-state commits are owner-ordered; survival break/place
   is additionally CAS-protected. Button release and interaction-triggered
   fluid scheduling are owner-ordered. Scheduled/server-origin execution is
   staged.
4. Shared containers and block entities. Network chest/furnace click commits,
   including their player inventory/cursor and throw/drop side, sign text, and
   player campfire insertion are migrated. Server-origin furnace/campfire
   ticking and hopper transfers enter through the owner, but their shared-world
   and cross-state commit internals remain staged. Window-0/crafting item-drop
   creation, other block entities, and persistence are staged. Timed survival
   break, selected-item drop, and death-drop creation are migrated.
5. Finish Prompt 03B by moving window-0/crafting clicks, active-use state,
   authoritative pose, and disconnect settlement out of connection tasks.
   Inventory, cursor, XP, and health/hunger are already owned for the migrated
   composite transactions but still have legacy mirrors. Active save entrypoints
   have an ordered owner snapshot barrier; global atomic disk persistence does
   not.
6. Remove the remaining shared-world mutation lock in Prompt 06.

Each stage must replay Prompt 02 inputs against the legacy operation and the
queued operation before production authority moves. The legacy helper may
remain only under `cfg(test)` once its production callers are gone.

## Consequences

This creates a real single-writer boundary without changing wire behavior.
Prompt 04 supplied the comparison oracle; Prompt 05 makes ECS the production
moving-entity authority and detaches it from the session mutex. Migrated
commands wake the owner directly instead of adding one tick of polling latency,
while bounded batches retain queue-pressure control. World state and several
player aggregates remain staged, so this does not claim the whole runtime is
already single-writer.
