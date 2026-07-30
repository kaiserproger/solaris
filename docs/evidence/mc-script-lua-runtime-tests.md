# `mc-script` `lua-runtime` feature-gated test classification

Scope: Phase 1 feature-gated test inventory for `crates/mc-script`.

`mc-script` deliberately has an empty default feature set. Its default build
owns the runtime-independent event, command, validation, queue, and capability
contracts. The `lua-runtime` feature adds the production Luau VM, plugin
discovery/configuration, loader manifests, timers, and startup worldgen
declarations together with their optional dependencies.

`mc-server` enables `mc-script/lua-runtime` for production, and
`mc-test-harness` enables it for integration coverage. The feature gate is
therefore a dependency/build boundary, not a parked implementation or a manual
test prerequisite.

## Inventory

Comparing the two Cargo test lists yields exactly 80 additional tests:

| Feature-only test prefix | Count | Owned boundary and exact close condition |
| --- | ---: | --- |
| `entity_interaction_tests` | 1 | Executes an interaction event through Lua and checks the exact fifteen-field payload. After any interaction-event or Lua projection change, the explicit feature suite must pass with the full payload unchanged or deliberately versioned. |
| `entity_kill_tests` | 1 | Executes an entity-kill event through Lua and checks its exact payload. Re-run after kill-event or projection changes; no field may disappear or change meaning without an API-version change. |
| `item_pickup_tests` | 1 | Executes an item-pickup event through Lua and checks its exact payload. Re-run after pickup-event or projection changes; the Lua observation must match the runtime-independent event contract. |
| `player_death_tests` | 1 | Executes a player-death event through Lua and checks its exact payload. Re-run after death-event or projection changes; the Lua observation must retain the typed death snapshot. |
| `lua::loader_tests` | 6 | Client-bundle manifest permissions, cache identity/path/hash fences, artifact verification, and the shipped two-owner fixture. Re-run after Loader manifest/runtime changes and on the release candidate; every invalid bundle must fail startup and the checked fixture must remain discoverable/runnable. The final server-only/client-required Loader matrix remains a separate release gate. |
| `lua::player_inventory_tests` | 2 | Capability rejection plus exact player-inventory request/result fields through a real Lua handler. Re-run after inventory API changes; unauthorized calls must emit nothing and authorized targeted results must retain their exact fields. |
| `lua::plugin_config_tests` | 8 | Optional config loading, structural/size/Lua-boundary limits, fresh-table isolation, and invalid-plugin containment. Re-run after config or discovery changes; malformed input must be rejected before command registration while a missing config remains an empty table. |
| `lua::tests` | 38 | Core VM sandbox, memory/fuel/queue bounds, plugin identity and command ownership, event dispatch, capability checks, DTO validation, targeted callbacks, shipped examples, and handler failure isolation. Re-run after any host/API/sandbox change and on the release candidate; all bounded-failure and cross-plugin isolation assertions must pass. |
| `lua::timer_tests` | 8 | Pushed simulation-tick scheduling, ordering, replacement/cancellation, per-tick bounds, shared fuel, stale ticks, and atomic staged changes. Re-run after timer or tick-delivery changes; callbacks must remain push-driven, ordered, bounded, and rollback-safe. |
| `lua::worldgen_tests` | 14 | Startup-only ore/settlement declarations, duplicate ownership, descriptor bounds/references, missing source, shipped profiles, and vanilla fallback. Re-run after plugin discovery or worldgen-declaration changes; invalid or conflicting plans must fail startup and the shipped profiles must resolve deterministically. |

The four top-level payload modules also contain runtime-independent event-schema
tests in the default suite. Only their exact Lua-handler execution tests require
the VM feature.

No test in this class is ignored, self-skipping, graphical, network-dependent,
or dependent on local Mojang data. The feature suite uses in-process VMs,
temporary directories, and checked-in plugin fixtures.

## Current disposition

The current tree produced:

- default: `85 passed; 0 failed; 0 ignored`;
- `--features lua-runtime`: `165 passed; 0 failed; 0 ignored`.

The 165-test feature result contains the 85 runtime-independent tests plus the
80 feature-only tests above. Running the default suite alone is not sufficient
evidence for a change to `lua.rs` or another VM-only test path; the explicit
feature command is the owning gate.

These package tests prove the in-process `mc-script` contract only. They do not
close the production server adapter, persistence, vanilla-client/server-only,
client-required Loader, or end-to-end gameplay-plugin release gates.

`benchmark: not applicable`: this checkpoint inventories and validates a
feature-gated correctness class; it changes no measured runtime path and has no
mapped performance contract.

## Reproduction

List and run both configurations explicitly:

```sh
cargo test -p mc-script -- --list
cargo test -p mc-script

cargo test -p mc-script --features lua-runtime -- --list
cargo test -p mc-script --features lua-runtime
```

The feature-only inventory is the sorted set difference between the two
`--list` outputs.
