# `mc-net` closed outbound-channel play-loop test extraction

Date: 2026-07-31

Checkpoint base: `1db8484b1157ac248b5ff0c9142bc2f9f95d32e9`

## Result

The singleton test `play_loop_exits_when_outbound_channel_closes` moved from
aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/outbound_channel_close.rs`.

The preserved test still uses the 64-byte duplex reader, sink writer, buffer,
session registry, simulation channel, slow-client config, capacity-one
outbound channel, and dropped sender. It preserves the exact pose and respawn,
250 ms timeout, complete `play_loop` argument list with both
`"ClosedOutbound"` names, exact timeout expectation, and clean-close result
expectation.

The child imports every fixture explicitly and does not inherit the aggregate
file's `use super::*`. The immediately preceding direct-response-stall test and
following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 2,981 physical lines and 42
test functions. The moved class contains 63 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 2,918 | 41 |
| `play/tests/outbound_channel_close.rs` | 81 | 1 |

The exact original-versus-extracted 63-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 100 function names.
The combined test count remains 42.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 63-line class diff: empty.
- Original-versus-split function-name multiset: identical, 100 entries before
  and after.
- Unique test/module ownership, explicit imports, aggregate helper ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  and the direct-response-stall test and `state` helper retain their single
  aggregate definitions.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no findings. The reviewer confirmed
  the exact 63-line class preservation, unchanged 100-name function multiset
  and 42-test total, explicit private-module ownership and order, aggregate
  boundaries, evidence metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
