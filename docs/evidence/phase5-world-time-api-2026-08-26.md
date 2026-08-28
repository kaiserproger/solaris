# Phase 5 typed world-time API — 2026-08-26

## Scope

This checkpoint advances Phase 5 item 3 with one bounded common-world mutation. It does not claim that the full player/world/block/inventory/combat/entity API matrix is complete.

API `0.6.0` adds a typed, capability-gated world-time request/result pair:

- Luau command: `solaris.set_world_time(request_id, world_time)`;
- capability: `world_time`;
- targeted result event: `world.time_set_result` / `on_world_time_set_result`;
- typed Rust request: `ScriptWorldTimeSetRequest`;
- typed public failures: `busy`, `runtime_unavailable`, `rejected`.

The plugin receives no clock registry, session registry, simulation handle, lock, region key or world-storage reference.

## Authority path

The production router does not mutate `SessionRegistry` directly. After host provenance and capability admission it calls:

`SimulationHandle::set_world_time_server_owned(world_time)`

This is the existing simulation-owner clock command lane. The owner processes `SimulationCommand::SetWorldTime`, applies the authoritative session/world clock transition, and publishes the existing visibility updates.

Existing simulation evidence `world_time_handles_enforce_player_and_server_fences_and_owner_ordering` proves the authority boundary:

- an unfenced generic simulation handle cannot use the player-only time-set method;
- a session-scoped handle cannot invoke the server-owned method;
- a server-owned time-set request remains pending until the simulation owner processes it;
- the registry time changes only after owner processing and the exact response succeeds.

The Luau adapter therefore reuses accepted authority rather than adding a plugin-owned clock path.

## Request and result contract

`request_id` uses the normal bounded script-id grammar. `world_time` is bounded to `0..=9_007_199_254_740_991` (`2^53 - 1`) so every accepted value round-trips exactly through the Luau number type; larger values are rejected as invalid bounds instead of being rounded before or after owner commit.

Success publishes exactly one targeted result with:

- the original `request_id`;
- the committed `world_time`;
- `committed = true`;
- `failure = nil`.

Router error categories are deliberately small and stable:

- simulation queue full / queue-admission timeout -> `busy`;
- closed/stopped/response-timeout/shutdown/world-unavailable -> `runtime_unavailable`;
- other owner rejection/mismatch/stale/invalid outcomes -> `rejected`.

The result describes the authoritative owner outcome. It does not claim that a client rendered the new sky state or that every socket write completed.

## Capability and targeting

The disk manifest accepts `capabilities = ["world_time"]`. An undeclared call fails synchronously in the Luau VM with `command capability denied: world_time` before the command batch crosses the host boundary.

Result delivery is targeted by the host-attached plugin identity. Luau does not pass or forge a plugin id when requesting the mutation.

## Focused executable evidence

`mc-script`:

- `world_time_command_is_capability_gated_and_result_is_targeted` proves the typed request shape and targeted result handler;
- `world_time_command_without_capability_fails_before_batch_admission` proves synchronous capability denial;
- `world_time_rejects_values_outside_exact_luau_integer_range` proves the `2^53 - 1` exact-integer ceiling and rejects the first out-of-range value;
- `disk_world_time_capability_emits_host_attested_request` proves disk manifest parsing, host startup, host-attested provenance, request id and exact time value.

Focused command:

```text
cargo test -p mc-script --features lua-runtime world_time -- --nocapture
running 4 tests
...
test result: ok. 4 passed; 0 failed
```

`mc-net`:

- `script::router::world_time_tests::world_time_command_routes_through_owner_and_returns_targeted_result` is the production round-trip: a strict disk plugin emits the host-attested command, `ScriptRouter` calls the existing server-owned simulation lane, the owner tick commits authoritative time `13000`, required targeted delivery reaches only the originating plugin, and its Luau result handler emits the final host-attested acknowledgement;
- `script::router::world_time_tests::world_time_failure_categories_are_stable` proves the public failure mapping;
- existing `play::simulation` owner tests prove the server-owned clock lane and ordering described above.

Focused command:

```text
cargo test -p mc-net --lib script::router::world_time_tests -- --nocapture
running 2 tests
...
test result: ok. 2 passed; 0 failed
```

Benchmark: not applicable. This is an explicit plugin control mutation using the existing simulation command queue; it does not add steady-state tick work.

## Independent review

Exactly one bounded read-only reviewer returned `CHANGES` with two findings. The Medium finding identified that an unconstrained Rust `u64` contract could not round-trip every integer exactly through the Luau number type; the request is now bounded to `2^53 - 1` and a focused boundary regression rejects the next value. The Low finding identified that the public event-field table omitted the already-implemented `committed` field; the table and this evidence now include it. Per repository policy, no second reviewer is run after addressing these finite findings; post-fix self-validation is required instead.

## Disposition

Typed world-time subdomain: **PASS after reviewer fixes and self-validation**.

Phase 5 item 3 remains **OPEN**. Player/inventory already have strong typed transactional coverage; further checkpoints still need to close the missing common block/combat/entity command surfaces and record the final six-domain matrix.
