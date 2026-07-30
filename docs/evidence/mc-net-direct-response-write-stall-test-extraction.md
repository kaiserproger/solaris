# `mc-net` direct-response write-stall play-loop test extraction

Date: 2026-07-31

Checkpoint base: `ae9b41ef593ef37249475b137c185b0e1fc1d719`

## Result

The singleton test
`play_loop_closes_session_when_direct_response_write_stalls` moved from
aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/direct_response_write_stall.rs`.

The preserved test still uses the 256-byte duplex client/reader, exact
command-suggestion id and command, packet-body and frame encoding, client
write, three-write stall writer, buffer, session/simulation fixtures, starting
timeout count, slow-client config, and capacity-one outbound channel. It
preserves the exact pose and respawn, 250 ms timeout, complete `play_loop`
argument list with both `"DirectWriter"` names, exact timeout and clean-close
expectations, and `start_timeouts + 1` assertion.

The child imports every fixture explicitly and does not inherit the aggregate
file's `use super::*`. The immediately preceding outbound-write-stall test and
following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 2,918 physical lines and 41
test functions. The moved class contains 81 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 2,837 | 40 |
| `play/tests/direct_response_write_stall.rs` | 101 | 1 |

The exact original-versus-extracted 81-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 99 function names.
The combined test count remains 41.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 81-line class diff: empty.
- Original-versus-split function-name multiset: identical, 99 entries before
  and after.
- Unique test/module ownership, explicit imports, aggregate helper ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  and the outbound-write-stall test and `state` helper retain their single
  aggregate definitions.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no findings. The reviewer confirmed
  the exact 81-line class preservation, unchanged 99-name function multiset
  and 41-test total, explicit private-module ownership and order, aggregate
  boundaries, evidence metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
