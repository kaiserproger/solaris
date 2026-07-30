# `mc-net` bounded outbound-pressure draining test extraction

Date: 2026-07-31

Checkpoint base: `32f6c569994ce4009267336a7401f25e2e408757`

## Result

The singleton test
`play_loop_drains_bounded_outbound_pressure_without_shedding` moved from
aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/outbound_pressure_draining.rs`.

The preserved test still uses the 64-byte duplex reader, sink writer, buffer,
session registry, simulation channel, starting pressure snapshot, and
slow-client config. It preserves the capacity-16 outbound queue, exact
`1..=16` nonblocking prefill and `17..=80` awaited producer with their
expectations, exact pose and respawn, 250 ms timeout, complete `play_loop`
argument list, result and producer expectations, and both unchanged
slow-client pressure-counter assertions and messages.

The child imports every fixture explicitly and does not inherit the aggregate
file's `use super::*`. The immediately preceding closed-outbound-channel test
and following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,069 physical lines and 43
test functions. The moved class contains 88 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 2,981 | 42 |
| `play/tests/outbound_pressure_draining.rs` | 107 | 1 |

The exact original-versus-extracted 88-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 101 function names.
The combined test count remains 43.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 88-line class diff: empty.
- Original-versus-split function-name multiset: identical, 101 entries before
  and after.
- Unique test/module ownership, explicit imports, aggregate helper ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  and the closed-outbound-channel test and `state` helper retain their single
  aggregate definitions.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no findings. The reviewer confirmed
  the exact 88-line class preservation, unchanged 101-name function multiset
  and 43-test total, explicit private-module ownership and order, aggregate
  boundaries, evidence metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
