# Phase 1 structural-tripwire separation — 2026-08-27

## Scope

This checkpoint closes Phase-1 item 4 in `PUBLIC_ALPHA_PLAN.md`:

> Separate behavioral tests from structural tripwires. Structural checks may enforce crate ownership and dependency direction, but may not assert Rust statement order or source-text layout.

The audit distinguishes two legitimate categories:

- **behavioral contracts**: packet routing, response order, publication, retries, rollback, client bootstrap, etc. These belong in executable tests that call the real functions or run the real client/harness path;
- **structural ownership fences**: adapters must not import/instantiate forbidden world/session/lock/channel internals or bypass the intended owner crate. These remain valid `xtask code-health` checks.

## Removed source-text behavioral tripwires

### `mc-net` Play liveness

`crates/mc-net/src/play/liveness.rs` previously used:

```rust
include_str!("../play.rs")
```

and searched the source text for exact calls such as `is_serverbound_movement_packet(frame.id)` or `frame.id == Serverbound...::ID`.

That test no longer reads `play.rs`. `every_recognized_packet_has_a_dispatch_classification` now checks the actual packet IDs and real family-classifier functions. KeepAlive and teleport confirmation remain explicit direct classifications; every other recognized Play packet must match its actual runtime family classifier.

Focused result:

```text
cargo test -p mc-net --lib liveness -- --nocapture
running 8 tests
...
test result: ok. 8 passed; 0 failed
```

This preserves executable liveness/decode coverage without coupling the test to `play_loop_inner` source layout.

### Real-client launcher adapter

`crates/mc-test-harness/tests/real_client_manifest.rs` previously read `SolarisClientAgentMod.java` and required source strings such as:

- `NeoForge.EVENT_BUS.addListener`;
- `ClientTickEvent.Post`;
- `Minecraft.getInstance()`.

That implementation-text assertion is removed. The remaining test checks the Gradle launch/dependency/isolation configuration and actually runs the repo launcher `--check` path. The broader repository also has real-client and MCP gates that exercise the compiled adapter.

Focused result:

```text
cargo test -p mc-test-harness --test real_client_manifest \
  gradle_runclient_adapter_is_the_default_real_client_launcher -- --exact --nocapture
running 1 test
...
test result: ok. 1 passed
```

## Removed `xtask` statement-order checks

Three `code-health` scans contained explicit Rust statement/layout assertions and were removed.

### Block-edit adapter

Removed checks that normalized the whole source, sliced one function by textual signature, then required:

- exact `#[cfg(test)]` / `#[cfg(not(test))]` text shapes;
- one specific resync call before one exact acknowledgement call.

The scan still forbids direct owner-boundary bypasses such as world storage/locks/channels/task spawning from the adapter.

### Bucket interaction adapter

Removed the source-order chain that searched for owner commit, resync, inventory packets, acknowledgements, visible finalization and animation, then compared byte offsets to enforce one exact implementation sequence.

The scan still enforces that the adapter does not acquire world/lock/channel/task authority directly.

### Player-damage adapter

Removed exact source-expression checks for the accepted-health CAS implementation and the byte-offset ordering of shield commit, retry fence, regular commit and fallback packet.

The scan still forbids `SessionRegistry`, world storage, locks, channels, task spawning and other direct owner-bypass internals from the adapter.

These behavioral invariants are already covered by the relevant executable player-damage, shield, transaction, block-edit and bucket tests; `xtask` no longer acts as a second parser for Rust statement order.

## What structural checks remain

`xtask code-health` continues to use source inspection where the contract is genuinely structural, for example:

- forbidden dependency/import direction;
- direct use of `WorldStorage`, `SessionRegistry`, `Mutex`, `RwLock`, channels or task spawning in narrow adapters;
- required domain ownership/delegation symbols where the concern is which crate owns the rule rather than statement sequencing;
- legacy duplicate implementation patterns that would move a domain rule back into the wrong crate.

A targeted search after the cleanup finds no `.find("...")` statement-order comparisons in `crates/xtask/src/main.rs`.

## Validation

- `cargo test -p mc-net --lib liveness -- --nocapture` — 8/8 PASS;
- focused real-client launcher manifest test — 1/1 PASS;
- reviewer-fix `committed_bucket_response_orders_block_ack_before_inventory_update` — 1/1 PASS and decodes committed block update -> block acknowledgement -> inventory slot update from the real `commit_bucket_use_and_respond` path;
- reviewer-fix `rejected_player_block_edit_resyncs_before_exactly_one_ack` — 1/1 PASS and decodes authoritative resync -> exactly one acknowledgement from the real `apply_player_block_edit_batch_conditionally` rejection path;
- `cargo test -p mc-net --lib --quiet` — 1975 passed / 5 ignored;
- `cargo run -p xtask -- code-health` — `0 fail / KEEP`;
- `cargo fmt --all -- --check` — PASS.

The pre-review candidate also passed `cargo clippy --workspace --all-targets -- -D warnings` and scoped `git diff --check`. The only post-review edits are the two executable regression tests above plus this evidence/status disposition; affected-crate compilation/execution, code-health, and formatter gates were rerun on the final tree.

Benchmark: not applicable. This checkpoint changes test/static-analysis strategy only; no runtime production path is added or optimized.

## Review status

The single independent read-only reviewer returned terminal **CHANGES** with two findings: bucket response ordering and rejected block-edit resync-before-ack ordering had lost their only regression protection when the source-order tripwires were removed. Both findings are now covered by executable tests that call the real production functions and decode the emitted packet order. Project policy does not request a second reviewer after fixing findings.

## Disposition

Phase-1 item 4 is **closed**. Behavioral packet-order invariants are executable tests; `xtask code-health` retains only ownership/dependency structural fences for these adapters.
