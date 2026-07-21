# ADR 0006 - mc-net module boundaries

**Date:** 2026-07-18
**Status:** Accepted, staged migration
**Related:** ADR 0004 and ADR 0005 remain authoritative for mutation ownership

## Problem

`mc-net` grew around one Play connection loop. `play.rs` now contains packet
decoding, gameplay rules, persistence coordination, owner requests, and packet
publication. `session.rs` mixes session indexes, entity ownership, combat,
containers, spawning, and outbound routing. Child files often use
`use super::*`, so the file split does not create a dependency boundary.

This makes ordinary gameplay work depend on connection internals, lets domain
results contain ready-made network commands, and makes unrelated changes
collide in the same large files.

## Decision

`mc-net` will be split by vertical gameplay domain. Each migrated domain has
one small concrete contract and one dependency direction:

```text
play driver -> domain request -> simulation/session authority
                                -> semantic result -> publication adapter
                                                       -> outbound transport
```

The Play driver owns packet decoding, connection-local state, socket writes,
and translation between wire packets and domain requests. A domain owns its
rules and request/result types. Simulation or session owners validate and
mutate authoritative state. Publication adapters turn accepted semantic
results into recipient-specific outbound commands after mutation completes.

A domain module must not import `play::*`, `InteractionState`,
`OutboundCommand`, `ServerConfig`, or session internals. It uses explicit
imports and concrete request/result types. Traits and generic service layers
are added only when two real implementations require them.

Outbound commands are owned values. Fanout constructs one independent command
per recipient. A connection loop is never an authority receipt endpoint:
authoritative mutation completes in the simulation/session owner before a
publication command enters any client queue. Socket progress therefore cannot
decide whether gameplay committed.

No network write or packet encoding occurs while a gameplay lock or regional
owner lease is held. No new global lock is introduced by a module extraction.
ADR 0004 and ADR 0005 continue to decide where mutations run.

## First Migration: Combat

`play::combat` is the first boundary because melee already crosses decoding,
cooldown rules, target ownership, victim authority, publication, and wire
tests.

The first accepted slice moves damage kinds, typed damage requests, pure
hurt-resistance transitions, active-shield identity, and knockback rules into
`play::combat`. PvP validation and mutation run as one simulation-owner
command. The same owner transaction validates and commits target damage plus
attacker exhaustion and weapon durability. Server-entity attacks use the same
cost commit instead of changing the connection-local inventory after damage.
Only then does the simulation owner enqueue `PlayerDamageCommitted` and hurt
events; attacker socket progress is not part of publication ordering.
The authority adapter lives in `play::session::player_combat`; it uses explicit
imports and cannot depend on `InteractionState` or `ServerConfig`. The pure
domain remains independent of session and outbound types.

The simulation command no longer accepts caller-supplied game mode or player
pose. It carries only attacker identity, target, damage, and an optional
expected cost plan. The owner reads the current pose, mode, death state, and
target identity from authoritative session state and rechecks them at commit.
Concrete `PlayerEntityAttack` and private commit context values keep that
boundary readable without a trait layer or long positional argument lists.

Victim publication is an expected-to-updated delta, not a replacement player
snapshot. Health, XP, carried item, and changed inventory slots apply locally
only while their expected values still match. This prevents an older victim
publication from overwriting newer exhaustion or weapon durability committed
when reciprocal attacks are processed in the same owner turn.

The earlier victim one-shot receipt design is rejected. It made acceptance
depend on the target's sequential connection loop, so reciprocal attacks could
leave both loops waiting on commands queued to each other. It also reported a
socket-write failure as rejected after authoritative state had already
committed. Hurt resistance now uses preview/commit: the next resistance state
is installed only after the target CAS succeeds. Active shield identity is
validated under the same commit fence. Base melee knockback is calculated from
the authoritative target-pose snapshot and publication only encodes that
result; player velocity is not yet persisted by player authority, so this does
not claim complete motion authority.

The bundled 26.1.2 oracle fixes the shield front hemisphere at an inclusive
90-degree angle and processes item blocking before hurt resistance. A fully
blocked direct melee hit still publishes the base shield-response impulse.
Solaris uses that confirmed `0.5` impulse, but cannot yet compose it with old
player velocity or the later generic impulse because player velocity is not an
authoritative persisted component.

`play::session::outbound` now has explicit imports. It remains a transport DTO
module rather than a generic event bus; semantic event extraction proceeds one
gameplay domain at a time.

`play::session::player_state` owns the concrete survival-plan CAS and its
death/respawn side effects. Combat calls that narrow authority contract instead
of importing persistence mutation helpers from the `session.rs` parent. A
successful dead-to-alive commit clears hurt resistance under the same owner
lock.

`play::session::entity_combat` now owns server-entity melee validation,
hurt-invulnerability, lethal transition, rewards, and the authority-fenced
attacker-cost commit. Explosion and projectile coordinators call its concrete
locked operations; the implementations no longer live in the `session.rs`
parent. The extraction does not add a lock or change regional ownership.

Other accepted concrete boundaries in this staged migration are:

- `play::containers::furnace` and `play::containers::chest` own pure menu
  state, slot mapping, and click rules. `play.rs` retains packet translation,
  owner requests, and outbound publication.
- `mc_data::fuel_values` owns the immutable default-feature 26.1.2 furnace-fuel
  snapshot. A complete sidecar is resolved in vanilla builder order and must
  match the canonical 280-item membership and durations; embedded startup uses
  the equivalent repo-owned derived table. `play::containers::furnace` and the
  scheduled hopper path consume this one snapshot rather than matching item
  names independently. Smoker/blast-furnace halving and final non-flammable
  wood removal therefore cannot drift between menu admission, ticking, and
  automation.
- `play::campfire` owns cooking state transitions, recipe projection, block
  identity, persistent NBT compatibility, and client block-entity NBT.
  `play.rs` retains ticking and recovery coordination, storage writes, packet
  publication, and the D1 intent -> entity materialization -> D2 acknowledgement
  ordering.
- `play::session::pickups` owns item, arrow, and experience pickup claims,
  inventory/XP credit, pickup candidate planning, item/XP materialization, and
  pickup-specific dispatch construction. It also owns the exact-tick readiness
  index that pushes item candidates when a stationary item becomes collectible;
  this avoids both a full entity scan and movement-dependent polling. Deferred
  entities such as campfire outputs are indexed only when their durable owner
  commit has succeeded and the entity is published.
  `SessionRegistryInner` keeps the authoritative maps and lock acquisition in
  `session.rs`; generic entity cleanup and selected-item authority also remain
  there. The pickup module currently imports the concrete outbound DTO because
  publication extraction is staged; it releases authority locks before the
  indexed readiness dispatch is sent.
- `play::containers::crafting` owns crafting-table window state, slot maps,
  shaped/shapeless/repair result rules, inventory projections, and wire-item
  construction. Click application, active-container ownership, packet I/O, and
  simulation commits remain in `play.rs`; this is an explicit staged boundary,
  not a claim that the complete menu coordinator has moved.
- `play::containers::enchanting` owns enchanting-table window state, offer and
  item rules, bookshelf geometry, slot projections, and wire-item construction.
  `play.rs` retains the concrete world-cache and player-slot wrappers together
  with packet handling, active-container ownership, click application, and
  simulation commits. The child module does not import `InteractionState`.
- `play::containers::stonecutter` owns stonecutter window state, offer
  filtering/selection, input projection, slot mapping, wire items, and pure
  pickup/quick-move rules. `play.rs` retains packet writes, active-window
  ownership, stale client fences, and the simulation-owner commit. The child
  module uses concrete recipe/item/inventory inputs and does not import
  `InteractionState`.
- `play::session::visibility` owns player/entity visibility mirrors, snapshot
  publication, recipient planning, and wire movement state. Registration,
  chunk tickets, lifecycle ownership, and lock acquisition remain in
  `session.rs`. Entity spawn, despawn, event, and movement publication reserve
  a per-session sequence while the existing session owner lock establishes
  plan order. Actual channel delivery occurs after that lock is released. A
  per-session atomic assigns the sequence; only out-of-order delivery uses the
  bounded per-session FIFO mutex. Dropped reservations publish an explicit
  cancellation entry, so an early return cannot leave a permanent sequence
  hole. Non-coalescible overflow still fails closed through the existing
  slow-client disconnect path. Entity movement is last-state reliable: once a
  recipient channel is full, adjacent movement batches in the per-session
  retry lane coalesce by entity to the newest absolute
  position, velocity, rotation, and ground state. This preserves command order
  without retaining every obsolete relative delta. The play-loop writer caps
  each movement write turn. Channel/retry limits bound command-count growth,
  and coalescing never grows a movement vector with previously absent entity
  IDs. The socket stall timeout remains the dead-peer fence. Queue occupancy
  alone is not a disconnect condition while writes continue.
  Wire position state follows the 26.1.2 `ServerEntity` tracking contract.
  Position is dirty when unquantized displacement squared is at least
  `7.62939453125E-6` or the global tracking-update count is divisible by 60;
  only the resulting relative delta uses quantized absolute endpoints. Each
  scheduled tracker update advances that count and the teleport delay once,
  including when a stale physics proposal is rejected; that update plans from
  the current authoritative motion instead of the rejected proposal.
  Signed-short overflow, an `on_ground` transition, or the incremented
  teleport delay exceeding 400 selects absolute `EntityPositionSync`, which
  is the only event that resets the delay. Opportunistic body-push publication
  updates the global baseline without advancing either counter. Wire selection
  distinguishes position, body rotation, combined position/body rotation,
  absolute sync, and independently packed head yaw. Arrow relative/body
  updates use the vanilla combined position/body packet shape. Selected motion
  precedes movement, and head rotation follows it. The state lives with the visibility
  mirror and adds no timer, polling loop, lock, or operator tuning.
- `play::session::projectiles` owns arrow spawn/expiry, hit resolution,
  knockback, projectile candidate scans, and segment/AABB geometry. It receives
  the existing `SessionEntityGuards` from `session.rs`; it does not acquire a
  lock or send to a channel. Generic lifecycle, player AABB, kill-reward data,
  and hostile bow coordination remain in `session.rs`. Projectile publication
  keeps the existing visibility helpers and recipient semantics.
- `play::session::container_state` owns the sharded chest/furnace viewer
  registry, timed guards, player-container reads, recipient snapshots, and
  container-specific test probes. `SessionRegistry` retains the existing shard
  fields and initialization, unregister lifecycle ordering, generic inventory
  and item-drop authority, and actual dispatch. The extraction preserves the
  literal session -> container order for registration/unregister and container
  -> player-persistence order for commits; production code does not send while
  either guard is held. Its immediate consumers, `container_views` and
  `transactions`, use explicit imports rather than inheriting the parent
  module namespace.
- `play::fluids` owns deterministic fluid tick planning, flow and contact
  rules, nearby rescheduling, delays, identifiers, and fluid-state
  construction. `play.rs` retains due-tick ownership, resident/global commit,
  durability, relight, interaction dispatch, and publication. The child uses
  concrete read-only planning inputs and has no world writer, session, async,
  packet, or configuration dependency.
- `play::session::campfire_authority` owns the campfire cooking registry
  operations, conditional tick/ack/cooldown, legacy commit, recovery probes,
  and regional transaction preparation. Registry fields and initialization,
  regional transaction commit, simulation queue, storage writes, publication,
  and D1/entity/D2 recovery coordination remain in their existing owners. The
  existing lock order is unchanged; only `#[cfg(test)]` push probes send on a
  oneshot channel.
- `play::toggles` owns pure door, trapdoor, fence-gate, button, and lever
  planning together with adjacent power propagation and button release
  scheduling. `play.rs` retains loaded snapshot acquisition, scheduled-tick
  classification, world commit, journal, relight, packet handling, and
  publication. The boundary rejects writer, session, lock, async, and packet
  dependencies.
- `play::session::explosion_authority` owns expired explosive and explosion
  target DTOs, source-specific center/power, target planning, entity impact
  application, due-fuse claim, chained TNT spawn, knockback, and dispatch
  planning. Player ignition, registry
  fields, generic cleanup, world explosion mutation, durability, drops, and
  actual delivery remain in their current owners. Existing session/entity
  lock order, authority fences, stable ordering, and reserve-before-deliver
  publication are unchanged.
- `play::random_ticks` owns deterministic section filtering/sampling, seed and
  rule planning, leaf decay/drops/distance, fire, grass, and farmland rules.
  Policy/report DTOs, candidate grouping, snapshot fanout, world commit,
  durability, relight, drop spawning, and publication remain in `play.rs`.
  The boundary rejects writer, session, lock, async, sender, packet, and
  publication dependencies.
- `play::session::hostile_authority` owns hostile attack planning, target
  refresh, bed-rest exclusion, melee/skeleton tick authority, creeper
  prime/cancel authority, goal diffing, and test probes. A creeper uses the
  retained explosive fuse but never the generic melee path. Registry/probe
  fields, generic lifecycle/indexes,
  projectile authority, simulation scheduling, and actual delivery remain in
  their current owners. Existing lock order, save barriers, indexed scan,
  target fences, and release-before-publication behavior are unchanged;
  test-only probes block on exact channel events.
- `play::scheduled_blocks` owns synchronous scheduled-block planning rules,
  comparator signals, hopper transfer planning/execution, furnace/campfire
  insertion, hopper geometry, placement ticks, and backfill scheduling.
  `play.rs` retains due-tick ownership, regional routing, lock acquisition,
  commit, durability, relight, invalidation, broadcasts, and publication. A
  narrow parent wrapper performs `ServerConfig` normalization; the child has
  no async, packet, direct-send, or lock backedge.
- `play::session::herd_spawn_authority` owns herd claims/outcomes, grouped
  admission, pending-hostile activation, owner commit/rollback, candidate
  construction, distance/cap rules, and committed publication installation.
  Registry/probe fields, lock helpers, generic indexes, simulation retry,
  sleep/world-time orchestration, and actual delivery remain in their current
  owners. Existing lock order, journal outcomes, exact retry, UUID dedupe, and
  stable batch publication are unchanged.
- `play::lighting` owns incremental source capture and currentness, light
  computation, full-fallback collection, outbound light value construction,
  cache seeding, and baked-light persistence. Async world-lock orchestration,
  mutation commit, prepared-cache invalidation, recipient selection, command
  publication, and packet writes remain in their current owners. The child
  acquires no lock, sends no command, and preserves final-state encoding and
  conditional persistence. Regional publication tries one optimistic
  source-identity commit. If that snapshot is stale, it performs one final
  recompute and publication under sorted affected-region write locks; it never
  retries by polling. This expensive lock-held path is limited to the
  publication-race fallback, while the normal path computes outside authority
  locks.
- `play::session::chunk_view_authority` owns block-entity dispatch planning,
  view replacement, loaded/unloaded revision fences, recipient planning, and
  sorted ticket/load snapshots. It operates on guards acquired by the parent;
  registry fields, teardown, shared index helpers, prepared-cache ownership,
  persistence, and actual delivery remain in their current owners. The literal
  session-to-prepared-cache lock order, subscriber cleanup, visibility refresh
  order, and ordered recipient construction are unchanged.
- `play::block_placement` owns synchronous placement planning, exact snapshot
  position construction, cactus support/cascade/obstruction rules, sign state
  and NBT rules, direction and sign rotation, and door-half helpers. Loaded
  snapshot acquisition, packet handling, simulation-owner commit, fencing,
  relight, sign-editor publication, and delivery remain in `play.rs`. The
  child has no async, writer, session, lock, or packet dependency.
- `play::session::survival_action_authority` owns direct survival break,
  placement, and bucket commits together with their regional transaction
  preparation. Simulation routing, regional transaction commit, TNT, food,
  publication, and delivery remain in their existing owners. Conditional
  world mutation still precedes inventory mutation, and the established
  world -> session -> entity/player-persistence lock order is unchanged.
- `play::movement` owns accepted absolute-movement validation and coordinate
  normalization, snapshot-based water/collision/campfire contact rules,
  pending-teleport state transitions, fall state/damage, farmland landing,
  and movement exhaustion. `play.rs` retains snapshot acquisition, async
  collision correction and teleport packet writes, simulation commit, and
  publication. The child acquires no world/session lock and performs no async
  work or delivery.
- `play::session::entity_lifecycle` owns generic falling-block and command
  entity spawn mutation, dying-entity completion, nearby indexed candidate
  queries, conditional removal cleanup, and entity chunk-index maintenance.
  `session.rs` retains registry fields, EntityStore -> SessionRegistry guard
  acquisition, public wrappers, specialized entity domains, and delivery.
  Consumers import lifecycle helpers directly rather than through the parent
  facade. Lethal mutation owners enqueue exact removal deadlines, and restore
  rebuilds that index from retained snapshots. The tick path drains at most
  four due entities, preserving exact timing for ordinary deaths while bounding
  overload work; later due entries remain queued without scanning live
  entities. Mutation and visibility plans retain stable entity-ID order.
- `play::containers::crafting` owns synchronous 2x2 and 3x3 menu mutation,
  pickup, swap, throw, quick-move, result taking, ingredient consumption, and
  container remainders. Shared slot and cursor primitives live beside
  `PlayerInventory` with explicit dependencies. `play.rs` retains packet and
  stale-state fencing, window ownership, async owner commit/rollback,
  persistence, drop publication, state IDs, delivery, and disconnect recovery.
  Quick-move destinations are disjoint: inventory main slots target only the
  hotbar, hotbar slots target only main inventory, and crafting-table grid
  slots return input to player inventory before refreshing the result.
- `play::session::session_lifecycle` owns session admission, active-session
  subscriptions and queries, the exact pushed empty-session wait, unregister,
  and unregister-preserving-player-state orchestration. `session.rs` retains
  registry fields and guard acquisition; persistence, prepared-cache,
  visibility, container cleanup, and delivery stay in their current owners.
  Active-count publication and the last-session empty notification happen
  while the session registry still serializes admission, preventing a
  concurrent register from being overwritten by stale zero-session state.
- `play::combat::player_actions` owns attack recharge/damage/Sharpness,
  held-weapon durability, shield state/identity/flags/front-arc rules, and
  shield durability mutation over explicit stack values and slices. It has no
  `PlayerInventory`, `InteractionState`, session, lock, async, sender, packet
  writer, or survival-module backedge. Local damage and PvP import the same
  identity, direction, and durability functions; the PvP boundary tripwire
  requires those production calls and rejects local formula copies.
- Local shield durability is one simulation-owner CAS with the active-shield
  identity. `PlayerSurvivalPlan` carries an optional expected/updated shield
  transition; `player_state` compares inventory and shield identity, then
  commits both under the existing session/entity/player-persistence turn.
  Rejection returns the exact authoritative inventory, carried item, and shield
  snapshot. The connection reconstructs local shield state and retries once;
  a repeated conflict fails closed as runtime unavailable instead of consuming
  a free blocked hit. Non-shield survival plans do not carry this transition.
- `play::session::player_pose_authority` owns accepted-pose mutation, nearby
  body-candidate capture, ECS body push, current-snapshot filtering, entity
  chunk-index/visibility publication, and accepted-pose completion. Registry
  and entity fields, guard acquisition, persistence, prewarm updates, test
  pause probes, pickup selection, and delivery remain in `session.rs`. Entity
  mutation and session publication stay separate, with the current-snapshot
  fence and body-push -> player-movement -> pickup order preserved.
- `play::beds` owns published-snapshot bed validation, canonical head/foot
  geometry, occupancy edit and mutation-token planning, obstruction and monster
  rules, respawn/wake poses, the ordered 12-candidate wake search, and morning
  arithmetic. `play.rs` retains hostile lookup, async world/session commits,
  packet writes, and publication. Both validated halves remain preconditions
  even when only one changes; foot and head interactions use the same canonical
  head; ordinary wake candidates stand on floor support while the final two
  candidates stand above the head or foot.
- `play::session::player_item_action_authority` owns the four concrete food,
  bow, selected-drop, and TNT commits. It preserves their deliberately distinct
  lock scopes: food session -> persistence, bow/drop entity -> session ->
  persistence, and TNT world -> entity -> session -> persistence. The module
  adds no async work, generic lock helper, direct delivery, or packet write;
  requester-loss and stale-state behavior remain simulation-owner concerns.
- `play::falling_blocks` owns falling start/landing DTOs, sorted unique snapshot
  chunk sets, state and landing-cell classification, start scans and token
  preconditions, sequential landing projection, and blocked landing drops. The
  module consumes explicit registries/facts/snapshots only. `play.rs` retains
  async orchestration, world locking and mutation, entity commits, relight,
  drop publication, packet translation, and delivery.
- `play::session::player_state` also owns persistence registration/recovery,
  active-shield publication, container inventory/drop commit, save snapshots,
  generation acknowledgement, and the exact pushed save-request wait. Registry
  fields and lock helpers remain in `session.rs`. Inventory/drop materialization
  retains the entity -> session -> persistence turn; disconnected saves retain
  their generation ABA fence; save waiting arms `Notify` before rechecking the
  generation and never polls or treats elapsed time as success.
- `play::command_execution` is an explicit protocol adapter. It owns parsed
  player-command execution, feedback/time/respawn packets, give/debug/survival
  application, game-mode changes, and client-command responses. Parsing, trees
  and suggestions remain in `commands`; socket dispatch remains in `play.rs`;
  mutations still cross existing simulation/session/world owner APIs. This
  adapter may use `InteractionState` and packet writes by role, but may not take
  world/session locks, use raw channels, sleep, poll, or add hidden retries.
  Both game-mode entry paths share one transition preflight that clears active
  shield authority only for an actual permitted transition.
- `play::session::player_pose_adapter` owns session/persistence lock ordering,
  prewarm updates, entity body-push orchestration, current-snapshot fencing and
  accepted-pose/pickup publication. The lock-free mutation and publication
  helpers remain in `player_pose_authority`; registry fields and generic lock
  helpers remain in `session.rs`. The adapter adds no async work, direct send,
  packet write, or lock class.
- `play::block_edit_commit` is the concrete storage/simulation/publication
  adapter for block edits. It owns conditional storage commits, scheduled-tick
  admission, opaque block-entity fencing, visible finalization, authoritative
  resync and player acknowledgements. Gameplay planning stays in its existing
  domains, production mutation still enters through the simulation owner, and
  storage remains owned by `mc-world`. The test-only direct world path remains
  explicitly fenced. Sibling authorities import this module directly; the
  light-change predicate stays in `play.rs` to avoid a dependency cycle with
  `lighting`. Its tripwire permits the existing world/packet adapter duties but
  rejects raw channels, spawned work, sleeps, polling and wildcard backedges;
  it also pins the production owner branch, peer-broadcast setting and
  resync-before-single-ack order.
- `play::script_gameplay_events` is the protocol-to-plugin DTO projection for
  committed gameplay facts. Block-break authority remains in `block_break`;
  placement planning and validation remain in `use_item_on_adapter`; mutation
  remains in the simulation/world owner. Crafting mutation remains in the 2x2
  inventory and 3x3 crafting-table container rules, while `play::recipes`
  returns the aggregate result of recipe-book crafting. Those paths alone
  decide whether the block transition or inventory candidate succeeded. They
  pass the prior destroyed state, actual applied placement root state,
  committed crafted-item fact, or exact credited pickup to the publisher only
  after owner commit. Pickup facts
  distinguish world items from grounded arrows and report only the amount
  merged into the player inventory. Synchronous combat commits use
  `play::session::script_commit_events`. Player death converges on
  `session::player_state`; a direct player-melee entity kill converges on
  `session::entity_combat` after both target damage and attacker costs commit.
  These owners snapshot exact player, mode, dimension, position, and death or
  target facts into one nonblocking push outbox before any client write. A
  single async server worker preserves outbox order and awaits required bounded
  Lua admission. PvP and projectile player death therefore do not depend on the victim connection
  consuming `PlayerDamageCommitted`, and a failed health/inventory packet write
  cannot erase an accepted event. The outbox is intentionally unbounded: a
  synchronous owner cannot await bounded capacity while holding state locks,
  and dropping an already committed event would violate the plugin contract.
  Death production is fenced to one entry per live-to-dead transition; direct
  melee-kill production is fenced to the target's one lethal transition.
  Another player-death entry requires an authoritative respawn. The bounded
  Lua queue remains the backpressure boundary. If measured hostile workloads
  make this outbox material, replace it with a reserved-permit or durable
  segmented outbox without moving waits under owner locks. No path invents
  incomplete killer or damage-source attribution. Queue closure cannot roll
  back the committed mutation. Shutdown fences connection and simulation
  producers before closing and draining the outbox; `server.stopping` then uses
  the same required admission path before event admission closes.
  FIFO is guaranteed within this committed outbox, not globally against
  concurrent lossy telemetry or player-command producers. A global script-event
  sequencer is a separate API change and is not implied by this adapter.
- `play::session::player_state_adapter` owns selected-slot, respawn-pose and
  game-mode event commits plus player animation/entity-data recipient
  projection. Persistence/inventory/survival authority remains in
  `player_state`; sleep policy remains in `sleep`; visibility selection remains
  in `visibility`; delivery remains in `outbound`. The adapter preserves the
  session -> persistence turn, releases locks before returned publication, and
  distinguishes observer-only from including-self delivery with direct
  recipient coverage. It adds no async work, packet write or direct send.
- `play::bucket_interactions` is the concrete bucket and cauldron protocol
  adapter. It owns bucket placement/pickup, cauldron planning, inventory
  replacement, simulation commit, rejection resync and success responses.
  Shared published-block snapshots and fluid scheduling remain in `play.rs`;
  fluid rules remain in `fluids`; block finalization remains in
  `block_edit_commit`; mutation remains simulation/session-owned. Session
  authorities import bucket inventory replacement directly from this module.
  The boundary rejects world/session locks, raw channels, spawned work, sleeps
  and wildcard imports. Its structural fence pins the simulation owner call and
  the existing reject resync/inventory/ack and success block/ack/inventory/
  animation order.
- `play::session::outbound_publication` owns external disconnect/custom-payload,
  system/script chat and debug-pressure dispatch projection. Channel lanes,
  retries and backpressure remain in `outbound`; recipient helpers remain in
  `visibility`; player animation/entity-data stays in `player_state_adapter`.
  It may create and dispatch existing `VisibilityDispatch` values, but it may
  not own raw channels, async work, packet encoding, gameplay authority or
  entity lock helpers. Direct publication releases the registry lock before
  dispatch; direct, all-session and missing-recipient behavior has focused
  coverage.
- `script::zone` owns the bounded plugin-zone registry, per-player membership
  snapshots, monotonic observation fencing and targeted entry/exit event
  creation.
  One adapter is created at server bind and shared with the admitted script
  command router and play connections. The router alone mutates zone
  definitions; an accepted absolute player movement pushes a pose observation;
  session cleanup forgets the player. The registry mutex is released before
  targeted event delivery awaits queue admission. Lua never receives session,
  entity or registry handles. Accepted mixed transitions publish deterministic
  exits before entries; rejected movement and cleanup cannot publish membership
  events.
- `script::zone` also owns the temporary shipped land-claim lookup. Only the
  exact `land-claims` plugin and its documented owner-UUID zone-id convention
  have protection semantics. `play::block_break` and
  `play::use_item_on_adapter` call that lookup immediately before authoritative
  block mutation and resynchronize denied clients; they do not parse plugin
  ids or claim ownership themselves. The registry lock is held only for the
  bounded lookup, which is the linearization point for an admitted player
  mutation; a claim command ordered afterward cannot retroactively cancel that
  in-flight mutation. Zone commands publish a targeted accepted/rejected result
  so durable claim storage can roll back an unapplied registry change. This
  bridge covers direct player break/place only and must be deleted when the
  script zone DTO gains an explicit protection policy.
- The `mc-script` Lua loader owns optional per-plugin `config.toml` discovery,
  bounded parsing, recursive type/shape validation, and the immutable startup
  snapshot exposed by `solaris.config()`. Each call materializes a fresh Lua
  table, so plugin mutation cannot alter host state or another call. Runtime
  handlers perform no configuration I/O. Live reload, environment expansion,
  defaults, and cross-plugin configuration access require a separate contract
  and are not implied by this boundary.
- The `mc-script` Lua host owns bounded in-memory plugin timers and deterministic
  due-callback dispatch. `mc-net` only pushes the authoritative simulation tick
  through a monotonic latest-value admission lane. When the normal script queue
  is full, that lane coalesces to the newest tick and wakes the existing host;
  it does not block simulation, create another task or thread, poll state, or
  wait on wall-clock time. The host suppresses stale queued ticks, drains at
  most eight due callbacks per pushed tick in deadline/id order, and shares the
  input tick's Lua fuel and command batch across callbacks and `on_server_tick`.
  Timers are host-local, non-durable plugin state; durable scheduling requires a
  separate storage/recovery contract.
- `script::colony` owns the bounded, owner-scoped in-memory colony registry and
  correlated colony, binding, and villager-order result publication. Registry
  keys include the host-attached plugin identity, replacements remain possible
  at capacity, and rejected admissions cannot mutate state. The registry mutex
  is released before targeted delivery awaits queue admission. The adapter
  validates colony ownership and the current single-world dimension, generates
  opaque random binding tokens, and retains a bounded token-to-plugin/colony/
  expiry map. A foreign plugin cannot consume or invalidate an owner's token.
  The separate `play::session::script_colony_endpoint` moves synchronous owner
  requests off the async router worker and calls only bounded regional claim and
  goal commands; it never scans session snapshots. Entity selection, duplicate
  exclusion, exact simulation-tick expiry, villager liveness/type validation,
  and journaled goal mutation remain owned by `mc-entity`. The public order
  surface maps only `home` to a server-owned follow-position goal at speed `0.3`
  and `hold` to idle. Deterministic request rejection, owner busy state, claim
  capacity exhaustion, and stale bindings publish an unsuccessful targeted
  result and keep the router alive; busy state retains the unexpired token for a
  retry. A changed non-overworld colony record rejects before owner mutation.
  Broken owner, lease, location, or journal authority still stops routing.
  Forced cancellation can orphan an already committed claim or goal;
  claims remain bounded by the same 600-tick expiry, and a committed goal is not
  rolled back when result publication closes. Cooperative shutdown drains the
  active route.
- `play::containers::script_menu` owns the immutable plugin-menu layout,
  item resolution, fixed-slot click classification and plugin/menu/player
  identity fence. `play::session::script_menu_endpoint` consumes admitted Lua
  commands and routes open/close requests through the target session's ordered
  reliable lane. `play.rs` remains the packet coordinator and holds the active
  window in the connection-owned interaction state. Rejected or stale clicks
  only resync content; accepted clicks publish a typed event to the retained
  plugin owner. The wire harness covers the full Lua-to-client-to-Lua path.
- `script::storage` owns plugin record versions, quota checks, CRC-framed batch
  durability, and transaction command serialization. `play::script_inventory_transaction`
  owns resource resolution and inventory planning;
  `play::session::script_inventory_transaction_endpoint` holds the canonical
  player-state lock across the storage commit and inventory replacement, then
  publishes one reliable authoritative inventory snapshot.
  This deliberately permits a cold-path storage `sync_all` under one player
  lock so concurrent inventory operations cannot interleave the two runtime
  mutations. A per-session lifetime gate orders this owner turn against
  unregister; unregister waits on the gate without retaining the registry lock.
  The transaction path likewise releases the registry lock before I/O. This
  adds no task, polling loop, operator setting, or hot tick work. Plugin WAL and
  vanilla playerdata crash recovery remain separate and must not be described
  as crash-atomic.
- `script::inventory` owns admitted player-inventory routing and targeted result
  publication. `play::script_inventory_transaction` provides the shared pure
  resource planner, while `play::session::script_player_inventory_endpoint`
  resolves the exact connected session, orders the request against disconnect
  with the existing script-transaction lifetime gate, holds the canonical
  player-persistence lock for plan and replacement, and publishes one reliable
  authoritative inventory snapshot. The adapter rejects a worldless runtime
  before session commit. Failed planning and stale lifetime paths publish an
  exact targeted failure and make no partial mutation. This path adds no task,
  polling loop, sleep, storage dependency, or new lock class.
- `play::player_damage_adapter` owns fall/contact/general player damage
  orchestration, publication projection, melee-knockback conversion and its
  concrete request/result DTOs. Damage and shield rules remain in `combat`;
  survival mutation remains behind the existing owner commits; movement and
  socket dispatch remain outside. Shield durability retries the exact owner
  CAS at most once, then fails closed. A rejected health CAS may still accept
  independent inventory/XP deltas, but it cannot publish death, hurt,
  knockback or death cleanup from the stale health result. The boundary adds
  no lock, raw channel, spawned task, sleep or polling path.
- `play::session::interaction_geometry` owns pure player/entity distance,
  AABB, eye-to-bounds and reach calculations. Block interaction uses the
  26.1.2 block-interaction attribute plus the packet buffer and a strict
  boundary. Entity interaction uses its separate attribute and strict packet
  boundary. Melee uses the authoritative main-hand `AttackRange` component and
  its inclusive boundary; callers cannot reuse interaction reach for attacks.
  Player eye height and target bounds follow standing, crouching, and swimming
  poses. Entity facts remain in
  `mc-data`, physics primitives in `mc-physics`, and decisions remain in their
  existing session children. Consumers use direct sibling imports; the module
  has no registry/world access, lock, async work, packet publication or cached
  duplicate facts. `mc-data::item_components` owns parsed and embedded
  `minecraft:attack_range` facts, including all seven 26.1.2 spear items.
- `play::session::script_entity_interaction` owns the authoritative snapshot
  used by `player.entity_interacted`: actor mode/liveness, target
  liveness/type, current pose, and canonical reach are checked in one existing
  session/entity owner turn. Living projection follows the entity behavior
  archetype rather than `MobCategory`, so living `MISC` entities such as
  villagers retain health. `play::script_gameplay_events` owns only the Lua DTO
  mapping and required queue admission. `play.rs` completes the existing
  vanilla interaction and client writes before awaiting that admission, while
  write errors keep their original immediate return. This boundary adds no
  lock class, task, channel, polling loop, sleep, or gameplay mutation.
- `play::campfire_adapter` owns campfire use responses, cooking-tick
  orchestration, persisted hydration, pending-output recovery and block-entity
  packet projection. Campfire rules/NBT remain in `campfire`; session CAS stays
  in `campfire_authority`; entity materialization stays in `simulation`; the
  shared resident journal remains in `play.rs`. Use rejection preserves
  resync -> inventory -> ack, success preserves inventory -> block entity ->
  ack, and full campfires do not debit inventory. Cooking and recovery preserve
  D1 -> entity materialization -> D2 -> publication, and incomplete
  materialization is never acknowledged. The adapter may use the existing
  world lock required by its concrete storage role, but adds no lock class,
  channel, task, sleep, polling or hidden retry.
- `play::use_item_on_adapter` owns use-item-on preflight, interaction priority,
  ordinary placement, hoe, bonemeal, plant and sign protocol projection. Bed,
  toggle, bucket and campfire rules remain in their existing modules; world and
  inventory mutation remain behind simulation ownership. Placement plans carry
  the expected game mode, while both direct and regional owners compare it with
  authoritative player state and decide debit themselves. A concurrent mode
  change, Adventure/Spectator mode, stale block token or held-stack mismatch
  fails without world or inventory mutation. Creative ordinary placement and
  bonemeal preserve inventory through the real wire path. The adapter adds no
  lock, channel, task, sleep, polling or retry.
- `play::session::entity_lifecycle` also owns the registry adapters for
  falling-block spawn, command-entity spawn and dying-entity completion. Each
  wrapper preserves one existing session/entity lock turn. Spawn still applies
  authoritative facts before AABB/index/wire/visibility installation; death
  completion remains entity-ID sorted with event 60 before removal. The
  boundary rejects world/protocol backedges, raw channels, spawned work,
  sleeps and polling.

The primed-explosive owner uses the same per-session ordered publication path without
holding the world or session lock during channel delivery. Claiming an expired
TNT removes its authority and visibility state but does not reserve its terminal
publication early. After world mutation completes and the world guard is
released, the owner reserves and dispatches each stable entity-ID transaction
in final order: block deltas, light updates, item-drop spawns, terminal despawn,
damage/explosion, then chained TNT spawns. A required earlier entity spawn
therefore blocks the whole TNT transaction, and one TNT transaction cannot be
overtaken by the next. Generic block-edit recipients remain unordered; the
ordered loaded-chunk recipient path is intentionally limited to this TNT
transaction boundary.

Creepers use the local decompiled 26.1.2 `Creeper`/`SwellGoal` common-path
values: a 30-tick fuse, exclusive 3-block trigger, 7-block cancellation
distance, and explosion power 3. Moving out of range reverses fuse progress one
tick at a time instead of resetting it. While swelling or reversing, the
creeper's navigation goal stays idle. Natural swell progress is omitted from
the save projection, matching vanilla restart behavior. Their
wire explosion, removal, and player damage use the same ordered publication
path as TNT. Solaris does not yet publish the client-side swell/ignited
metadata; the accessor order is visible in decompiled source, but exact wire
indexes still require a local wire/javap oracle before being added. Line-of-
sight cancellation is also not yet connected to the world-read path.
- `play::session::passive_mobs` owns breeding and grazing plans, while its
  explicit-import `authority` child owns feed, shear, breeding, and grazing
  commits. It adds no lock and keeps mutation in the session/entity authority.
- `play::session::entity_simulation::persistence_projection` owns pure entity
  save/restore timing projection. Entity NBT keeps vanilla `Motion` in
  blocks/tick; the persistence boundary converts to and from Solaris's internal
  blocks/second velocity exactly once. `solaris/entities.dat` must carry
  `SolarisEntityFormatVersion = 2`. Missing, duplicate, malformed, or unknown
  versions fail closed instead of guessing units or preserving an unreleased
  Solaris persistence schema.
- `connection_driver` owns the concrete Handshake -> Status or
  Login/Transfer -> Configuration -> Play socket sequence. `server.rs` retains
  listener supervision and service construction. This is orchestration only;
  it does not create a service trait layer or move gameplay authority.
- `play::simulation::queue` owns bounded command admission, queue envelopes and
  metrics, herd enqueue coalescing, push-driven owner wakeup, ready-batch
  draining, shutdown rejection, and channel construction. `simulation.rs`
  retains command/response types, handle/owner storage, regional authority,
  gameplay processing, and publication. The split preserves permit ordering,
  the background herd cap, exact depth/dequeue accounting, and distinct
  explicit-shutdown versus owner-drop errors; it adds no task, lock, config, or
  passive wait.
- `play::simulation::regional_mutation` owns preparation and execution of the
  existing regional block/container mutation lane and its test probe. It uses
  explicit imports from the parent coordinator and preserves sorted lane
  admission, leases, mutation-token checks, WAL decision order, atomic failure,
  response order, and post-commit publication. Command classification,
  batching, world access, lighting/publication helpers, and `SimulationOwner`
  remain in `simulation.rs`; this extraction does not claim the regional or ECS
  migration complete and adds no task, lock, config, sleep, or polling.
- `script::teleport` owns admission routing and targeted Lua result publication.
  `play::session::script_teleport_endpoint` resolves the exact reliable session
  and carries the correlation token. That token moves into
  `SimulationCommand::CommitPlayerPose`, so the simulation owner publishes the
  mutation outcome before waking the connection waiter; cancellation cannot
  report failure after an authoritative commit. `play::player_teleport` is an
  explicit connection coordinator rather than a pure domain module: it updates
  `InteractionState`, pending teleport confirmation, socket publication, chunk
  replanning, and zone observation after the owner commit. It contains no
  authoritative player mutation and makes no client-confirmation claim.

The existing `play::block_placement` boundary also selects stair facing and
stair/slab top or bottom state from the player yaw and target-relative hit Y.
The rule is pinned to the local 26.1.2 `StairBlock`/`SlabBlock` oracle. Slab
merging and stair neighbour-shape selection are implemented, including
neighbour recomputation and stale selector-dependency rejection. An
ordinary torch placed against a horizontal full-cube support selects the exact
wall-torch facing, while `UP` retains the standing state and `DOWN` is rejected.
The support predicate is deliberately conservative; irregular sturdy faces,
redstone/soul/copper torches, neighbour break cascades, and complete
`isFaceSturdy` parity remain outside this slice.

Entity physics prefetches its versioned owner input before reacquiring the
session registry lock. After commit, publication still checks the current
published session state before writing the mirror. This shortens the second
session-lock critical section without removing the stale-publication fence or
adding a lock.

`xtask code-health` protects each accepted owner with table-driven rules. A
domain may use multiple rules when more than one stable definition anchor is
needed. The module file and exact parent declaration must exist, every guarded
anchor must remain in the owner file, and none may reappear in `play.rs`,
`session.rs`, or `server.rs`. This is a migration tripwire, not proof of
gameplay correctness or complete isolation. Its former synthetic unit tests
that compared arrays of Rust source lines were removed; behavioral claims must
come from owner, integration or wire tests.

Later combat slices will:

1. Return ordered combat events instead of `VisibilityDispatch` values.
2. Move the remaining armor inventory calculations behind the combat contract.
3. Translate events in a combat publication adapter.
4. Leave only packet decoding and wire publication in the Play driver.

## Migration Order

After combat, migrate shared containers, world actions, entity publication,
chunk streaming, and persistence snapshots. `play.rs` and `session.rs` are
coordinators during this migration, not permanent homes for domain state. Each
slice must remove a real
backedge and keep existing behavior under focused tests. Merely moving code to
a child file while retaining `use super::*` does not count as migration.

Legacy modules may remain coupled while they are being replaced. New isolated
modules are protected by `xtask code-health`; widening their dependencies
requires updating this ADR in the same slice. The current line counts remain
large, so these accepted slices do not claim the `play.rs` or `session.rs`
migration complete.

## Validation

Every extraction keeps its existing unit and wire regressions. Combat requires
hurt-resistance, shield/armor, PvP commit/cost, reciprocal attacks,
hostile/projectile, death, and raw-wire PvP coverage. Architecture checks prove that isolated domain
files do not regain Play/session backedges. Performance claims still require
focused measurements; a module split alone is not a speedup.
