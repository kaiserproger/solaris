# Phase 5 typed entity-spawn API — 2026-08-26

## Scope

This checkpoint advances Phase 5 item 3 with one bounded common entity-mutation surface. It does not claim that the complete player/world/block/inventory/combat/entity matrix is finished.

API `0.6.0` now exposes entity spawn as a correlated typed request/result operation:

- Luau command `solaris.spawn_entity(request_id, player_id, entity_type, x, y, z)`;
- manifest `spawn_entities = [...]` as the exact entity-type allow-list;
- targeted result `entity.spawn_result` / `on_entity_spawn_result`;
- the existing `ScriptCommand::SpawnEntity` now carries a bounded `request_id`;
- typed public failures `unknown_entity_type`, `actor_unavailable`, `busy`, `runtime_unavailable`, and `rejected`.

The plugin receives no entity registry handle, regional owner, simulation handle, session handle, entity pointer, lock, epoch, or persistence writer.

## Authority path

The public request stays on the existing authority path:

1. Luau bounds `request_id`, the resource id and coordinates before command construction.
2. Host admission verifies the exact `spawn_entities` allow-list and attaches unforgeable plugin provenance.
3. `ScriptRouter` resolves the requested namespaced type against the active server entity registry. A manifest-allowed type that is absent from that runtime registry fails before simulation admission.
4. A resolved request enters `SimulationHandle::spawn_script_entity(actor_session, ...)`.
5. That method derives a session-fenced simulation handle for the supplied actor and submits the existing `SpawnCommandEntity` owner command.
6. The simulation owner commits entity creation/persistence and visibility dispatches; stale actor identity is rejected by the session fence.
7. Only after that exact owner outcome does the router publish one required targeted `entity.spawn_result` to the issuing plugin.

The router does not mutate the entity store directly and does not infer actor validity from usernames, proximity, or timing.

## Request and result contract

`request_id` uses the normal lowercase bounded script-id grammar. `entity_type` must be a fully namespaced lowercase resource id and must be present in the plugin manifest's exact `spawn_entities` allow-list. Position uses the existing finite script-coordinate contract.

The targeted result carries:

- original `request_id`;
- actor `player_id`;
- requested `entity_type`;
- exact requested `x`, `y`, `z`;
- `spawned = true` and `failure = nil` on simulation-owner success;
- `spawned = false` and a non-nil failure otherwise.

Public failure categories are intentionally bounded:

- manifest-allowed type absent from active server entity registry -> `unknown_entity_type`;
- stale/missing actor session fence -> `actor_unavailable`;
- owner queue full or queue-admission timeout -> `busy`;
- closed/stopped/response-timeout/shutdown/world-unavailable owner path -> `runtime_unavailable`;
- other owner rejection/mismatch/invalid outcomes -> `rejected`.

The result is the simulation-owner outcome. `spawned = true` does not claim that every client socket received or rendered the spawn packet.

## Capability and targeting

Entity spawn uses the existing exact `spawn_entities` manifest allow-list rather than a broad boolean mutation capability. An undeclared entity type fails host command admission before it can reach the simulation owner. Luau never supplies a plugin id; the host-attached provenance chooses the owner of the targeted result.

Result events do not require a broad event subscription and are delivered only to the issuing plugin.

## Executable evidence

### `mc-script`

The Luau feature suite covers the public request/result projection and raw-boundary validation:

- `lua_spawn_entity_emits_authorized_bounded_dto` proves `request_id`, actor, type, position, successful `spawned=true` projection and typed failure projection;
- the malformed/unauthorized spawn cases prove the allow-list and resource/coordinate validation;
- `every_lua_api_bounds_strings_before_command_construction` now exercises the new six-argument spawn signature and proves oversized entity identifiers are rejected before command construction.

Current feature-suite result before this documentation-only step:

```text
cargo test -p mc-script --features lua-runtime --quiet
running 201 tests
...
test result: ok. 201 passed; 0 failed; 0 ignored
```

### `mc-net`

`script::router::entity_spawn_tests` contains three focused checks:

- `entity_spawn_routes_through_owner_and_returns_targeted_result` is the full production round-trip: a strict disk plugin emits a host-attested spawn request for `minecraft:pig`; the actor is a real registered/loaded session; the simulation owner commits the spawn; the persisted entity snapshot contains the exact pig/type/position; required targeted result delivery reaches the plugin; and the Luau result handler emits a final host-attested acknowledgement.
- `unknown_entity_type_returns_targeted_failure_without_owner_mutation` proves a manifest-allowed but runtime-unknown type returns `unknown_entity_type`, leaves simulation queue depth at zero, and still reaches only the issuing plugin as a targeted result.
- `entity_spawn_failure_categories_are_stable` pins the public actor/busy/runtime/rejected taxonomy.

Focused result:

```text
cargo test -p mc-net --lib script::router::entity_spawn_tests -- --nocapture
running 3 tests
...
test result: ok. 3 passed; 0 failed
```

Existing simulation-owner test `script_entity_spawn_is_session_fenced_visible_and_saved` independently proves the underlying owner lane: successful spawn is visible and included in the save barrier, while unregistering the actor before owner processing returns `StaleSession` and does not create a second entity.

Benchmark: not applicable. This is an explicit plugin mutation reusing the existing simulation owner command lane; it adds no steady-state work when plugins do not issue entity spawn requests.

## Independent review

Exactly one bounded independent read-only reviewer returned terminal `PASS` with no findings. The review was restricted to the typed entity-spawn request/result contract, Luau projection, production router/owner path, test-only session fixture, public documentation, and this evidence. No second reviewer was run.

Final checkpoint validation on the reviewed tree:

- `cargo test -p mc-script --features lua-runtime --quiet` — 201/201 PASS;
- `cargo test -p mc-net --lib script::router::entity_spawn_tests -- --nocapture` — 3/3 PASS;
- `cargo test -p mc-net --lib --quiet` — 1969 passed / 5 ignored;
- `cargo fmt --all -- --check` — PASS;
- `cargo run -p xtask -- code-health` — `0 fail / KEEP`;
- `cargo clippy --workspace --all-targets -- -D warnings` — PASS;
- scoped `git diff --check` — PASS.

## Disposition

Typed entity-spawn subdomain: **PASS**.

Phase 5 item 3 remains **OPEN**. Player/inventory, world time, bounded world-block mutation, and now entity spawn have typed authority-preserving paths; the common combat mutation/result surface still needs its own bounded checkpoint before the six-domain matrix can close.
