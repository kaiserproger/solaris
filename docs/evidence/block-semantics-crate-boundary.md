# Block-semantics crate-boundary evidence

Date: 2026-08-17

## Ownership cutover

Protocol-neutral 26.1.2 block-name semantics no longer live in `mc-net::play::chunk_stream`.

`mc-data::block_semantics_26_1_2` owns:

- the passable block-name table shared by movement, spawn safety, beds, falling blocks, and natural-spawn adapters;
- the natural generated-land fallback surface predicate used by herd planning.

`mc-net` retains only registry adaptation from `BlockRegistry` state records to raw block names and a thin compatibility wrapper required by existing `play.rs` imports. No session, world mutation, packet, channel, or async type moved downward.

## Correctness fences

- Lower tests cover representative plants, crops, aquatic plants, torches, solids, and all accepted natural herd fallback surfaces.
- Existing network tests continue to cover the full common-flower and non-colliding-crop compatibility sets.
- `xtask code-health` requires both predicates to remain in `mc-data` and rejects restoring the passable-name table in `mc-net`.
- The generic lower-crate transport/session and reverse-dependency guards apply to this module.

Benchmark: not applicable; this is an ownership-only cutover of constant-time name predicates.

## Validation

- `cargo test -p mc-data block_semantics_26_1_2`: 2 passed.
- `cargo test -p mc-net passable_block_names`: 2 passed.
- `cargo run -p xtask -- code-health`: `0 fail`, `KEEP`.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo test --workspace --quiet`: passed; `mc-data` 232 passed/25 ignored, `mc-entity` 581 passed/6 ignored, `mc-net` 1,932 passed/5 ignored, and `mc-world` 261 passed/15 ignored in this checkpoint.

No graphical/client or performance claim is made here.
