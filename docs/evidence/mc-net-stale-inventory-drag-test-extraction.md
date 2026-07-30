# `mc-net` stale inventory-drag test extraction

Date: 2026-07-31

Checkpoint base: `bc9d8241367c4fdc1ca3dd2608067ba30b34ff2d`

## Result

The singleton test
`stale_inventory_drag_resyncs_exact_owner_state_without_loss_or_publication`
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module `crates/mc-net/src/play/tests/stale_inventory_drag.rs`.

The preserved test still builds the exact item, carried-stack, player,
session, persistence, simulation, XP, and script fixtures; queues the same
three quick-craft clicks; proves the closing click waits for one owner command;
replaces the authoritative saved inventory; and asserts the exact recovered
inventory, conserved item count, single resync packet, empty outbound queue,
and absent persisted entity records.

The child imports every dependency explicitly and does not inherit the
aggregate file's `use super::*`. The immediately preceding rejected inventory-
drag test and following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 2,254 physical lines, 29 test
functions, and 82 function-name entries under the checkpoint's
whitespace-tolerant `fn` scan, including nested test-local helpers. The moved
class contains 127 physical lines and one test. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 2,127 | 28 |
| `play/tests/stale_inventory_drag.rs` | 141 | 1 |

The exact original-versus-extracted 127-line class diff is empty. Under the
same whitespace-tolerant scan, the original aggregate and split result retain
the same sorted multiset of 82 function names. The combined test count remains
29.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: passed, 1 test.
- Exact original-versus-extracted 127-line class diff: empty; both copies have
  SHA-256 `74d301af3fcf095495ad8e933c85949e9a453cf32c1cce1d08900f4bb36a711c`.
- Original-versus-split whitespace-tolerant function-name multiset, including
  nested test-local helpers: identical, 82 names.
- Unique test/module ownership, explicit imports, aggregate helper ownership,
  and adjacent aggregate boundaries: passed.
- `cargo test -p mc-net`: passed, 1,852 tests; 5 ignored; 3 doc-tests.
- `git diff --check`: passed.
- `cargo run -p xtask -- code-health`: passed.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Independent read-only review: changes requested. The function-name metric is
  now explicitly defined as the whitespace-tolerant scan that includes nested
  test-local helpers.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
