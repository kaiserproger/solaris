# Inventory/equipment crate-boundary evidence

Date: 2026-08-17

## Ownership cutover

A large protocol-neutral inventory/equipment slice has moved out of `mc-net`.

`mc-data::inventory_semantics_26_1_2` now owns:

- canonical empty-stack normalization;
- stack compatibility including canonical enchantment-order comparison;
- bounded stack take/decrement operations;
- throw semantics;
- regular left/right pickup semantics;
- regular swap semantics;
- outside-window pickup/drop semantics.

`mc-data::item_semantics_26_1_2` owns player equipment-slot classification from immutable item facts.

`mc-data::armor` owns `ArmorStats`, vanilla armor/toughness damage reduction, and Protection-point damage reduction.

`mc-net::inventory` deliberately retains the mutable 46-slot player inventory, hotbar/offhand layout, range merge ordering, owner/session integration, equipment aggregation from live inventory contents, persistence, and publication. Its compatibility helpers delegate to the lower semantic owners.

## Correctness fences

- Direct lower tests cover stack identity/canonical-empty transitions and enchantment-order-insensitive compatibility.
- Existing `mc-net inventory` regressions cover merge order, partial/full inventory behavior, offhand pickup, overflow edges, custom-name/damage separation, crafting/container integration, scripts, death, placement, merchant inventory, and owner-commit paths.
- Existing combat/inventory tests continue to exercise armor/protection reduction through the network adapter.
- `xtask code-health` pins all lower stack-transaction primitives, equipment-slot classification, and armor-reduction math and requires `mc-net::inventory` to consume them.
- Generic lower-crate reverse-dependency and transport/session leakage guards apply.

Benchmark: not applicable. This wave moves deterministic constant-time/stack-local rules without changing scheduling or authority.

## Validation

- `cargo test -p mc-data inventory_semantics_26_1_2`: 2 passed.
- `cargo test -p mc-net inventory`: 85 passed.
- strict workspace Clippy passed after the cutover.
- `cargo run -p xtask -- code-health`: `0 fail`, `KEEP`.

Final gates: `cargo fmt --check` passed; `cargo clippy --workspace --all-targets -- -D warnings` passed; `cargo run -p xtask -- code-health` reported `0 fail` / `KEEP`; `cargo test --workspace --quiet` passed with `mc-data` 238 passed/25 ignored, `mc-entity` 586 passed/6 ignored, `mc-net` 1,932 passed/5 ignored, and all executable integration groups green.
