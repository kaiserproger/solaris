# Solaris Durable Memory Index

This file is the short continuity index for long agent runs. It routes work to
the canonical document that owns the detail. It is not a validation ledger and
does not turn local artifacts into release evidence.

## Current Checkpoint

Date: 2026-07-19. Branch: `dev/M100-client-agent`. The worktree is intentionally
dirty; inspect exact file ownership before editing and never discard unrelated
changes.

- Latest bounded two-CPU P44 artifact:
  `.analysis/real-client-runs/20260718T111008Z-m94-regression-pack-4X1iF7`.
  Cow and sheep passed; chicken moved at normal speed/distance but failed the
  smooth-yaw check with a 79.2-degree minimum delta. The run had 24 slow ticks
  instead of the previous 124; slow-tick averages were about 69.7 ms total,
  7.18 ms goals, 1.87 ms physics, and 11.26 ms dispatch, max tick 102.7 ms.
  This is improved diagnostic evidence, not gameplay or performance green.
  Keep 60 seconds as the failure-only client startup bound on the two-CPU path.
- Regional goal preparation can reuse one owner-scoped, versioned active
  snapshot batch. The runtime accepts it only for the same authority/version;
  any intervening mutation falls back to a fresh prepare read. Local goal apply
  remains lane-CAS fenced, and remote follow targets still receive exact source
  validation.
- `minecraft_navigate_to_block` is registered through the client MCP/bridge.
  Its event-driven decision loop retains start/target, uses observed raised
  forward clearance for a one-block step, rejects escape from a bounded route
  corridor, and clears keys on every terminal path. Fresh bridge/java tests and
  independent re-review pass. A real-client navigation scenario is still
  required before claiming the terrain behavior proved in Minecraft itself.
- The stonecutter packet/menu skeleton is present with an oracle-derived 26.1.2
  `ClientboundUpdateRecipes` codec. The review fix now batches maximum bounded
  shift-click crafting in one owner transaction, sends recipe data once during
  initial play sync, shares one air/max-stack offer predicate, and adds real
  handler/owner conservation coverage through close/disconnect/rejoin. Focused
  tests and independent re-review pass. A real-client gate remains before
  calling the stonecutter client-proved.
- Detached chunk-herd commands are grouped without raising the two-command
  per-tick background limit, and same-chunk producers share a push-driven
  enqueue result. Independent review approved those parts but rejected the
  first direct-night/pending implementation: time change could overtake pending
  insertion, and a known-safe journal failure consumed retryable herd claims.
  Player time changes now use owner order and definitely-uncommitted outcomes
  restore exact claims. Integration review then found the console time setter
  still bypassed the owner and time-transition paths discarded retry metadata;
  server-owned changes now use the same queue and propagate SAFE retry chunks.
  A final pushed-path fix prevents a later `TimeSet` overtaking earlier deferred
  herd work while retaining the two-command background cap. Integrated tests
  pass, and the final static re-review approved the production pushed path.
- The current mc-net modularization wave is staged, not complete: `play.rs` is
  21,343 lines and `play/session.rs` 5,954. `play::campfire` now owns 545 lines
  of cooking state, recipes, block identity, and compatible NBT; runtime tick,
  recovery, storage, packets, and D1/E/D2 ordering remain in `play.rs`.
  `play::session::pickups` owns 1,016 lines of candidate planning, item/arrow/XP
  claims and credit, materialization, and pickup dispatch construction. It
  preserves entity-store -> session -> player-persistence lock order and has no
  hidden dependency on the coordinator's wildcard imports. Registry maps,
  generic cleanup, and selected-item authority remain in `session.rs`.
  `play::containers::crafting` now owns 296 lines of crafting window state,
  slot maps, recipe/repair results, inventory projections, and wire-item
  construction. `play::session::visibility` owns 792 lines of visibility
  mirrors, recipient planning, entity/player snapshot publication, and wire
  movement state. Per-session atomic reservations plus a bounded delivery FIFO
  preserve plan order after the gameplay lock is released; a deterministic
  regression proves a movement call made before delayed spawn delivery still
  reaches the channel as spawn then movement. Dropped reservations cancel
  explicitly instead of leaving a permanent queue hole. Expired TNT reserves
  block/light publication, item-drop spawns, terminal despawn, damage/explosion,
  and chained spawns only after world mutation releases its guard. The ordered
  loaded-chunk path is limited to TNT, so generic block-edit transactions retain
  their established publication behavior.
  Enforced owners also cover combat, player-state CAS, outbound DTOs, furnace
  and chest rules, passive-mob plans/authority, entity persistence projection,
  server-entity combat, and the connection driver. `xtask code-health` checks
  owner files, exact parent declarations, stable anchors, and explicit module
  fences.
  Combat commands carry no caller pose or mode; owner state is rechecked at
  commit, victim publication uses expected-to-updated deltas, and player or
  server-entity attacker costs commit under the same authority fence as damage.
  Entity physics also moved the versioned prefetch before the second session
  lock while retaining the post-commit current-publication fence. Fresh full
  `mc-net --lib` is 1139 passed/1 ignored, including corrected event-driven PvP
  publication and dirty-high-water fixtures. Full `mc-entity --lib` is 190
  passed/7 ignored; focused pathing is 17/17 and the real terrain detour
  regression passes three repeated runs. The old public `PathingProbe` method
  and two-field `PathingBudget` source contract are preserved; entity-aware
  probing is a defaulted extension. Fresh status/login/configuration/play
  integration is 37/37. For the current split, crafting, visibility, pickup,
  ordered spawn/movement, and ordered TNT regressions pass; full `mc-net` is
  1,144 passed/1 ignored. Strict all-target `mc-net` Clippy passes, `xtask` is
  23/23, and code-health reports `0 fail / KEEP`. The previous broader
  integration evidence remains unchanged.
  Wave 3 further reduces `play.rs` to 21,042 lines and `session.rs` to 5,593.
  The 370-line `containers/enchanting.rs` owns table state, offers, pure
  bookshelf/slot rules, projections, and wire items; concrete world-cache and
  player-slot wrappers remain in `play.rs`. The 398-line
  `session/projectiles.rs` owns arrow spawn/expiry/hits/knockback and segment
  geometry while receiving already-held entity/session guards and never
  acquiring a lock or sending directly. Enchanting is 7/7, arrow is 31/31,
  projectile is 7/7, full `mc-net` remains 1,144 passed/1 ignored, strict
  all-target `mc-net` Clippy passes, and Wave 3 ownership/code-health fences are
  green.
  Wave 4 reduces `play.rs` to 20,821 lines and `session.rs` to 5,269. The
  533-line `containers/stonecutter.rs` owns window state, offer selection,
  projections, wire items, and pure click/quick-move rules while packet/stale
  fences and simulation commits remain in `play.rs`. The 348-line
  `session/container_state.rs` owns registry shards/guards, player-container
  reads, recipient snapshots, and test probes while registry fields,
  unregister ordering, inventory/drop authority, and actual dispatch remain in
  `session.rs`. Stonecutter is 15/15 after removing a redundant source-string
  architecture test; container is 12/12, chest 21/21, furnace 26/26, full
  `mc-net` is 1,144 passed/1 ignored, strict all-target Clippy passes, both
  independent reviews approve the explicit boundaries, and Wave 4
  ownership/code-health fences are green. Startup
  campfire recovery now requires `minecraft:item` only when a pending output
  actually exists. No full workspace, performance, soak, or real-client gate
  has run for this refactor; the campfire wire harness was also not rerun.
  Wave 5 reduces `play.rs` to 20,460 lines and `session.rs` to 4,955. The
  398-line `play/fluids.rs` owns deterministic fluid planning, flow/contact
  rules, nearby rescheduling, delays and state construction while due-tick
  ownership, world commit, journal, relight and publication remain in
  `play.rs`. The 333-line `session/campfire_authority.rs` owns cooking-registry
  operations, conditional tick/ack/cooldown, recovery probes and regional
  transaction preparation while fields, transaction commit and D1/entity/D2
  coordination remain in their existing owners. Fluid is 13/13 plus three
  focused flow/contact regressions; campfire is 29/29; full `mc-net` is 1,144
  passed/1 ignored; strict all-target Clippy, xtask 23/23, fmt, code-health and
  two independent reviews pass. No full workspace, performance, soak, wire
  harness, or real-client gate ran for Wave 5.
  Wave 6 reduces `play.rs` to 20,162 lines and `session.rs` to 4,576. The
  311-line `play/toggles.rs` owns pure door/trapdoor/fence-gate/button/lever
  planning and power propagation while snapshot acquisition, commit,
  durability and publication stay in `play.rs`. The 412-line
  `session/explosion_authority.rs` owns TNT/explosion targets, fuse claim,
  entity impacts, chained spawn, knockback and dispatch planning while player
  ignition, generic cleanup, world mutation and actual delivery stay in their
  existing owners. Toggle is 5/5 plus button 4/4; explosion is 19/19 and TNT
  5/5; full `mc-net` is 1,145 passed/1 ignored; strict all-target Clippy,
  xtask 23/23, fmt, code-health and two independent reviews pass. No full
  workspace, performance, soak, wire harness, or real-client gate ran for
  Wave 6.
  Wave 7 reduces `play.rs` to 19,706 lines and `session.rs` to 4,072. The
  464-line `play/random_ticks.rs` owns deterministic sampling/seeds and
  leaf/fire/grass/farmland rules while policy/orchestration, commit,
  durability, drops and publication stay in `play.rs`. The 548-line
  `session/hostile_authority.rs` owns target refresh, bed exclusion,
  melee/skeleton attack authority, goal diffing and push probes while registry
  fields, lifecycle, projectile authority, scheduling and actual delivery stay
  in their existing owners. Random tick is 34/34; hostile is 23/23, skeleton
  2/2 and bed-rest 1/1; full `mc-net` is 1,145 passed/1 ignored; strict
  all-target Clippy, xtask 23/23, fmt, code-health and two independent reviews
  pass. A workspace all-target check also passed during review, but full
  workspace tests, performance, soak, wire harness, and real-client gates did
  not run for Wave 7.
  Wave 8 reduces `play.rs` to 18,625 lines and `session.rs` to 3,652. The
  1,128-line `play/scheduled_blocks.rs` owns synchronous scheduled-block,
  comparator and hopper rules while due-tick ownership, regional routing,
  world locks, commits, durability, relight and publication stay in `play.rs`.
  The 454-line `session/herd_spawn_authority.rs` owns herd admission/claims,
  pending-hostile activation, commit/rollback, candidate rules and publication
  installation while fields, lock helpers, retry/sleep orchestration and
  actual delivery stay in their existing owners. Scheduled button is 8/8,
  hopper 19/19, comparator 2/2, herd 24/24 and pending-hostile 6/6; full
  `mc-net` is 1,145 passed/1 ignored; strict all-target Clippy, xtask 23/23,
  fmt, code-health and two independent reviews pass. A workspace all-target
  check also passed during review, but full workspace tests, performance,
  soak, wire harness, and real-client gates did not run for Wave 8.
  Wave 9 reduces `play.rs` to 18,178 lines and `session.rs` to 3,363. The
  331-line `play/lighting.rs` owns incremental source capture/currentness,
  light computation, fallback collection, outbound light values, cache seeding
  and baked-light persistence while async lock orchestration, commit,
  invalidation, recipient selection and delivery stay in their existing
  owners. The 317-line `session/chunk_view_authority.rs` owns view replacement,
  loaded/unloaded revision fences, block-entity dispatch plans, recipient
  planning and sorted snapshots while fields, guard acquisition, teardown,
  prepared-cache ownership and delivery stay in `session.rs`. Review removed
  the remaining parent relight implementation and replaced an unbounded stale
  snapshot retry loop with one optimistic publish followed by one final
  recompute under sorted regional locks. Relight is 4/4,
  final-state encoding 1/1, prepared-chunk 5/5, mark-loaded 3/3, replace-view
  1/1 and loaded-recipient 1/1; full `mc-world` is 204/204 and full `mc-net`
  is 1,145 passed/1 ignored; strict all-target Clippy, xtask 24/24, fmt and
  code-health pass. Full
  workspace tests, performance, soak, wire harness and real-client gates did
  not run for Wave 9.
  Wave 10 reduces `play.rs` to 17,844 lines and `session.rs` to 3,104. The
  459-line `play/block_placement.rs` owns synchronous placement planning,
  exact snapshot sets, cactus/sign/door rules and placement helpers while
  snapshot acquisition, simulation commit, fencing, relight and publication
  stay in `play.rs`. The 281-line
  `session/survival_action_authority.rs` owns direct break/place/bucket commits
  and regional transaction preparation while routing, regional commit, TNT,
  food and delivery stay in their existing owners. Review added a direct
  snapshot-set regression for every placement category and multiple ownership
  anchors for both modules. Placement transaction is 30/30, cactus 8/8, sign
  1/1, door 1/1, and the block-placement module has three focused tests; full
  `mc-net` is 1,146 passed/1 ignored. Strict all-target Clippy, xtask 24/24,
  fmt, diff-check, code-health and both independent reviews pass. Full
  workspace tests, performance, soak, wire harness and real-client gates did
  not run for Wave 10.
  Wave 11 reduces `play.rs` to 17,568 lines and `session.rs` to 2,940. The
  326-line `play/movement.rs` owns movement validation/normalization,
  snapshot-based water/collision/campfire rules, pending teleports, fall,
  farmland and exhaustion while snapshot acquisition, packet writes,
  simulation commit and publication stay in `play.rs`. The 216-line
  `session/entity_lifecycle.rs` owns generic spawn/death/removal and entity
  chunk-index mutation while fields, guard acquisition, specialized authority
  and delivery stay outside. Review fixed one passive-mob facade backedge and
  expanded ownership anchors across every movement rule group and the chunk
  index. Movement 1/1, pending teleport 6/6, fall 2/2, collision 2/2,
  farmland 2/2, campfire contact 2/2 and six focused lifecycle/index/order
  regressions pass; full `mc-net` is 1,146 passed/1 ignored. Strict all-target
  Clippy, xtask 24/24, fmt, diff-check, code-health and both re-reviews pass.
  Full workspace tests, performance, soak, wire harness and real-client gates
  did not run for Wave 11.
  Wave 12 reduces `play.rs` to 17,075 lines and `session.rs` to 2,632. The
  938-line `containers/crafting.rs` owns synchronous 2x2/3x3 click mutation,
  quick-move, result consumption and remainders; shared slot/cursor primitives
  are in the 613-line explicit-import `inventory.rs`. The 331-line
  `session/session_lifecycle.rs` owns admission, active-session subscriptions,
  pushed empty-session waiting and teardown orchestration. Review corrected
  main-inventory quick-move overflow, crafting-grid quick-move, a remaining
  wildcard import, and a register/unregister race by publishing active count
  and the empty event inside the serialized registry turn. Direct regressions
  cover both quick-move cases; focused crafting/recovery and session lifecycle
  tests pass. Full `mc-net` is 1,152 passed/1 ignored. Strict all-target
  Clippy, xtask 24/24, fmt, diff-check, code-health and both re-reviews pass.
  Full workspace tests, performance, soak, wire harness and real-client gates
  did not run for Wave 12.
  Wave 13 leaves `play.rs` at 17,121 lines and reduces `session.rs` to 2,510.
  The 427-line explicit-import `combat/player_actions.rs` owns attack cadence,
  held damage/Sharpness, weapon durability, shield identity/front-arc/flags and
  durability over explicit stacks/slices. The 218-line
  `session/player_pose_authority.rs` owns accepted pose, body candidates/ECS
  push, stale filtering and visibility/index publication. Review found and
  fixed duplicated PvP shield formulas, a free-block CAS race, stale local
  shield reconstruction, and a durability-to-PvP owner-turn window.
  `PlayerSurvivalPlan` now commits optional shield identity and inventory
  atomically, returns exact authoritative state on rejection, retries once and
  fails closed on a repeated conflict. Pose review added exact dispatch-order
  and removal/same-ID replacement regressions. The Play coordinator grew by 46
  lines versus Wave 12 because this correctness fence is still staged there;
  no size reduction is claimed for that file. Shield 21/21, focused pose/body
  tests and both owner-batch races pass; full `mc-net` is 1,162 passed/1
  ignored. Strict all-target Clippy, xtask 25/25, fmt, diff-check, code-health
  and final re-review pass. Full workspace tests, performance, soak, wire
  harness and real-client gates did not run for Wave 13.
  Wave 14 reduces `play.rs` to 16,881 lines and `session.rs` to 2,231. The
  387-line explicit-import `beds.rs` owns canonical bed geometry, occupancy
  token plans, obstruction/monster rules, respawn and ordered wake planning;
  the 298-line `session/player_item_action_authority.rs` owns food, bow, drop
  and TNT commits with their distinct lock scopes unchanged. Review fixed
  ordinary-floor wake Y, the final above-head/foot candidates, head/foot
  respawn and hostile-query canonicalization, mixed-occupancy ABA coverage,
  and a test-only arrow import. Beds are 14/14, sleep 9/9, food 4/4, bow 5/5,
  drop 6/6 and TNT 2/2. Full `mc-net` is 1,167 passed/1 ignored; strict
  all-target Clippy, xtask 25/25, fmt, diff-check, code-health and both final
  re-reviews pass. Full workspace tests, performance, soak, wire harness and
  real-client gates did not run for Wave 14.
  Wave 15 reduces `play.rs` to 16,681 lines and `session.rs` to 2,026. The
  226-line explicit-import `falling_blocks.rs` owns start/landing DTOs, chunk
  sets, state classification, token-fenced start scans and sequential landing
  projection. The existing `session/player_state.rs` grows to 381 lines and now
  also owns persistence registration/recovery, active shield publication,
  inventory/drop commits, save snapshots/ack and the exact Notify-driven save
  wait. Async world/entity/publication work, registry fields and lock helpers
  remain outside. Falling blocks are 5/5; inventory owner 3/3 and the direct
  save-notification cleanup regression pass. Full `mc-net` is 1,167 passed/1
  ignored; strict all-target Clippy, xtask 25/25, fmt, diff-check, code-health
  and both reviews pass. Full workspace tests, performance, soak, wire harness
  and real-client gates did not run for Wave 15.
  Wave 16 reduces `play.rs` to 16,022 lines and `session.rs` to 1,910. The
  717-line explicit `command_execution.rs` owns command/client-command
  execution and response packets while parsing remains in `commands.rs`, socket
  dispatch in `play.rs`, and mutations in existing owners. The 137-line
  `session/player_pose_adapter.rs` owns pose lock/prewarm/body-push/currentness/
  pickup orchestration while the 218-line authority remains lock-free. Review
  found and fixed a game-mode race: both command and direct packet transitions
  now clear local/session shield authority immediately only for real permitted
  changes; denied/no-op transitions retain it. Commands are 35/35, pose 16/16,
  focused body-push review tests and two transition regressions pass. Full
  `mc-net` is 1,169 passed/1 ignored; strict all-target Clippy, xtask 26/26,
  fmt, diff-check, code-health and final reviews pass. The focused command wire
  harness is 8/8. Full workspace tests, performance, soak, broad wire harness
  and real-client gates did not run for Wave 16.
  Wave 17 reduces `play.rs` to 15,658 lines and `session.rs` to 1,781. The
  394-line explicit `block_edit_commit.rs` owns conditional storage edits,
  scheduled-tick admission, opaque block-entity fencing, visible finalization,
  authoritative resync and one player acknowledgement. Production mutation
  still enters through the simulation owner; the direct world path is
  test-only; sibling authorities import the module directly. The 148-line
  `session/player_state_adapter.rs` owns player state-event lock orchestration
  and animation/entity-data recipient projection while persistence, sleep,
  visibility and delivery owners stay separate. Review strengthened exact
  ownership anchors, production structural fences and direct observer/self
  recipient coverage. Full `mc-net` is 1,170 passed/1 ignored; strict
  all-target Clippy, xtask 27/27, code-health, fmt, diff-check and final reviews
  pass. Full workspace tests, performance, soak, wire harness and real-client
  gates did not run for Wave 17.
  Wave 18 reduces `play.rs` to 15,369 lines and `session.rs` to 1,667. The
  312-line explicit `bucket_interactions.rs` owns bucket/cauldron planning,
  inventory replacement, simulation commit and ordered responses while shared
  snapshot/fluid helpers and existing mutation owners stay separate. The
  128-line `session/outbound_publication.rs` owns external disconnect/custom,
  system/script chat and debug dispatch projection while raw channels,
  backpressure and visibility helpers stay in their owners. Review added a
  scoped bucket owner/response-order fence, raw-channel rejection and direct
  recipient success/missing coverage. Bucket/cauldron focused tests are 9/9;
  outbound projection is 1/1 and retry baselines pass. Full `mc-net` is 1,171
  passed/1 ignored; strict all-target Clippy, xtask 28/28, code-health, fmt,
  diff-check and both final reviews pass. Full workspace tests, performance,
  soak, wire harness and real-client gates did not run for Wave 18. Creative
  cauldron behavior and lava/powder-snow cauldron rules remain preexisting
  parity follow-ups requiring a vanilla oracle; Wave 18 makes no parity claim.
  Wave 19 reduces `play.rs` to 15,099 lines and `session.rs` to 1,564. The
  299-line explicit `player_damage_adapter.rs` owns fall/contact/general player
  damage orchestration, publication projection, knockback conversion and its
  concrete DTOs while combat rules, survival authority, movement and delivery
  stay separate. Review found and fixed a stale-publication race: rejected
  health CAS no longer leaks death cleanup, hurt or knockback, while independent
  inventory/XP deltas keep their own CAS behavior. The 122-line pure
  `session/interaction_geometry.rs` owns distance, AABB, eye/block-center and
  reach math with direct sibling imports and no runtime/publication dependency.
  Damage focused tests are 13/13, geometry focused tests are 4/4, and the stale
  publication regression is 1/1. Full `mc-net` is 1,172 passed/1 ignored;
  strict all-target Clippy, xtask 30/30, code-health, fmt, diff-check and both
  final reviews pass. Full workspace tests, performance, soak, wire harness and
  real-client gates did not run for Wave 19; this refactor makes no new parity
  or performance claim.
  Wave 20 reduces `play.rs` to 14,462 lines and `session.rs` to 1,502. The
  679-line explicit `campfire_adapter.rs` owns use responses, owned cooking
  ticks, hydration, pending-output recovery and block-entity projection while
  campfire rules/NBT, session CAS, simulation materialization and shared
  resident journaling stay with their existing owners. The 279-line existing
  `session/entity_lifecycle.rs` now also owns falling-block/command-entity spawn
  and dying-tick registry adapters with one unchanged lock turn. Review
  expanded the lifecycle tripwire against world/protocol/raw-channel/task
  backedges. A focused wire failure exposed a harness race: restart could
  finish cooking before reconnect, while `drain_until_chunk` discarded the
  authoritative empty campfire sidecar. The test now observes chunk and delta
  packets in one push-driven loop. Campfire unit tests are 29/29,
  falling-block tests 5/5, death completion 1/1 and focused campfire wire tests
  5/5. Full `mc-net` is 1,172 passed/1 ignored; strict all-target Clippy, xtask
  31/31, code-health, fmt, diff-check and both independent reviews pass. Full
  workspace tests, performance, soak, broad wire harness and real-client gates
  did not run for Wave 20; no parity or performance claim is added.
- Worktree cleanup is current: the old outbound worktree was removed and only
  `/home/kaiserroman/solaris` remains registered.
- `EntityStore` is production ECS-only. The former shadow comparison is
  default-off and is enabled only through the `mc-net` dev-dependency; it is not
  a second production authority.
- Wander pathing now retains one absolute target for its deterministic goal
  epoch instead of moving the target with the entity every tick. The configured
  per-entity pathing budget is the hard ceiling for all terrain probes,
  including retained-node validation, step-up, and fallback. Candidate probes
  use the actual one-tick displacement; backward recovery is in the first
  budget group so an AABB already touching a wall can retreat without a
  discrete far-point probe jumping across the obstacle.
- Ore sidecars admit at most 64 rules and 2,000,000 estimated work units per
  generated chunk. Oversized inputs fail validation as a whole; they are never
  silently truncated.
- Each dirty high-water pass flushes at most 64 dirty chunks, then relies on
  pushed tail convergence. Full checkpoints remain interval, disconnect,
  shutdown, or explicit operations. Focused tests pass; P44 has not been rerun.
- Movement fanout takes the adaptive path only when `S * M > 2E` and skips
  entirely when `M = 0`; its three focused tests pass. A runtime performance
  rerun remains pending.
- Client MCP motion/removal waits, virtual-thread transport, and canonical
  interact reach/raycast/world fences pass the focused bridge/java/client-mod gate
  after adding test-only client dependencies. `runClient` has not run.
- P47 stonecutter coverage is `134` focused tests with review approved; the real
  client gate remains pending. Spruce and jungle growth have three passing wire
  tests. The `wide` SIMD experiment remains non-promoted: kernel median gain is
  `7.86%` and full median gain is `0.72%`, both below the 10% promotion gate.
- Detached herd publication now indexes the committed batch first, groups it
  by chunk, and traverses the session table once. Static cost falls from one
  full session scan per entity to `sessions x unique batch chunks + actual
  dispatches`; entity-major packet order and visibility/index semantics remain
  unchanged. Focused tests, strict `mc-net` Clippy, fmt/diff checks, and an
  independent static review pass. The short P44 rerun stayed performance-red;
  this proves the fast path did not remove the dominant per-tick goal/dispatch
  costs, not that the isolated structural reduction regressed runtime.
- Entity physics now performs its initial versioned CAS prefetch before taking
  the session registry lock and uses the published session mirror for the empty
  fast path, reducing counted owner reads from three to two. The authoritative
  post-commit read and current-state publication fence remain deliberately:
  removing either produced a stale-publication race and broke cross-region
  vehicle passenger motion in review. Focused physics/vehicle tests, full
  `mc-net`, strict Clippy, and final static review pass. An ordered publication
  token is required before removing the remaining current-state read.
- Goal apply now projects sorted alive `EntityKinematics` directly from the
  regional owner instead of rebuilding full `EntitySnapshot` values and then
  discarding most fields. The fast path is used only after a coherent goal CAS.
  Stale/no-batch results trigger a rare current full-state read that rechecks
  active membership, AABB, and physics kind; transaction errors still fail.
  Cross-region movement is rejected before lane apply so an expected entity
  cannot disappear from its CAS set. Entities intentionally outside the goal
  CAS, currently grazing sheep, always use the same current full-state path.
  Full snapshot CAS inputs remain the correctness fence and are not removed by
  this slice.
- Work autoscaling attributes `entity_dispatch` to entity pressure. Computed
  percentile windows now push directly from the async worker into the ticker;
  the controller no longer polls a potentially stale snapshot on an arbitrary
  100-tick boundary. Scheduled exhaustion accumulates until a window is
  accepted. Healthy recovery restores half the remaining headroom per decision
  rather than jumping straight to maximum; no worker percentage knob exists.
- Wave 21 moved the complete use-item-on adapter into the 1,446-line
  `play/use_item_on_adapter.rs`; `play.rs` is 13,079 lines, `session.rs` 1,502
  and `simulation.rs` 17,104. Review found two real creative-debit bugs: the
  plan trusted a caller-provided boolean, and the regional transaction always
  consumed an item. Plans now carry expected game mode; direct and regional
  owners share one authoritative mode/debit fence. Mode-change, unsupported
  mode, stale support and inventory mismatch paths fail without mutation.
  Creative ordinary placement and bonemeal inventory preservation pass through
  the real server wire path. Full `mc-net` is 1,176 passed/1 ignored; placement
  wire is 4/4 and bonemeal wire 7/7. The checkpoint workspace run exposed and
  fixed stale harness contracts: complete two-half bed fixtures, isolated TNT
  mob-damage resistance, 20-HP zombie bare-hand hit count, and Survival rather
  than Creative persistence placement. It also restored Gradle runtime
  provenance in validator fixtures and removed the remaining shell/Python
  source-substring test while keeping 38 behavioral manifest/runner tests.
  Full workspace tests, workspace all-target strict Clippy, code-health, fmt,
  diff-check and independent review pass. `xtask` has 0 unit tests and remains
  only a fail-only code-health command. Performance, soak and an actual
  real-client run did not run for Wave 21; ignored oracle/load rows remain
  degraded exactly as reported by the workspace suite.
- Wave 22 moved bounded simulation admission, metrics, herd coalescing,
  batching, wakeup, shutdown and channel construction into the 663-line
  `play/simulation/queue.rs`; `simulation.rs` fell from 17,104 to 16,622 lines.
  Permit ordering, queue accounting, background herd admission and pushed
  wakeups are unchanged, and behavioral tests cover blocked sender closure,
  owner drop, zero budget and shutdown across prefetched/deferred/receiver
  storage. Wave 23 adds local-26.1.2-oracle stair facing/half and slab top/bottom
  placement with target-relative cursor Y; rejection tests preserve no-debit,
  resync and acknowledgement order. Independent reviews approved both slices.
  Full workspace tests, workspace all-target strict Clippy, fmt, code-health and
  diff-check pass; ignored oracle/load/benchmark rows remain explicit. Slab
  merging, stair neighbour shapes, real-client, performance and soak gates are
  not claimed.

## Task Routes

| Task | Read first | Canonical update target |
| --- | --- | --- |
| Playable loop, client-visible gameplay, real-client regression | `docs/playable/README.md`, then `docs/playable/ACTIVE.md` | `docs/playable/ACTIVE.md`; keep raw runs under `.analysis/` |
| Core performance, regional ownership, ECS, autoscale | `docs/milestones/M90.md` through `M93.md`, ADR 0004 and 0005 | Relevant milestone progress log and ADR when the ownership decision changes |
| Minecraft client MCP and reusable client automation | `docs/AGENT_TOOLING.md`, `client-mod/solaris-client-agent/README.md` | The same tooling docs; scenario evidence also goes to playable `ACTIVE.md` |
| Server Lua plugins and API surface | `docs/PLUGINS.md`, ADR 0004/0005 for authority boundaries | `docs/PLUGINS.md`; ADR for new authority or isolation decisions |
| Protocol and packet layout | ADR 0002, local protocol dump, `wire_probe.rs` | Relevant protocol decision/milestone; never commit Mojang bytes |
| Readiness, milestone closeout, replacement claims | `docs/DEFINITION_OF_DONE.md`, relevant milestone, validation ledger | DoD evidence matrix and ledger only when readiness work is explicitly requested |
| Broad unspecialized continuation | `docs/NEXT_SESSION.md` | Update its short lane handoff; do not copy complete run logs |

## Update Rules

- Keep this index short. Replace stale checkpoint bullets instead of appending
  an unbounded history.
- The owner permits documented optimization hacks and narrow fast paths. Record
  their trigger, correctness fence, measured benefit, fallback, and removal
  condition in the relevant ADR or milestone note; do not hide them behind a
  generic helper.
- Put exact gameplay/client observations in `docs/playable/ACTIVE.md` and exact
  architecture decisions in `docs/decisions/`.
- Update an ADR in the same slice that changes authority, threading, waiting,
  persistence ordering, or cross-region semantics. Mark partial migration and
  non-goals; do not imply the final architecture is complete.
- `.analysis/junior-readonly-wal.md` is append-only local evidence. Append a
  concise current-head line after a proved slice; never rewrite its history and
  never treat it as committed memory.
- Do not copy secrets, Mojang/vendor bytes, transient command output, or guessed
  parity into durable docs.
