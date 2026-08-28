# Recipe-semantics crate-boundary evidence

Date: 2026-08-17

## Ownership cutover

Protocol-neutral recipe, crafting, stonecutting, and furnace selection semantics live in `mc-data::recipes` rather than in network container code.

`mc-data` now owns:

- item/tag ingredient matching through `ItemRegistry` and resolved `TagsData`;
- shaped and shapeless 3x3 crafting-grid matching;
- two-tool repair result calculation, including the vanilla five-percent durability bonus;
- final crafting result selection;
- stonecutting input acceptance plus result existence/count/max-stack validation;
- furnace/smoker/blast-furnace recipe selection;
- furnace fuel-duration scaling by cooking kind;
- deterministic furnace recipe experience calculation from recorded recipe use and seed.

The generic max-stack rule used by recipe validation is owned by `mc-data::item_semantics_26_1_2::max_stack_for_stack`.

`mc-net` deliberately retains recipe-book packet translation, container/window state, click planning, inventory settlement, furnace tick/slot mutation, owner commits, stale-state recovery, and publication. Existing compatibility helpers delegate to the lower rule owners.

## Correctness fences

- Existing `mc-data` recipe loader/validation tests remain green.
- Existing stonecutter tests cover advertised-offer ordering, invalid selection/input, quick-move capacity, transactional settlement, reconnect conservation, and stale-owner/session fail-closed behavior.
- Existing crafting tests cover shaped/shapeless matching, repair durability, inventory/table grids, transactional result moves, and settlement using the lower result planner.
- Existing furnace tests cover cooking-category selection, fuel timings, XP award, tick progression, owner commits, viewer publication, hoppers, and stale-state recovery while lower selection/math rules are delegated to `mc-data`.
- Merchant tests remain green and confirm that merchant price/reputation/input semantics stay in `mc-entity`; only inventory/payment transaction remains in `mc-net`.
- `xtask code-health` pins the complete lower recipe/crafting/furnace primitive set and requires the `mc-net` crafting, furnace, stonecutter, and recipe adapters to consume it.
- `max_stack_for_stack` is also pinned to the lower item-semantics owner.

Benchmark: not applicable. These are deterministic data-selection rules; container authority and scheduling are unchanged.

## Validation

- `cargo test -p mc-data recipes`: 28 passed, 1 ignored.
- `cargo test -p mc-net crafting`: 30 passed.
- `cargo test -p mc-net stonecutter`: 15 passed.
- `cargo test -p mc-net item_max_stack`: 2 passed.

Final gates: `cargo fmt --check` passed; `cargo clippy --workspace --all-targets -- -D warnings` passed; `cargo run -p xtask -- code-health` reported `0 fail` / `KEEP`; `cargo test --workspace --quiet` passed with `mc-data` 236 passed/25 ignored, `mc-entity` 586 passed/6 ignored, `mc-net` 1,932 passed/5 ignored, and all executable integration groups green.
