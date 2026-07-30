# `mc-net` outbound write-stall play-loop test extraction

Date: 2026-07-31

Checkpoint base: `31bc56428fb3f6768d2120d4530b655b5425e670`

## Result

The singleton test `play_loop_closes_session_when_outbound_write_stalls` moved
from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/outbound_write_stall.rs`.

The preserved test still uses the 64-byte duplex reader, three-write stall
writer, buffer, session/simulation fixtures, starting timeout count,
slow-client config, and capacity-one outbound channel. It preserves the queued
entity-1 animation and exact expectation, exact pose and respawn, 250 ms
timeout, complete `play_loop` argument list with both `"SlowWriter"` names,
exact timeout and clean-close expectations, and `start_timeouts + 1`
assertion.

The child imports every fixture explicitly and does not inherit the aggregate
file's `use super::*`. The immediately preceding enchanting owner-commit test
and following `state` helper remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 2,837 physical lines and 40
test functions. The moved class contains 70 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 2,767 | 39 |
| `play/tests/outbound_write_stall.rs` | 89 | 1 |

The exact original-versus-extracted 70-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 98 function names.
The combined test count remains 40.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 70-line class diff: empty.
- Original-versus-split function-name multiset: identical, 98 entries before
  and after.
- Unique test/module ownership, explicit imports, aggregate helper ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  and the enchanting owner-commit test and `state` helper retain their single
  aggregate definitions.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: changes. The reviewer confirmed the exact
  70-line extraction, metrics, ownership, imports, package evidence, and links,
  then found that the next cursor described the final enchantment generically.
  The cursor now names the exact `minecraft:efficiency` assertion; the focused
  static/diff checks below cover that documentation-only fix.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
