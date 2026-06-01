# M67 Code Health Review

This review focuses on cleanup candidates, not immediate code changes. The goal
is to feed M68 with small, safe refactors that reduce future feature cost.

## High Priority

| Finding | Evidence | Risk | Suggested Cleanup |
|---|---|---|---|
| `mc-net/src/play.rs` is doing too many jobs | `crates/mc-net/src/play.rs` is about 8.5k lines and contains protocol loop, crafting, containers, block edits, fluids, plant lifecycle, placement, food/bow/shield use, and tests hooks. | New gameplay slices keep landing in one file, increasing merge risk and making one-off helpers look cheaper than reusable structure. | Extract review-safe domains first: `play/crafting.rs`, `play/block_edits.rs`, `play/plants.rs`, and `play/use_item.rs`. Move tests with the domain where practical. |
| Container click handling is duplicated by menu type | `apply_pickup_click`, `apply_furnace_pickup_click`, and `apply_chest_pickup_click` repeat cursor/slot merge logic with different accessors. Swap/throw/quick-move paths have the same pattern. | Container bug fixes must be replicated across inventory, crafting, furnace, and chest paths; adding barrels/special furnaces will multiply the cost. | Introduce a small menu-slot adapter trait or enum that exposes get/set/can-place/storage ranges, then share pickup/swap/throw core logic. Do not change wire behavior in the same commit. |
| Plant lifecycle policy is scattered | Age growth is in `next_crop_growth_state`, stem fruit in `stem_fruit_edits`, saplings in `sapling_tree_edits`, sweet berries in `handle_plant_use_on`/`sweet_berry_harvest`, cocoa placement in `play/item_blocks.rs`, and cocoa drops in `play/survival.rs`. | Kelp/chorus/mushrooms or stricter support checks will add more ad hoc branches and make vanilla-divergence claims harder to audit. | Create `play/plants.rs` with explicit policy structs/functions for growth, bonemeal, use-on harvest, placement, and drops. Keep deterministic local semantics visible in names. |
| Drop policy is split between local tables and fallback loot | `block_drop_stacks_from` checks `crop_drop_stacks` before generic loot fallback. Recent cocoa/crop additions expanded hardcoded deterministic tables in `play/survival.rs`. | More special drops will bury gameplay policy in survival helpers and bypass `mc-data` loot abstractions. | Decide M68 policy: either keep a clearly named `local_survival_drops` module, or start partial loot-table execution in `mc-data`. Avoid growing `crop_drop_stacks` inline. |
| Use-item-on dispatch is becoming a chain of feature hooks | `handle_use_item_on` checks container opens, bed/toggle, campfire, bucket, plant use, bonemeal, item placement, and placement post-processing in one function. | Ordering bugs become likely as more interactions are added; future hooks may accidentally shadow placement or consume acks differently. | Extract a small ordered interaction pipeline with explicit outcomes: handled/no-op/place. Keep ack behavior centralized. |

## Medium Priority

| Finding | Evidence | Risk | Suggested Cleanup |
|---|---|---|---|
| Block placement special cases live in multiple layers | Item mapping chooses signs/cocoa/crops in `play/item_blocks.rs`; `plan_place_block_edits` handles signs again and doors in `play.rs`. | Placement behavior is hard to reason about because item-to-block and block-to-edit phases both mutate state choice. | Split placement into item resolution and block-shape planning modules with clear responsibilities. Sign wall/floor choice belongs in item resolution; multi-block doors belong in shape planning. |
| One-use helpers are often created inside the large module | Examples include recent `stem_lifecycle_blocks`, `sweet_berry_harvest`, and many container-specific `*_menu_stack` helpers. Some are justified, but locality in `play.rs` hides whether they should be policy modules. | The code reads as many tiny private helpers with no domain boundary. Future cleanup must determine which helpers are abstractions vs just extracted branches. | During extraction, delete helpers that only obscure a single call site; keep helpers that become shared within a domain module. |
| Test fixtures are growing into a second registry model | `crop_test_reports` now contains crops, stems, cocoa, fruits, attached stems, jungle logs, and simple support blocks. | Fixture state ids become brittle and unrelated tests can fail when adding a new state with overlapping ids. | Move plant fixture creation into a dedicated helper module or assign named constants for fixture states. |
| Runtime architecture has drifted from the original spec | The project spec says world state is mutated from a main game thread and warns against mutexes on shared world state; current play code frequently locks `state.world` from async handlers. | This may be intentional milestone drift, but it should be explicit before M68 cleanup chooses ownership boundaries. | Record an ADR or spec amendment describing the current async/world-lock model and which locks are acceptable. |

## Low Priority

| Finding | Evidence | Risk | Suggested Cleanup |
|---|---|---|---|
| Static identifier parsing is repeated | Many modules use `Identifier::parse(...).expect("static identifier")`. | Low runtime risk, but repeated parsing makes policy tables noisier. | If a domain module accumulates many static ids, use local constants or small resolver helpers inside that module. |
| Milestone docs still use `this commit` in active tables until closeout | This is useful while writing but less useful after stacked local commits. | Minor traceability issue. | Keep the M66 closeout pattern: replace landed rows with commit hashes during closeout. |

## M68 Candidates From This Review

- Extract plant lifecycle policy from `play.rs`, `play/item_blocks.rs`, and
  `play/survival.rs` before adding kelp/chorus/mushrooms.
- Refactor shared container click behavior behind a menu-slot adapter, starting
  with no behavior changes and existing tests only.
- Split `handle_use_item_on` into ordered interaction handlers with one shared
  ack/apply outcome model.
- Move crop/plant deterministic drops into a named module or commit to partial
  loot execution in `mc-data`.
- Add a current architecture note for async handlers and world locking so M68
  cleanup does not chase an obsolete spec shape.
