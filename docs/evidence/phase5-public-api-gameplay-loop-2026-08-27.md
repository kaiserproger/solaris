# Phase 5 public-API gameplay loop — 2026-08-27

## Scope

This checkpoint closes Phase 5 item 8:

> Close the phase with a real plugin implementing a small end-to-end gameplay loop using only the public API and no Solaris-private imports.

The selected real plugin is the shipped `examples/plugins/basic-economy` Luau plugin. The gate does not add a second toy implementation. It drives the shipped plugin through a fresh real-client survival loop and verifies the authoritative transaction outcome from the client side.

## Public API only

`examples/plugins/basic-economy/main.lua` contains no `require`, `import`, `crate`, or `mc_` private/runtime import path. Its Solaris calls are exclusively public API surface:

- `solaris.config()`;
- `solaris.send_message(...)`;
- `solaris.storage_get(...)`;
- `solaris.open_inventory_menu(...)`;
- `solaris.upsert_zone(...)`;
- `solaris.close_inventory_menu(...)`;
- `solaris.inventory_storage_transaction(...)`.

The plugin owns its catalog, pricing, refund terms, durable ledger encoding, menu policy, pending-request correlation, and business messages in Luau. Rust owns bounded menu/session transport, inventory authority, durable storage, zone authority, and the atomic inventory/storage transaction.

## Real-client gameplay gate

The server-only half of `tools/run-plugin-client-compat-gate.py` now doubles as the Phase-5 final gameplay-loop gate.

For the isolated gate only, the copied shipped plugin configuration changes the physical currency from `minecraft:emerald` to `minecraft:dirt` and the first Apples price to one Dirt. This does not change plugin code or add a privileged currency path; `basic-economy` already documents that its currency is any configured physical item. Dirt keeps the gate deterministic, fast, and obtainable by ordinary survival play without admin/debug setup.

The gate performs this chain on a fresh world:

1. start the production Solaris server with only copied shipped `basic-economy`;
2. launch the ordinary no-Loader real-client automation profile;
3. enter Play normally;
4. assert the client begins with zero Dirt and zero Apples;
5. scan the loaded nearby world for a natural exposed `minecraft:dirt` or `minecraft:grass_block` surface;
6. navigate to that block through normal movement input;
7. mine it through the ordinary client mining path;
8. observe the block become air, the world item drop, pickup, and exact inventory increase to one Dirt;
9. send `/economy` through the real client command path;
10. observe the shipped server-owned economy `ChestMenu` with first product `owned 0` and one-Dirt buy terms;
11. primary-click slot 0 through the normal container click path;
12. wait for the public `inventory_storage_transaction` owner path to commit;
13. observe Dirt debit to zero and Apples credit to two;
14. observe the plugin-authored success chat `Purchased Apples.`;
15. reopen `/economy` and require the durable storage-backed ledger to render `owned 1`.

No debug/admin command, operator privilege, direct inventory writer, private Solaris import, or test-only gameplay mutation is used in this loop.

## Fresh executable evidence

Fresh artifact:

```text
.analysis/plugin-client-compat/20260827T112310/result.json
```

Relevant result:

```text
server_only.passed = true
server_only.in_play = true
server_only.plugin = basic-economy
server_only.economy_menu_screen = net.minecraft.client.gui.screens.inventory.ContainerScreen

gameplay_loop.currency = minecraft:dirt
gameplay_loop.currency_source = natural_dirt_or_grass_block
gameplay_loop.currency_target = { x = 0, y = 101, z = 0 }
gameplay_loop.break_result.started = true
gameplay_loop.break_result.became_air = true
gameplay_loop.break_result.saw_drop = true
gameplay_loop.break_result.pickup_confirmed = true
gameplay_loop.break_result.pickup_detail = inventory_count=1 initial_count=0 expected_delta=1
gameplay_loop.purchase_slot = 0
gameplay_loop.currency_after_purchase = 0
gameplay_loop.apples_after_purchase = 2
gameplay_loop.success_message = Purchased Apples.
gameplay_loop.ledger_owned_after_purchase = 1
```

The same full invocation also reruns the item-7 client-required no-Loader rejection half, which passed independently in the same fresh run.

## Existing composition regression

The full shipped plugin harness remains green after the real-client loop change:

```text
cargo test -p mc-test-harness --test plugin_examples -- --nocapture

running 5 tests
...
test result: ok. 5 passed; 0 failed
```

This retains the broader economy/refund/claims/inventory/colony composition coverage in addition to the new real-client purchase loop.

## Gate design notes

The gate intentionally changes only copied runtime configuration, not tracked shipped plugin business logic. Using one Dirt rather than three Emeralds is not a test-only privileged shortcut: Dirt is mined through ordinary client gameplay and the purchase still traverses the exact same public plugin command/menu/inventory/storage API and authority boundaries as any configured currency.

The result is checked from multiple independent observable surfaces:

- world/block state and pickup result prove natural currency acquisition;
- inventory counts prove debit/credit;
- plugin chat proves the Luau transaction-result handler observed commit;
- reopened menu `owned 1` proves the durable ledger was written and subsequently reread.

Thus a false-positive result would require several independent world, inventory, plugin-event, and storage observations to agree incorrectly.

## Quality gates

- fresh real-client economy loop — PASS;
- full shipped `plugin_examples` suite — 5/5 PASS;
- public/private import audit — PASS, no private imports found;
- `cargo fmt --all -- --check` — PASS;
- `cargo run -p xtask -- code-health` — `0 fail / KEEP`;
- `cargo clippy --workspace --all-targets -- -D warnings` — PASS;
- scoped `git diff --check` — PASS.

One unrelated dead-code warning surfaced during the fresh gate build: `CommandPermissionConfig::relay_player_chat` had no call sites anywhere in `mc-net`. The unused accessor was removed without changing auth/chat behavior, restoring strict-Clippy cleanliness.

Benchmark: not applicable. This is an explicit user-driven gameplay transaction, not a new steady-state server loop.

## Independent review

Exactly one bounded independent read-only reviewer returned **PASS** on the same unchanged shipped-plugin/public-API loop, using fresh artifact `.analysis/plugin-client-compat/20260827T093618/result.json`. The reviewer accepted the config-only Dirt denomination, natural break/drop/pickup, real menu click, authoritative inventory/storage transaction result, and durable `owned 1` reread as sufficient for Phase 5 item 8.

The later current-tree rerun `.analysis/plugin-client-compat/20260827T112310/result.json` repeated the same loop successfully after the item-7 gate-hardening changes, so the reviewed behavior remains live on the newer tree. No second reviewer was started.

## Disposition

Phase 5 item 8: **CLOSED**. The shipped `basic-economy` plugin completes a real-client gameplay loop using only the documented public Luau API and no Solaris-private imports.
