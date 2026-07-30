# `mc-net` no-state pending-teleport guard test extraction

Date: 2026-07-31

Checkpoint base: `ccc65e17835e8799c6929a98b1da2219289b9947`

## Result

The singleton test
`pending_teleport_movement_guard_returns_false_without_pending_teleport` moved
from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/pending_teleport_movement_guard.rs`.

The preserved test passes the same absent pending teleport and exact
`ServerboundMovePlayerPos` packet name to the movement guard. The guard still
returns false when no teleport confirmation is outstanding.

The child imports the guard explicitly and does not inherit the aggregate
file's `use super::*`. The immediately preceding teleport-command test and
following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,295 physical lines and 53
test functions. The moved class contains 9 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,286 | 52 |
| `play/tests/pending_teleport_movement_guard.rs` | 11 | 1 |

The exact original-versus-extracted 9-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 111 function
names. The combined test count remains 53.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 9-line class diff: empty.
- Original-versus-split function-name multiset: identical, 111 entries before
  and after.
- Unique test/module ownership, explicit import, aggregate helper ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  and the teleport-command test and `state` helper retain their single
  aggregate definitions.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no blocking findings. The reviewer
  confirmed the empty 9-line class diff, unchanged 111-name function
  multiset, preserved 53-test total, explicit private-module ownership,
  aggregate boundaries, evidence metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
