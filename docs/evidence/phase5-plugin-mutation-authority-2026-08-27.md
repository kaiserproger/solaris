# Phase 5 plugin mutation authority boundary — 2026-08-27

## Scope

This checkpoint closes Phase 5 item 4: gameplay/state mutations emitted by Luau must enter an accepted owner/actor boundary and return a semantic result without exposing mutable engine internals to scripts.

The audit is intentionally about authoritative state mutations. Bounded presentation/session-control commands (`send_message`, `broadcast`, `disconnect`, inventory-menu presentation, and Loader screen presentation) are not claimed as simulation mutations: they do not write world/entity/inventory/storage gameplay state. Menu gameplay adapters remain owned by Phase 5 item 5, while disconnect/reload lifecycle ordering remains owned by item 6.

## Cross-domain console escape removed

The previous plugin API exposed `solaris.run_console(command)` plus `console_commands = [...]`. Even with an exact root allow-list, that was an authority escape hatch because a plugin could enter heterogeneous operator command implementations (`time`, `weather`, `give`, `tp`, `summon`, `gamemode`, `kill`, `save-all`, `stop`, etc.) instead of using one typed mutation/result contract.

This checkpoint removes that path from the plugin API entirely:

- `ScriptCommand::RunConsoleCommand` is gone;
- `ScriptCommandCapability::RunConsoleCommandRoot` and its public capability kind are gone;
- disk `plugin.toml` no longer accepts `console_commands`;
- Luau no longer exposes `solaris.run_console`;
- the script router no longer receives runtime-control or chunk-pipeline handles solely to execute operator commands;
- active documentation contains no `run_console` / `console_commands` contract.

The ordinary server/operator console implementation still exists for operators. Only the Luau plugin escape hatch was removed.

Existing plugin tests which used `/time set day` were migrated to the already-typed authority-preserving API:

```luau
solaris.set_world_time("set-day", 1000)
```

The migrated raw TCP server fixture and operator-only plugin command harness both pass.

## Loader mutations now have semantic results

Two Loader mutations were already routed through server authority but remained fire-and-forget. They are now correlated typed requests:

```luau
solaris.place_loader_block(request_id, block_id, x, y, z)
solaris.grant_loader_block_item(request_id, player_id, block_id, count)
```

### Loader block placement

`ScriptLoaderBlockPlacementRequest` bounds the request id and namespaced block id before routing. The router verifies:

1. an active Loader manifest exists;
2. the host-attached plugin owns the exact verified Loader block id;
3. coordinates are within the public horizontal coordinate bound and world height;
4. the mutation is submitted through `SimulationHandle::place_loader_block_server_owned` and the existing server-owned block-edit transaction.

Only after the owner outcome does Solaris publish required targeted `loader.block_placement_result` with the original request/block/position plus `placed` and `failure`.

Failures are bounded as `loader_unavailable`, `not_owned`, `out_of_world`, `busy`, `runtime_unavailable`, or `rejected`.

### Loader item grant

`ScriptLoaderItemGrantRequest` bounds request id, player id, namespaced block id, and count (`1..=64`). The router resolves the item only from the host-attached plugin's verified Loader ownership, then calls `SessionRegistry::route_loader_item_grant`.

That existing endpoint reserves the exact acknowledged Play-session inventory owner, waits for its owner-side commit, persists the canonical inventory before publication, and rejects a full inventory without mutation. Only after the endpoint returns does Solaris publish required targeted `loader.item_grant_result` with `granted` and a semantic failure.

Public failures are `loader_unavailable`, `not_owned`, `player_unavailable`, `inventory_full`, `runtime_unavailable`, or `rejected`.

## Authoritative mutation inventory

The current public mutation-bearing script surface is:

| Mutation | Accepted owner/actor boundary | Semantic result |
| --- | --- | --- |
| plugin storage compare-and-swap/delete | durable plugin-storage actor | targeted storage CAS/delete result with version/failure |
| inventory + storage transaction | inventory/storage transaction adapter and durable storage authority | `inventory.storage_transaction.result` |
| player inventory transaction | exact player session inventory owner | `player.inventory_transaction_result` |
| zone upsert/remove | plugin-owned zone adapter | `zone.command_result` |
| villager binding/goal | session/entity authority and opaque binding fence | villager binding/goal result |
| player teleport | session-fenced simulation/player-pose commit | `player.teleport_result` |
| world time | simulation owner | `world.time_set_result` |
| world block | registry validation + simulation-owner block-edit lane | `world.block_set_result` |
| Loader custom block placement | Loader ownership + simulation-owner block-edit lane | `loader.block_placement_result` |
| Loader custom block item grant | Loader ownership + exact Play-session inventory owner | `loader.item_grant_result` |
| entity spawn | actor-session fence + simulation owner | `entity.spawn_result` |
| entity damage | simulation owner + generic server-entity combat kernel | `entity.damage_result` |

`ListOnlinePlayers` and plugin-storage get are reads, not mutations. `SendChatMessage`, `BroadcastChatMessage`, `DisconnectPlayer`, `OpenInventoryMenu`, `CloseInventoryMenu`, and `OpenClientScreen` are bounded presentation/session-control operations rather than writes to authoritative world/entity/inventory/storage gameplay state; they are therefore outside the item-4 mutation matrix.

## No engine authority crosses `mc-script`

`crates/mc-script/Cargo.toml` depends only on script/runtime support crates (`luaur`, `mlua`, `serde`, `sha2`, `tempfile`, `tokio`, `toml`, `tracing`). It has no dependency on `mc-net`, `mc-world`, `mc-entity`, `mc-protocol`, or server persistence crates.

A scoped code search for engine authority/network/storage names (`SimulationHandle`, `SimulationCommand`, `SessionRegistry`, `EntityAttackOutcome`, `RegionalOwner`, `WorldStorage`, `PlayerPersistedState`, `OutboundCommand`, `mc_protocol`) returns none in `mc-script`. The only lock-name matches are private synchronization inside the script host itself (`StdMutex`, `RwLock`, and an invocation-state `MutexGuard`), not game-engine lock guards exposed in public DTOs.

Public commands/results contain bounded value DTOs, opaque numeric ids/tokens, strings, primitive coordinates/amounts, and semantic failure enums. They do not contain simulation handles, regional owner handles/epochs, session objects, entity references, network packet DTOs, world-storage handles, or persistence writers.

## Executable evidence

### Loader router result path

`cargo test -p mc-net --lib script::router::loader_mutation_tests -- --nocapture`:

```text
running 3 tests
...
test result: ok. 3 passed; 0 failed
```

This proves:

- Loader block success is reported only after the simulation owner commits the custom canonical state to world storage;
- Loader item ownership resolution with a missing player returns targeted `player_unavailable` rather than silently dropping the mutation;
- Loader block queue/runtime error categories and Loader inventory failure categories remain stable.

### Exact Loader inventory owner

`cargo test -p mc-net --lib script_loader_item_endpoint_tests -- --nocapture`:

```text
running 2 tests
...
test result: ok. 2 passed; 0 failed
```

This existing production endpoint evidence proves that a successful Loader grant waits for the exact acknowledged Play-session owner commit, updates persisted canonical inventory, and that full inventory / missing acknowledgement preserve state.

### Console-escape migration fixtures

`cargo test -p mc-server --test play lua_plugin_loaded_from_disk_replies_to_join_and_chat_over_the_wire -- --nocapture`:

```text
running 1 test
...
test result: ok. 1 passed; 0 failed
```

The disk plugin now uses typed `set_world_time` rather than `run_console("time set day")` and still produces the expected world-time publication over the real wire path.

`cargo test -p mc-test-harness --test commands lua_operator_command_is_hidden_from_non_operators_and_routes_for_operators -- --nocapture`:

```text
running 1 test
...
test result: ok. 1 passed; 0 failed
```

The operator-only plugin command remains operator-gated while its world mutation uses the typed `world_time` capability.

## Final primary gates

- `cargo test -p mc-script --features lua-runtime --quiet` — 202/202 PASS;
- `cargo test -p mc-net --lib --quiet` — 1973 passed / 5 ignored / 0 failed;
- Loader router mutation tests — 3/3 PASS;
- exact Loader item-owner tests — 2/2 PASS;
- migrated `mc-server` disk-Luau wire test — 1/1 PASS;
- migrated operator-command harness test — 1/1 PASS;
- `cargo fmt --all -- --check` — PASS;
- `cargo run -p xtask -- code-health` — `0 fail / KEEP`;
- `cargo clippy --workspace --all-targets -- -D warnings` — PASS after replacing one single-arm match with `if let`;
- scoped `git diff --check` — PASS.

Benchmark: not applicable. The changes remove a generic mutation route and add result publication to explicit plugin requests; there is no steady-state work when plugins issue no such commands.

## Independent review

Exactly one bounded independent read-only reviewer returned **PASS** with no findings. The reviewer accepted the authoritative-mutation classification, Loader result timing, removal of the generic plugin console escape, the no-engine-authority boundary, and the exclusion of bounded presentation/session-control operations from item 4 given the explicit Phase-5 item-5/item-6 ownership split.

## Disposition

Phase 5 item 4: **CLOSED**. Authoritative plugin gameplay/state mutations enter accepted owner/actor boundaries and return semantic outcomes without exposing engine authority, network DTOs, or persistence writers to scripts.
