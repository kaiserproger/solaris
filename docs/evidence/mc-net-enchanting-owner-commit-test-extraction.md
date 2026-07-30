# `mc-net` enchanting owner-commit test extraction

Date: 2026-07-31

Checkpoint base: `996598dd1ff21694e90daf6af4e3568336846f61`

## Result

The singleton test
`enchanting_button_commits_xp_through_owner_before_mutating_table_inputs`
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module `crates/mc-net/src/play/tests/enchanting_owner_commit.rs`.

The preserved test still builds the required item and item-facts registries,
stone-pickaxe and lapis ids, interaction state, simulation owner, exact
player/session, persisted XP, and enchanting table inputs. It preserves the
pinned button request, pending-before-owner assertion and message, one
processed owner command, resulting XP and table state, consumed lapis, exact
`minecraft:efficiency` enchantment, atomically persisted XP/inputs and message,
and non-empty writer.

The child imports every fixture explicitly and does not inherit the aggregate
file's `use super::*`. The immediately preceding bookshelf-geometry test and
following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 2,767 physical lines and 39
test functions. The moved class contains 97 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 2,670 | 38 |
| `play/tests/enchanting_owner_commit.rs` | 107 | 1 |

The exact original-versus-extracted 97-line class diff is empty. The original
aggregate and split result retain the same sorted multiset of 97 function
names. The combined test count remains 39.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: passed, 1 test.
- Exact original-versus-extracted 97-line class diff: empty; both copies have
  SHA-256 `d081664621288a86e9c4e2000733073811031a0868be68f5e1138d0dd0d7538a`.
- Original-versus-split function-name multiset: identical, 97 names.
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
