# Item-semantics crate-boundary evidence

Date: 2026-08-17

## Ownership cutover

Versioned protocol-neutral item-path semantics live in `mc-data::item_semantics_26_1_2` instead of being duplicated in network gameplay adapters.

`mc-data` owns:

- durability-tool path classification;
- vanilla maximum durability for wooden, stone, iron, diamond, golden, and netherite tools;
- mining-loot enchantability classification for pickaxes, axes, shovels, and hoes;
- enchanting offer tiers and bookshelf thresholds;
- supported Sharpness/Protection/Efficiency selection;
- Efficiency eligibility from item facts plus vanilla fallback;
- additional Fortune/Silk Touch selection by offer button.

`mc-net` retains live item-registry lookup, bookshelf world reads, inventory/XP/lapis mutation, stale-state validation, owner commits, and publication. Combat and enchanting adapters consume the lower rules.

## Correctness fences

- Direct `mc-data` tests cover durability classification/constants and mining-loot enchantability, including sword rejection.
- Existing `mc-net` enchanting tests continue to cover efficiency, fortune, silk touch, armor/sword offers, costs, owner commit, settlement, and recovery.
- `xtask code-health` requires all lower item-semantic primitives to remain in `mc-data`, requires combat to consume durability semantics, and requires enchanting to consume the mining-loot rule.
- Generic lower-crate reverse-dependency and transport/session leakage guards apply.

Benchmark: not applicable. These are constant-time deterministic classification rules.

## Validation

- `cargo test -p mc-data item_semantics_26_1_2`: 1 passed.
- `cargo test -p mc-net enchanting`: 16 passed.

Final formatter, strict-Clippy, code-health, and full-workspace results are recorded when this checkpoint closes.
