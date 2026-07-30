# `mc-net` held-sharpness damage test extraction

Date: 2026-07-31

Checkpoint base: `22b112a253b7f6e4de6fa174fde44a5f2787c550`

## Result

The singleton test
`held_sharpness_uses_the_vanilla_26_1_2_damage_formula`
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module `crates/mc-net/src/play/tests/held_sharpness_damage.rs`.

The preserved test still builds the Solaris-required item registry, resolves
the stone sword, builds interaction state from the fluid test registry,
installs a level-three `minecraft:sharpness` sword in the selected hotbar
stack, and asserts exact base attack damage `5.0` and held enchanted attack
damage `7.0`.

The child imports every dependency explicitly and does not inherit the
aggregate file's `use super::*`. The immediately preceding protection
enchanting test and following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 2,626 physical lines, 37 test
functions, and 95 function-name entries. The moved class contains 24 physical
lines and one test. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 2,602 | 36 |
| `play/tests/held_sharpness_damage.rs` | 31 | 1 |

The exact original-versus-extracted 24-line class diff is empty. The original
aggregate and split result retain the same sorted multiset of 95 function
names. The combined test count remains 37.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: passed, 1 test.
- Exact original-versus-extracted 24-line class diff: empty; both copies have
  SHA-256 `fe96a3e4fde6c75b4590b1bb6c9c7a8a0108a4c890d543b7f151cee179bad9c5`.
- Original-versus-split function-name multiset: identical, 95 names.
- Unique test/module ownership, explicit imports, aggregate helper ownership,
  and adjacent aggregate boundaries: passed.
- `cargo test -p mc-net`: passed, 1,852 tests; 5 ignored; 3 doc-tests.
- `git diff --check`: passed.
- `cargo run -p xtask -- code-health`: passed.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Independent read-only review: passed with no findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
