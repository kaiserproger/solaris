# Survival-mining crate-boundary evidence

Date: 2026-08-18

## Ownership cutover

The first bounded survival/mining extraction moves protocol-neutral fallback mining rules from `mc-net::play::survival` into the existing `mc_data::block_mining` owner.

`mc-data` now owns:

- fallback block hardness and correct-tool requirements for reduced/custom registries;
- fallback tool-family selection by block path;
- material-tier fallback mining speed;
- common pickaxe drop-tier admission;
- vanilla destroy-progress calculation, including submerged and airborne penalties;
- the canonical submerged mining-speed constant used by the fallback rule;
- resolved tag membership lookup, including `#tag` normalization and raw-id membership;
- food-rule lookup from item components with builtin fallback and use-duration projection.

`mc-entity::player_survival_26_1_2` owns the protocol-neutral player survival transitions and constants: damage/heal clamping, food/saturation addition, exhaustion spending, periodic regeneration/starvation decisions, maximum health/food, and movement/attack/break exhaustion constants. `mc-net::SurvivalState` is now only the mutable storage/wire adapter: production callers consume lower constants directly and delegate transitions downward instead of exposing a second semantic namespace. `mc-data::food::DEFAULT_USE_DURATION` likewise owns the vanilla default food duration used by the network adapter.

`mc-net` deliberately retains:

- `SurvivalState` storage and `ClientboundSetHealth` projection;
- live block/item/tag registry adaptation;
- tag-aware `mineable/pickaxe|axe|shovel|hoe` suffix selection when server data is available;
- item-component tool rules and Efficiency enchantment application;
- authoritative player pose inputs (`on_ground`, `eye_in_water`);
- world snapshots, block-break authority, mutation, drops, durability, and publication.

This preserves tag-aware gameplay behavior: the lower fallback API accepts the suffix chosen by the adapter rather than attempting to inspect server tags itself. No lower crate receives session, packet, world-mutation, or `mc-net` dependencies.

## Correctness fences

- `mc-data::block_mining` focused tests cover common block hardness, all supported fallback pickaxe speeds, axe/shovel/pickaxe family selection, wrong-tool fallback speed, iron/diamond/obsidian drop tiers, submerged and airborne destroy-progress penalties, instant zero-hardness progress, unbreakable blocks, and the unknown-block fallback; `TagsData` directly tests raw-id membership, `#tag` normalization, and invalid identifiers.
- `mc-entity::player_survival_26_1_2` directly tests eating eligibility, exhaustion spending, and saturated regeneration/starvation boundaries; the `mc-net survival` suite covers the live adapter end to end.
- `mc-data::food` directly tests item-component food overrides, builtin apple/bread fallback, the canonical default use duration, and component use-duration selection.
- Duplicate `mc-net` tests for pure fallback mining/drop/food semantics were removed after equivalent or stronger lower-owner coverage was installed; network tests retain registry/tag adaptation and authority integration coverage.
- `xtask code-health` requires fallback facts, suffix, speed, drop-tier, destroy-progress, food duration/rule lookup, and survival transitions to remain in lower owners. It rejects restored `fallback_mining_time`, `food_rule_for_item`, public tag/drop shims, `SurvivalHealthTick` re-exports, or `SurvivalState` semantic constant aliases in `mc-net`.
- The generic lower-crate reverse-dependency and transport/session leakage gates continue to forbid semantic backedges into `mc-net`.

Benchmark: not applicable. This is a pure ownership cutover preserving the existing constants and formulas.

## Validation

- `cargo test -p mc-data block_mining --quiet`: 5 passed, 1 existing ignored sidecar gate.
- `cargo test -p mc-data food --quiet`: 5 passed.
- `cargo test -p mc-net survival --quiet`: 67 passed after duplicate lower-semantic tests were removed.
- Final old-shim search across `crates/mc-net/src` returned no `SurvivalState` semantic aliases, `fallback_mining_time`, `food_rule_for_item`, default-food-duration duplicate, public tag/drop shim, or `SurvivalHealthTick` re-export.
- `cargo test -p mc-net --lib --quiet`: 1,925 passed, 5 existing ignored, 0 failed.
- `cargo test --workspace --quiet`: passed with exit 0; `mc-data` 254 passed/25 ignored, `mc-entity` 586 passed/6 ignored, `mc-net` 1,925 passed/5 ignored, `mc-world` 261 passed/15 ignored, and all enabled executable/integration groups completed without failure.
- `cargo fmt --all --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo run -p xtask -- code-health`: `0 fail`, `KEEP`.

No manual graphical/client or performance gate is claimed by this ownership checkpoint. `SurvivalState` storage and wire projection remain in `mc-net`; protocol-neutral survival/mining/food semantics are lower-owned. World/session authority and publication are intentionally not extracted by this evidence.
