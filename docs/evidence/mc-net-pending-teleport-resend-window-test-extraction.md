# `mc-net` pending-teleport resend-window test extraction

Date: 2026-07-31

Checkpoint base: `86d05fac4c511636a171085d9edb2cae742cf434`

## Result

The singleton test `pending_teleport_resends_after_vanilla_tick_window` moved
from aggregate `crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/pending_teleport_resend.rs`.

The preserved test uses the same pose `(12.5, 70.0, -3.25)`, pending teleport
id `7` at sent tick `100`, and next id `8`. Tick `120` still produces no
resend and leaves the writer empty. Tick `121` still produces one
position-sync packet with teleport id `8` and the exact pose coordinates,
stores pending id `8` at sent tick `121`, and advances the next id to `9`.

The child imports its protocol, teleport, pose, and decode fixtures explicitly
and does not inherit the aggregate file's `use super::*`. The immediately
preceding pending-confirmation behavior test and following `state` helper
remain aggregate-owned.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 3,236 physical lines and 51
test functions. The moved class contains 48 physical lines and one test.
Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 3,188 | 50 |
| `play/tests/pending_teleport_resend.rs` | 53 | 1 |

The exact original-versus-extracted 48-line class diff is empty. The original
aggregate and split result have the same sorted multiset of 109 function
names. The combined test count remains 51.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 1 passed, 0 failed.
- Exact original-versus-extracted 48-line class diff: empty.
- Original-versus-split function-name multiset: identical, 109 entries before
  and after.
- Unique test/module ownership, explicit imports, aggregate helper ownership,
  and adjacent aggregate boundaries: pass. The module declaration and test
  each occur once, the child contains no wildcard parent import or public item,
  and the pending-confirmation behavior test and `state` helper retain their
  single aggregate definitions.
- `cargo test -p mc-net`: pass, 1,852 tests passed and 5 ignored; all 3
  doc-tests passed.
- `git diff --check`: pass.
- `cargo run -p xtask -- code-health`: pass.
- `cargo test --workspace`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- Independent read-only review: pass with no findings. The reviewer confirmed
  the exact 48-line class preservation, unchanged 109-name function multiset
  and 51-test total, explicit private-module ownership, aggregate boundaries,
  evidence metrics and links, and the exact next cursor.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
