# Phase 5 lifecycle ordering, rollback, disconnect and restart — 2026-08-27

## Scope

This checkpoint audits Phase 5 item 6:

> Preserve deterministic event ordering, cancellation, rollback, disconnect/reload behavior, and persistence across restart.

The contract is not that every piece of runtime-local Lua state is persisted. Solaris distinguishes durable plugin state from ephemeral runtime state explicitly:

- durable plugin data belongs in plugin storage and survives/replays across restart;
- runtime-local state such as timers is intentionally reset by plugin disable, successful reload, and process restart;
- once an owner/control request has crossed its documented commit-intent boundary, cancelling a caller waiter does not create an ambiguous maybe-commit race.

This evidence checks those declared semantics rather than silently changing them.

## Deterministic event ordering

### Bounded host event FIFO

The ordinary event queue is bounded and preserves already-admitted event order. When capacity is exhausted, a later ordinary event receives `Full`; the existing event is not displaced.

Focused gate:

```text
cargo test -p mc-script --features lua-runtime \
  tests::script_boundary_is_bounded_and_preserves_the_first_event -- --exact --nocapture
running 1 test
...
test result: ok. 1 passed; 0 failed
```

Required/targeted result publication does not jump ahead of queued work. When the queue is saturated, required delivery waits for receiver capacity and then arrives after the buffered event:

```text
cargo test -p mc-script --features lua-runtime \
  tests::required_event_delivery_waits_for_receiver_capacity_notification -- --exact --nocapture
running 1 test
...
test result: ok. 1 passed; 0 failed
```

Closing event admission rejects new events while preserving already-buffered events for drain; required/targeted delivery detects a closed receiver instead of claiming delivery.

### Simulation-tick timer ordering

The timer contract orders due callbacks by scheduled tick then timer id, caps callbacks per pushed tick, preserves later due work, and never fires before the scheduled simulation tick. A stale/lower pushed tick cannot move the timer clock backwards or repeat a callback.

The focused timer suite is 8/8 PASS and includes:

- `timer_callbacks_are_ordered_and_bounded_per_pushed_tick`;
- `stale_tick_does_not_move_timer_clock_backwards_or_repeat_a_callback`;
- `earlier_due_callback_can_cancel_a_later_timer_due_on_the_same_tick`.

### Reload FIFO barrier

Ordinary events and reload controls share one host input FIFO. Events before the reload barrier execute on the old generation; events after a successful barrier execute on the new generation. Coalesced `server.tick` keeps its explicitly documented latest-value/monotonic semantics rather than being falsely reclassified as a strict FIFO record.

`cargo test -p mc-script --features lua-runtime reload -- --nocapture` is 9/9 PASS, including `reload_swaps_generation_at_fifo_boundary`.

## Cancellation semantics

### Explicit timer cancellation

Timers can be replaced or cancelled atomically. An earlier callback may cancel a later timer already due on the same pushed tick. Missing cancellation is an explicit false result rather than a trap or hidden mutation.

The timer suite proves replace/cancel/callback-reschedule atomicity and same-tick cancellation.

### Cancellation after owner/control admission

Reload has a clear commit-intent boundary: once the reload control frame is admitted to the host FIFO, cancelling the caller's response waiter does not cancel the host-owned replacement attempt. `admitted_reload_commits_even_when_response_waiter_is_cancelled` passes in the 9/9 reload suite.

The same design principle is used by result-bearing simulation/session/storage adapters: semantic results describe owner outcomes; dropping a waiter is not treated as permission to roll an already-admitted authoritative mutation backwards.

## Rollback / no-partial-mutation boundaries

### Handler-local staged timer state

Timer schedule/cancel changes are staged with the current Luau handler and commit only when the handler returns successfully. `failed_handler_discards_its_staged_timer_changes` proves a trapped handler does not leak timer changes.

### Reload generation replacement

Reload constructs and initializes the complete candidate generation before swap. Candidate startup commands remain staged until capability/admission/capacity and player-command ownership validation all succeed. Compile/init/budget/backpressure/contract failure leaves the previous generation authoritative.

The 9/9 focused reload suite proves:

- candidate reinitialization before publication;
- initialization failure keeps old generation;
- command-queue pressure keeps old generation;
- syntax/contract failure keeps old generation;
- repaired compatible generation restores command ownership atomically.

Detailed already-reviewed evidence is in [`phase5-luau-safe-reload-boundary-2026-08-26.md`](phase5-luau-safe-reload-boundary-2026-08-26.md).

### Inventory/storage transactions

The script economy transaction boundary rejects invalid inventory/storage state before partial mutation. Current focused result:

```text
cargo test -p mc-net --lib script_inventory_transaction -- --nocapture
running 4 tests
...
test result: ok. 4 passed; 0 failed
```

This includes insufficient/full/unknown resource rejection without a plan, disconnect rejection before storage is touched, and unregister-after-capture rejection before storage commit. The durable storage suite separately proves batch failure/over-quota/stale-state paths do not partially mutate memory or disk.

## Disconnect behavior

A real raw-TCP script lifecycle test proves the normal production sequence:

1. `server.started`;
2. player connects and produces targeted player snapshot;
3. player chat enters the script boundary;
4. simulation tick enters the boundary;
5. script reply reaches the client;
6. the socket is dropped;
7. Solaris publishes exact `player.left(player_id, "disconnected")`.

Focused gate:

```text
cargo test -p mc-server --test play \
  play_script_boundary_carries_lifecycle_chat_tick_and_targeted_reply -- --nocapture
running 1 test
...
test result: ok. 1 passed; 0 failed
```

Session-side disconnect publication is bounded under a slow recipient:

```text
cargo test -p mc-net --lib \
  disconnect_player_retries_are_bounded_per_slow_recipient -- --nocapture
running 1 test
...
test result: ok. 1 passed; 0 failed
```

Plugin-owned zone membership cleanup also has focused proof: `dimension_boundaries_and_player_cleanup_drive_fresh_entries` passes, so stale player membership does not survive cleanup and suppress a later valid entry.

Shipped economy/colony plugins subscribe to `player.left` and explicitly drop per-player runtime caches/pending state; durable business state remains in plugin storage.

## Reload behavior

Focused reload result:

```text
cargo test -p mc-script --features lua-runtime reload -- --nocapture
running 9 tests
...
test result: ok. 9 passed; 0 failed
```

The suite proves FIFO generation swap, candidate-before-publication, rollback on init/backpressure/invalid contract, repair of a faulted plugin, cancellation semantics after admission, and normal host closure. Production SIGHUP reload was already closed with an independent `PASS` in [`phase5-luau-safe-reload-boundary-2026-08-26.md`](phase5-luau-safe-reload-boundary-2026-08-26.md).

Successful reload intentionally creates fresh runtime-local timer state. Durable plugin data is not tied to the Luau VM generation because it resides in the independent plugin-storage actor.

## Persistence across restart

Plugin storage is the public durable-state contract. The 19/19 storage suite covers restart/replay broadly. Two focused restart guarantees were rerun explicitly:

```text
cargo test -p mc-net --lib \
  script::storage_tests::storage_restarts_with_get_cas_and_delete_state \
  -- --exact --nocapture
running 1 test
...
test result: ok. 1 passed; 0 failed
```

and:

```text
cargo test -p mc-net --lib \
  script::storage_tests::committed_result_survives_closed_delivery_and_replays_once_after_restart \
  -- --exact --nocapture
running 1 test
...
test result: ok. 1 passed; 0 failed
```

Therefore a committed durable mutation survives restart, and a committed semantic result whose delivery was interrupted is replayed exactly through the durable result outbox rather than being silently lost or causing the mutation to repeat.

Request identity is also durable: identical retries reuse the original transaction/result, while substituted content under the same plugin/request identity is rejected.

### Explicit ephemeral boundary

`docs/PLUGINS.md` states that timers are in-memory only and disappear on server restart or plugin disable. The reload contract likewise states that runtime-local timer state is intentionally reset on successful generation replacement.

This is deliberate public semantics, not an untracked loss of durable plugin data. A plugin that needs a scheduled intent to survive process restart must persist that intent in plugin storage and reconstruct a timer from the restored value after startup. Item 6 is therefore closed only if this declared durable-vs-ephemeral boundary is considered valid by independent review; this checkpoint does **not** claim transparent persistence of VM locals or timers.

## Lifecycle matrix

| Requirement | Contract | Evidence |
| --- | --- | --- |
| deterministic event ordering | bounded ordinary FIFO; required results wait for capacity; tick coalescing is monotonic/latest-value | boundary tests + timer 8/8 |
| cancellation | explicit timer cancel; same-tick cancel; admitted reload remains commit intent | timer 8/8 + reload 9/9 |
| rollback | trapped handler discards staged timer state; failed reload keeps old generation; transactions reject before partial commit | timer/reload + transaction 4/4 + storage 19/19 |
| disconnect behavior | exact real-wire `player.left`; bounded disconnect retries; zone/player cleanup | mc-server lifecycle test + mc-net disconnect/zone tests |
| reload behavior | atomic compatible generation replacement at FIFO boundary; failure retains old generation | reload 9/9 + prior independent reload evidence |
| restart persistence | durable plugin storage/state/result outbox replay; request identity preserved | storage 19/19 + focused restart tests |
| intentionally ephemeral state | VM locals/timers reset; durable intent must be reconstructed from storage | documented timer/reload contract |

## Current quality gates

The most recent shared Phase-5 tree before this evidence has:

- `cargo test -p mc-script --features lua-runtime --quiet` — 202/202 PASS;
- `cargo test -p mc-net --lib --quiet` — 1973 passed / 5 ignored / 0 failed;
- full shipped `plugin_examples` — 5/5 PASS;
- formatter — PASS;
- code-health — `0 fail / KEEP`;
- strict workspace Clippy — PASS.

Item-6 focused gates on that tree:

- reload — 9/9 PASS;
- timers — 8/8 PASS;
- script inventory/storage rollback — 4/4 PASS;
- real-wire script lifecycle/disconnect — 1/1 PASS;
- slow-recipient disconnect retry bound — 1/1 PASS;
- zone player cleanup — 1/1 PASS;
- boundary FIFO — 1/1 PASS;
- required-event capacity ordering — 1/1 PASS;
- durable storage restart state — 1/1 PASS;
- committed-result restart replay — 1/1 PASS.

Scoped `git diff --check`: PASS.

Benchmark: not applicable. This is lifecycle/atomicity/restart contract validation, not a steady-state performance change.

## Independent review

Exactly one bounded independent read-only reviewer returned **PASS** with no findings. The reviewer accepted the lifecycle matrix and explicitly accepted the documented durable-vs-ephemeral boundary: plugin storage/request/result state survives restart, while VM locals and timers are intentionally runtime-local and must be reconstructed from durable intent when a plugin needs restart-surviving scheduling.

## Disposition

Phase 5 item 6: **CLOSED**. Deterministic ordering, explicit cancellation boundaries, rollback/no-partial-commit behavior, disconnect cleanup, atomic reload behavior, and declared restart persistence semantics are all executable and documented.
