# Merchant-semantics crate-boundary evidence

Date: 2026-08-17

## Ownership cutover

Protocol-neutral villager trade semantics stay with the entity/data owners instead of being duplicated in the merchant container adapter.

`mc-entity::villager_merchant_26_1_2::VillagerTradeOffer` now owns validation that the two offered input stacks satisfy cost A, optional cost B, and the already-computed reputation-adjusted cost A count.

Generic item stack capacity used by merchant payment planning is owned by `mc-data::item_semantics_26_1_2::max_stack_for_stack`.

`mc-net` deliberately retains player inventory debit/return, cursor/window projection, selected-offer state, villager/session authority, owner commits, stale-state handling, protocol offer translation, and publication.

## Correctness fences

- Existing `mc-entity` merchant tests cover price/demand/reputation order, restock, persisted use/xp/level state, and out-of-stock behavior.
- Existing `mc-net` merchant tests cover exact payment movement, repeated payment remainder, reputation projection, stale replay rejection, out-of-stock rejection, commit behavior, gossip consequences, persistence, and reconnect/session paths.
- `xtask code-health` requires trade input matching to remain in `mc-entity` and requires the network merchant adapter to use lower trade and max-stack rules.
- Existing lower-crate reverse-dependency and transport/session leakage guards apply.

Benchmark: not applicable. This is a deterministic semantic ownership cut; inventory/session transaction ordering is unchanged.

## Validation

- `cargo test -p mc-entity villager_merchant_26_1_2`: 4 passed.
- `cargo test -p mc-net merchant`: 12 passed.

Final gates: `cargo fmt --check` passed; `cargo clippy --workspace --all-targets -- -D warnings` passed; `cargo run -p xtask -- code-health` reported `0 fail` / `KEEP`; `cargo test --workspace --quiet` passed with `mc-data` 236 passed/25 ignored, `mc-entity` 586 passed/6 ignored, `mc-net` 1,932 passed/5 ignored, and all executable integration groups green. The requested independent read-only Codex review was written to the handoff plan, but the local handoff runner remained `unknown`, so no reviewer verdict was available for this checkpoint.
