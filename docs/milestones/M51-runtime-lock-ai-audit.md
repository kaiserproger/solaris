# M51.g/h runtime lock/starvation and mob AI audit

Date: 2026-05-28
Branch: `dev/M51-core-parity-tellus-ai-perf`

## Scope audited

- `crates/mc-net/src/play/session.rs`
  - `SessionRegistry` lock coverage and entity visibility/AI/dispatch paths.
  - Outbound queue backpressure paths.
- `crates/mc-net/src/server.rs`
  - Runtime tick loop metrics and world/physics sampling lock boundaries.
- `crates/mc-entity/src/lib.rs`
  - Dense `EntityStore` goal ticking and current mob AI primitives.
- `crates/mc-net/src/lock_metrics.rs`
  - Existing lock wait/hold telemetry.
- Harness tests:
  - `crates/mc-test-harness/tests/load_scenarios.rs`
  - `crates/mc-test-harness/tests/mob_presence.rs`

## Concrete hot locks / starvation risks

1. Session registry lock covers AI target selection, goal mutation, and physics-query planning.
   - File/function: `crates/mc-net/src/play/session.rs`,
     `SessionRegistry::tick_entities_and_collect_physics_queries`.
   - Current work under the mutex:
     - Build `active_chunks` from all sessions.
     - Build player-position facts.
     - `update_hostile_targets_locked` scans hostiles and players.
     - `EntityStore::tick_goals` mutates dense goal/velocity/rotation state.
     - Scan all entity views to produce physics candidates and distance-sort budget.
   - Risk: with many loaded entities, normal player/session operations queue behind one
     large registry hold. Existing `lock_metrics` will show this as
     `session_registry.max_hold_us` / `hold_us`, but the runtime log did not isolate
     queued reliable retries before this audit.
   - Safe next test: a unit or harness liveness test that keeps one task calling
     `tick_entities_and_collect_physics_queries` on a populated registry while another
     task repeatedly calls a cheap registry operation, asserting bounded positive
     progress rather than fixed timing.

2. Entity physics sampling holds the world-storage async mutex while sampling every
   query before worker fan-out.
   - File/function: `crates/mc-net/src/server.rs`, `entity_physics_steps`.
   - Current work under the mutex:
     - For every query, `sample_entity_physics_input` allocates a per-entity
       `HashMap<BlockPos, BlockMaterial>` and calls `WorldStorage::get_cached_block`.
   - Risk: large query batches block unrelated chunk/world operations on the same
     world lock. The expensive physics integration itself is correctly moved to
     blocking workers after samples are copied.
   - Existing mitigation: `ENTITY_PHYSICS_QUERY_BUDGET_PER_TICK` caps query count
     before sampling; world lock wait/hold metrics are logged in the runtime tick
     metrics.
   - Safe next test: a debug-only diagnostic that constructs a high-entity candidate
     set and asserts the physics-query budget is honored before `entity_physics_steps`.

3. Slow clients can accumulate reliable retry tasks behind bounded outbound queues.
   - File/function: `crates/mc-net/src/play/session.rs`,
     `dispatch_visibility_commands` and `retry_reliable_command`.
   - Current behavior:
     - Coalescible movement/light/block updates use `try_send` and drop when full.
     - Reliable spawn/despawn/container/data commands spawn an async retry that waits
       on `tx.send(command).await`.
   - Risk: a permanently slow reader no longer blocks the session-registry mutex, but
     it can retain bounded queue capacity and accumulate retry tasks. This is a
     starvation/backpressure risk rather than a deadlock.
   - Change made in this audit: added `reliable_command_retries_in_flight` pressure
     telemetry and runtime-log fields so this backlog is visible alongside drops.
   - Safe next fix: add a per-session reliable backlog cap or disconnect policy once
     the intended slow-client behavior is decided.

4. Full visibility refresh is quadratic in active sessions and also scans visible
   entities while holding the session-registry mutex.
   - File/functions: `crates/mc-net/src/play/session.rs`, `refresh_visibility_locked`,
     `replace_view`, `try_register`.
   - Risk: mass view changes or reconnect storms can hold the registry while building
     `HashMap`/`HashSet` snapshots and diffing every observer/target pair.
   - Existing mitigation: dispatch commands are returned and sent outside the lock in
     the call paths audited; reliable retries also happen outside the lock.
   - Safe next test: concurrent registration/view-replacement liveness under a
     saturated outbound receiver, asserting registry operations still complete.

5. Mob AI is still phase-collapsed inside the session registry.
   - File/functions:
     - `crates/mc-net/src/play/session.rs`, `update_hostile_targets_locked`.
     - `crates/mc-entity/src/lib.rs`, `EntityStore::tick_goals`.
   - Current AI behavior:
     - Hostiles acquire nearest player position within `FollowRange` and use
       `GoalState::FollowPosition`.
     - If no player is in range, hostiles fall back to a wander goal.
     - Wander/aquatic/follow goals deterministically update velocity/rotation in the
       dense store.
   - Gaps vs M51.h target:
     - Fact collection, decision, and application are not yet split.
     - No melee cadence/reach, panic/flee, hazard avoidance, or forget hysteresis.
     - Decisions are single-threaded and mutate store state under the registry lock.
   - Safe next test: observable hostile reacquire/forget behavior with two players and
     a loaded/unloaded chunk boundary before any broad rewrite.

## Code/test change from this audit

- Added `SessionPressureSnapshot::reliable_command_retries_in_flight`.
- Increment/decrement the in-flight counter around reliable retry tasks.
- Included the field in runtime tick metrics in `server.rs`.
- Extended the existing reliable-retry unit test to assert that a full channel reports
  an in-flight retry and returns to zero after the receiver drains.

## Validation commands

Focused commands used for this slice:

```sh
cargo test -p mc-net --lib -- --nocapture
cargo test -p mc-test-harness --test load_scenarios -- --nocapture
cargo test -p mc-test-harness --test mob_presence -- --nocapture
cargo clippy -p mc-net --all-targets -- -D warnings
```
