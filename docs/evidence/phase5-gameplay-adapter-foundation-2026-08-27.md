# Phase 5 gameplay-adapter foundation — 2026-08-27

## Scope

This checkpoint audits and closes Phase 5 item 5: the first production Luau foundation must include tick/event scheduling, plugin storage, command registration, and gameplay adapters sufficient to build menus, an economy loop, owned zones/claims, and a colony/villager loop without Solaris-private imports.

The required pieces were already implemented incrementally on the current tree. This checkpoint does not replace them with parallel APIs; it verifies the owner boundaries and shipped compositions together, fixes one stale test-fixture call site, and records the minimal complete matrix.

## Tick/event scheduling

Luau exposes simulation-tick timers through:

```luau
solaris.schedule_timer(timer_id, delay_ticks)
solaris.cancel_timer(timer_id)
```

Timers use the pushed monotonic simulation tick rather than wall-clock time. The runtime owns bounded timer capacity, deterministic ordering, replacement/cancellation, callback rescheduling, and transactional staging with the handler invocation.

Focused evidence:

```text
cargo test -p mc-script --features lua-runtime lua::timer_tests -- --nocapture
running 8 tests
...
test result: ok. 8 passed; 0 failed
```

The suite proves no early firing, no duplicate callback after stale ticks, deterministic same-tick ordering, bounded capacity/input validation, atomic replace/cancel/reschedule, shared instruction budget, and rollback of staged timer changes after a handler failure.

## Durable plugin storage

The production storage actor provides plugin-scoped get/CAS/delete plus the storage half of atomic inventory/storage transactions. The journal is CRC-framed, bounded, revisioned, plugin-isolated, replayable, and owns a durable result outbox/ack protocol.

Focused evidence:

```text
cargo test -p mc-net --lib script::storage_tests -- --nocapture
running 19 tests
...
test result: ok. 19 passed; 0 failed
```

The suite covers restart, request-id replay/idempotency, substituted-content rejection, stale/absent mutations, quota rejection without partial state, batch atomicity, malformed/checksum/oversized fail-closed behavior, append/sync failure boundaries, result replay after closed delivery, and explicit unavailable/durability failures.

## Player/operator command registration

Plugin manifests declare literal `player_commands` and `operator_commands`. Command roots are bounded, globally exclusive, cannot shadow Solaris built-ins, route only to their owning plugin, and operator roots require verified operator context before event admission. Runtime disablement unregisters ownership before later host progress.

Focused evidence:

```text
cargo test -p mc-script --features lua-runtime player_command -- --nocapture
running 15 tests
...
test result: ok. 15 passed; 0 failed
```

This covers disk manifest parsing, root validation/bounds, aggregate registration atomicity, conflicts, owner-only delivery, bounded arguments/queues, operator authorization, poisoned-authority fail-closed handling, and ownership removal when a plugin disables.

## Menu adapter

`inventory_menus` gives a plugin bounded immutable menu DTOs, exact-player open/close routing, and targeted `inventory.menu.clicked` callbacks. Inventory menu state remains server/session owned; Luau receives values, not container/network handles.

Focused evidence:

```text
cargo test -p mc-net --lib script_menu_endpoint_tests -- --nocapture
running 8 tests
...
test result: ok. 8 passed; 0 failed
```

The suite proves exact connected-session identity, reliable ordering under outbound pressure, open/close ownership, click targeted-delivery backpressure, and explicit rejection when the session or script sink is unavailable.

## Economy adapter

The public economy primitive is not a privileged currency service hidden in Rust. Luau owns product/catalog/business policy, while Solaris exposes bounded inventory/storage transactions so one purchase can debit item currency, credit product items, and CAS the durable ledger atomically through existing player/storage authority.

The shipped `examples/plugins/basic-economy` plugin uses:

- plugin storage;
- inventory menus and click events;
- inventory/storage transactions;
- plugin zones;
- one literal `/economy` command;
- durable encoded purchase ledger and bounded catalog/refund terms.

The shipped-plugin harness proves the composition rather than only DTO construction:

```text
cargo test -p mc-test-harness --test plugin_examples -- --nocapture
running 5 tests
...
test result: ok. 5 passed; 0 failed
```

Relevant economy rows prove real Luau command routing, ledger read/menu creation, purchase/refund transaction shape, retained refund terms when configuration changes, bounded purchase counts, and corrupted-ledger fail-closed behavior.

## Zone / land-claim adapter

The plugin-owned zone registry supports bounded upsert/remove, exact plugin ownership, deterministic membership transitions, player cleanup, optional explicit protection policy, capacity limits, and targeted semantic command results. Ordinary plugin names/zone ids cannot accidentally enable protection.

Focused evidence:

```text
cargo test -p mc-net --lib script::zone_tests -- --nocapture
running 14 tests
...
test result: ok. 14 passed; 0 failed
```

The shipped `land-claims` plugin additionally passes real wire coverage in the full `plugin_examples` suite: a stranger's break and placement are rejected by the registered protected zone while owner policy/storage updates route through the public plugin API.

## Colony / villager adapter

The first colony adapter deliberately keeps colony identity, roles, orders, home policy, membership state, and durable intent in Luau. Rust exposes only generic bounded mechanisms:

- plugin storage for colony/member state;
- plugin-owned zone for colony territory;
- opaque ephemeral villager binding tokens, not entity handles;
- generic `idle` / `follow_position` villager goal requests through entity authority;
- literal `/colony` command routing.

The shipped `examples/plugins/colony-villager-scaffold` composes those primitives into recruit/status/role/order flows.

Real wire evidence is part of the five-test `plugin_examples` suite. The dedicated colony row:

```text
shipped_colony_scaffold_recruits_and_applies_updated_order_over_wire ... ok
```

It proves a client-visible villager, `/colony status`, durable recruitment, stored role/order generation, `hold` goal application, real player attacks killing the bound villager, and a later `home` order returning bounded `not_found` rather than retaining a stale entity reference.

### Fixture migration found during this audit

The first item-5 rerun exposed a real stale fixture rather than a colony-runtime defect. `villager-fixture` still called the pre-item-3 `spawn_entity(player_id, ...)` signature. The Luau handler therefore trapped before creating its test villager.

The fixture now uses the correlated API:

```luau
solaris.spawn_entity("fixture-villager", event.player_id, "minecraft:villager", ...)
```

and emits `fixture-villager-ready` only from `on_entity_spawn_result` after `event.spawned == true`. The harness diagnostic was also improved to report independently whether colony readiness, fixture readiness, and the expected villager spawn were observed within the unchanged five-second deadline. After the real fix, the colony wire test passes in about 2.5 seconds.

This makes the fixture stronger: it no longer treats command emission as entity-spawn success.

## Item-5 matrix

| Required foundation | Production public surface | Executable proof |
| --- | --- | --- |
| tick/event scheduling | simulation-tick timers + normal event handlers | `lua::timer_tests` 8/8 |
| plugin storage | durable get/CAS/delete + result outbox | `script::storage_tests` 19/19 |
| command registration | bounded player/operator roots + targeted command events | `player_command` focused suite 15/15 |
| menus | bounded server-owned inventory menu + targeted clicks | `script_menu_endpoint_tests` 8/8 |
| economy | inventory/storage transaction + storage/menu/zone composition | shipped `basic-economy`; full plugin examples PASS |
| zones/claims | owned zones, membership, protection, command results | `script::zone_tests` 14/14 + shipped `land-claims` wire gate |
| colonies | storage + zones + opaque villager binding + generic goals | shipped colony scaffold real wire recruit/order/death gate |

This is the "first gameplay adapters" foundation required by the plan. It is not a claim that every future economy, colony, UI, scheduling, or settlement feature is already exposed.

## Quality gates on the checkpoint

- `cargo test -p mc-script --features lua-runtime --quiet` — 202/202 PASS;
- `cargo test -p mc-net --lib --quiet` — 1973 passed / 5 ignored / 0 failed;
- timer focused suite — 8/8 PASS;
- storage focused suite — 19/19 PASS;
- player-command focused suite — 15/15 PASS;
- menu owner suite — 8/8 PASS;
- zone suite — 14/14 PASS;
- full shipped `plugin_examples` harness — 5/5 PASS;
- `cargo fmt --all -- --check` — PASS;
- `cargo run -p xtask -- code-health` — `0 fail / KEEP`;
- `cargo clippy --workspace --all-targets -- -D warnings` — PASS;
- scoped `git diff --check` — PASS.

Benchmark: not applicable. This checkpoint verifies explicit plugin event/command paths and fixes a test fixture; it adds no new steady-state server loop.

## Independent review

Exactly one bounded independent read-only reviewer returned **PASS** with no findings. The reviewer accepted the timer/storage/command/menu/economy/zone/colony matrix as sufficient for the plan's "first gameplay adapters" wording and accepted the villager-fixture migration as a stronger owner-confirmed spawn gate rather than a weakened timeout workaround.

## Disposition

Phase 5 item 5: **CLOSED**. The production Luau foundation has tick/event scheduling, durable plugin storage, bounded command registration, and shipped gameplay compositions for menus, economy, zones/claims, and colonies without Solaris-private imports.
