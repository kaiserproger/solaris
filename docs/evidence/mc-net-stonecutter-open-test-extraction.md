# `mc-net` stonecutter-open test extraction

Date: 2026-07-30

Checkpoint base: `e5c2fe44062e58f30c63880cbf766b9aa108d7ac`

## Result

The writer-lock independence test
`stonecutter_open_uses_proved_menu_type_and_published_world_view` moved from
aggregate `crates/mc-net/src/play/tests.rs` into the existing focused
`crates/mc-net/src/play/tests/stonecutter.rs` module.

The preserved test opens a published stonecutter while the mutable world lock
is held, verifies the active window and proved menu type `24`, decodes the
open-screen packet, and rejects any redundant initial recipe-update packet.

The focused module imports every newly needed item explicitly and does not use
`use super::*`. The immediately preceding crafting-table-open test and
following `button_test_registry` helper remain aggregate-owned. Existing
stonecutter helper visibility and production code are unchanged.

This is a test-ownership change only. Production behavior did not change.

## Concentration

Before the move, the aggregate and focused files contained:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 4,036 | 78 |
| `play/tests/stonecutter.rs` | 614 | 12 |

The moved class contains 68 physical lines and one test. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,967 | 77 |
| `play/tests/stonecutter.rs` | 685 | 13 |

The exact original-versus-moved class diff is empty. The two files retain the
same sorted multiset of 151 function names and combined total of 90 tests.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: 0 failures, verdict `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- Exact original-versus-moved 68-line class diff: empty.
- Original-versus-current two-file function-name multiset: all 151 entries
  identical.
- The moved test name occurs exactly once; no wildcard import or public
  visibility was added.
- Boundary inspection confirms that the preceding crafting-table-open test and
  following `button_test_registry` helper remain aggregate-owned.
- Independent read-only review: passed with no findings.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
