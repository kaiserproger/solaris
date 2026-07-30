# `mc-net` pending-teleport movement-gate test extraction

Date: 2026-07-31

Checkpoint base: `3aa7d9fd2d55680e0f5f5ccdfdd5813a0b0b8d0c`

## Result

The singleton test
`pending_teleport_movement_gate_waits_without_duplicate_sync_packets` moved
from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/pending_teleport_movement_gate.rs`.

The preserved test starts with pending teleport id `12` at sent tick `0`. All
four movement guards still return true for the exact
`ServerboundMovePlayerPos` packet name, and the pending state still retains id
`12` afterwards without producing duplicate position-sync packets.

The child imports its teleport state and movement guard explicitly and does
not inherit the aggregate file's `use super::*`. The immediately preceding
unexpected-confirm test and following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,163 physical lines and 49
test functions. The moved class contains 13 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,150 | 48 |
| `play/tests/pending_teleport_movement_gate.rs` | 15 | 1 |

The exact original-versus-extracted 13-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 107 function
names. The combined test count remains 49.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 13-line class diff: empty.
- Original-versus-split function-name multiset: identical, 107 entries before
  and after.
- Unique test/module ownership, explicit imports, aggregate helper ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  and the unexpected-confirm test and `state` helper retain their single
  aggregate definitions.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no findings. The reviewer confirmed
  the exact 13-line class preservation, unchanged 107-name function multiset
  and 49-test total, explicit private-module ownership and order, aggregate
  boundaries, evidence metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
