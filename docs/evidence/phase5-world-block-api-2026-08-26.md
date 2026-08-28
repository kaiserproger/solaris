# Phase 5 typed world-block API — 2026-08-26

## Scope

This checkpoint advances Phase 5 item 3 with one bounded common block-mutation surface. It does not claim that the complete player/world/block/inventory/combat/entity API matrix is finished.

API `0.6.0` adds:

- Luau command `solaris.set_block(request_id, dimension, block_id, x, y, z)`;
- capability `world_blocks`;
- targeted result `world.block_set_result` / `on_world_block_set_result`;
- typed Rust `ScriptWorldBlockSetRequest`;
- public failures `unknown_block`, `unsupported_dimension`, `out_of_world`, `busy`, `runtime_unavailable`, and `rejected`.

The API exposes no `WorldStorage`, chunk handle, block-state registry handle, simulation handle, region key, epoch, lock guard, or mutable batch.

## Closed first contract

The first mutation surface is intentionally narrow:

- `dimension` must be exactly `minecraft:overworld`;
- `block_id` must be one active namespaced registry block;
- the mutation selects that block's registry **default state** only;
- Y must be inside Solaris world height;
- X/Z must remain inside the existing ±30,000,000 script horizontal coordinate limit;
- one request changes one root block only;
- block-state properties, block entities, cross-dimension mutation, arbitrary NBT, and unbounded edit batches are not public API `0.6.0` features.

`request_id` uses the normal bounded script-id grammar. Both resource ids use the existing namespaced resource-id validation.

## Authority path

The production `ScriptRouter` performs only bounded adaptation:

1. host provenance and `world_blocks` capability are already admitted in `mc-script`;
2. `resolve_world_block_request` verifies the closed dimension/height/block-id contract and resolves the registry default `BlockStateId`;
3. the router submits the position/state through the existing server-owned single-block simulation command lane (`SimulationHandle::place_loader_block_server_owned`);
4. the simulation owner owns the actual `ApplyBlockEdits` world transition and visibility publication;
5. the router converts the exact owner outcome to one required targeted result for the issuing plugin.

The router never acquires the world lock or edits a chunk directly.

The method name of the existing internal owner adapter still references its original Loader caller, but its implementation is the generic server-owned one-block `ApplyBlockEdits` lane. No compatibility shim or second mutation path was introduced for this plugin API.

## Result semantics

A successful owner application publishes:

- original `request_id`;
- `dimension`, `block_id`, and exact integer coordinates;
- `applied = true`;
- `failure = nil`.

`applied` means the simulation owner accepted and applied the edit batch. It does **not** mean that the previous state was different: an idempotent same-state request is still an applied owner mutation and reports `applied = true`. Result states are closed: success is exactly `applied = true, failure = nil`; every rejection is exactly `applied = false` with a non-nil failure. `applied = false, failure = nil` is rejected as an inconsistent DTO rather than published.

Pre-owner validation failures are:

- unknown active block id -> `unknown_block`;
- any dimension other than `minecraft:overworld` -> `unsupported_dimension`;
- Y outside server world height or X/Z outside the script horizontal limit -> `out_of_world`.

Owner/runtime failures are deliberately compressed into stable public categories:

- queue full / queue-admission timeout -> `busy`;
- closed/stopped/response-timeout/shutdown/world-unavailable -> `runtime_unavailable`;
- other owner rejection/mismatch/stale/mutation outcomes -> `rejected`.

A targeted result is an authoritative owner outcome. It does not claim that every client socket accepted the resulting block/light packets or that every renderer displayed them.

## Capability and targeting

`capabilities = ["world_blocks"]` is parsed through the normal strict manifest path. Without that capability, `solaris.set_block(...)` traps synchronously with `command capability denied: world_blocks` before a command can cross the host boundary.

The plugin never supplies its own plugin id. Host-attached provenance selects the owner of `world.block_set_result`; result events are targeted and do not require broadcast subscription.

## Executable evidence

### `mc-script`

Focused Luau tests prove the typed command/result projection and capability denial:

```text
cargo test -p mc-script --features lua-runtime world_block -- --nocapture
running 2 tests
...
test result: ok. 2 passed; 0 failed
```

The complete Luau feature suite on the same tree passes:

```text
cargo test -p mc-script --features lua-runtime --quiet
running 201 tests
...
test result: ok. 201 passed; 0 failed; 0 ignored
```

### `mc-net`

`script::router::world_block_tests` contains three focused checks:

- `world_block_resolution_validates_closed_contract` proves successful default-state resolution plus `unsupported_dimension`, `out_of_world`, and `unknown_block` before owner admission;
- `world_block_failure_categories_are_stable` proves the public simulation-error taxonomy;
- `world_block_command_routes_through_owner_and_returns_targeted_result` is the production round-trip: a strict disk plugin emits the host-attested command, the router resolves `minecraft:stone`, a real simulation owner applies the block edit to an in-memory authoritative chunk, the test reads back the committed state, required targeted result delivery reaches the plugin, and its Luau callback emits a host-attested acknowledgement. The same test repeats the identical request and proves same-state application remains a successful owner outcome.

Focused result:

```text
cargo test -p mc-net --lib script::router::world_block_tests -- --nocapture
running 3 tests
...
test result: ok. 3 passed; 0 failed
```

The complete `mc-net` library suite on the same tree passes:

```text
cargo test -p mc-net --lib --quiet
running 1971 tests
...
test result: ok. 1966 passed; 0 failed; 5 ignored
```

Benchmark: not applicable. This is an explicit plugin mutation using the existing simulation command/commit path; it adds no steady-state work when no plugin issues the command.

One post-review full `mc-net` run transiently failed the unrelated `play::world_journal::tests::closed_writer_wakes_append_turn_waiter` scheduling assertion. The exact test immediately passed, and the immediate full `mc-net` rerun passed `1966/5`. The world-block checkpoint does not touch `world_journal`; this is recorded rather than silently treating the first run as green.

## Independent review

Exactly one independent read-only reviewer returned `CHANGES` with one Medium finding: the result DTO allowed the ambiguous state `applied = false, failure = nil`, and the router could theoretically construct it from an owner `Ok(false)` outcome. The fix closes the result state machine: construction and validation now require `applied == failure.is_none()`, owner `Ok(false)` maps to explicit `rejected`, and the Luau result test proves `false/nil` is rejected as inconsistent. Per repository policy, no second reviewer was run after this finite finding.

Post-review self-validation on the fixed tree:

- focused `mc-script` world-block tests: 2/2 PASS;
- focused `mc-net` world-block tests: 3/3 PASS;
- full `mc-script + lua-runtime`: 201/201 PASS;
- final full `mc-net --lib`: 1966 passed / 5 ignored;
- `cargo fmt --all -- --check`: PASS;
- `cargo run -p xtask -- code-health`: `0 fail / KEEP`;
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS.

## Disposition

Typed world-block subdomain: **PASS**.

Phase 5 item 3 remains **OPEN**. Player, inventory, world time, and now bounded world-block mutation have typed authority-preserving paths; common combat/entity mutation surfaces still require bounded typed checkpoints before the six-domain matrix can close.
