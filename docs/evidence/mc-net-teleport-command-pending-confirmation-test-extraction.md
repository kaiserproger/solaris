# `mc-net` pending-confirmation teleport-command test extraction

Date: 2026-07-31

Checkpoint base: `892261f97fb474f0c971a2e222847f44ea223181`

## Result

The singleton test
`teleport_command_waits_for_pending_confirmation_before_repositioning_player`
moved from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust
module
`crates/mc-net/src/play/tests/teleport_command_pending_confirmation.rs`.

The preserved test uses the same slow-client config, session/simulation
fixtures, original pose `(1.0, 65.0, 2.0)`, and `/tp 10 70 -5` console
command. With pending teleport id `7` and next id `8`, it still emits no newer
position-sync packet and leaves both ids and every pose component unchanged.

The child imports its command, teleport, session and state fixtures explicitly
and does not inherit the aggregate file's `use super::*`. The immediately
preceding resend-window test and following `state` helper remain
aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,286 physical lines and 52
test functions. The moved class contains 50 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,236 | 51 |
| `play/tests/teleport_command_pending_confirmation.rs` | 56 | 1 |

The exact original-versus-extracted 50-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 110 function
names. The combined test count remains 52.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 50-line class diff: empty.
- Original-versus-split function-name multiset: identical, 110 entries before
  and after.
- Unique test/module ownership, explicit imports, aggregate helper ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  and the resend-window test and `state` helper retain their single aggregate
  definitions.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no findings. The reviewer confirmed
  the empty 50-line class diff, unchanged 110-name function multiset,
  preserved 52-test total, explicit private-module ownership, aggregate
  boundaries, evidence metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
