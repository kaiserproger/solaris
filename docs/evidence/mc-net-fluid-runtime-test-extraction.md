# `mc-net` fluid-runtime test extraction

Date: 2026-07-30

Checkpoint base: `6533ab46062399856c326309569f75dcfef42868`

## Result

The contiguous ten-test bucket-source, fluid-tick, decay, scheduling, and
lava-water interaction class moved from aggregate
`crates/mc-net/src/play/tests.rs` to the focused Rust module
`crates/mc-net/src/play/tests/fluid_runtime.rs`. Its exact boundary runs from
`bucket_items_resolve_fluid_sources` through
`water_lava_interactions_make_solid_blocks` and includes the shared
`seed_fluid_test_floor` and `run_fluid_test_step` helpers. The preceding
`sign_items_choose_floor_or_wall_sign_for_clicked_face` test and following
ignored local-sidecar door parity gate remain aggregate-owned.

The child imports its shared aggregate helpers explicitly and does not inherit
the aggregate file's `use super::*`. The extracted tests retain bucket source
resolution and inventory replacement, published-state preconditions, sideways
flow with the downward path blocked, no-neighbour-chunk materialization,
unsupported-flow decay, source-spread draining, current/shared simulation-tick
scheduling, and water-lava solidification.

The now-unused aggregate `AtomicUsize` and `Ordering` import was removed after
the first focused compile identified it; the child imports those test-only
types directly.

This is a test-ownership change only. Production code and behavior did not
change.

## Concentration

Before the extraction, `play/tests.rs` contained 5,639 physical lines and 120
test functions. Afterwards:

| File | Physical lines | Test functions |
| --- | ---: | ---: |
| `play/tests.rs` | 5,127 | 110 |
| `play/tests/fluid_runtime.rs` | 523 | 10 |

The moved body contains 511 physical lines, ten tests, two shared helpers, and
one nested test-world generator. The final structural comparison covers all
182 function names from the original aggregate module. The exact
original-versus-extracted body diff and the sorted function-name multiset are
unchanged. The combined test count remains 120.

Benchmark: not applicable. The checkpoint changes only test-module ownership
and has no performance contract.

## Validation

- Focused execution: 10 passed, 0 failed, 0 ignored.
- `cargo test -p mc-net`: 1,852 passed, 0 failed, 5 documented ignored;
  three doctests passed.
- `cargo run -p xtask -- code-health`: passed with `0 fail / KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed after
  removing the stale aggregate atomic import reported by the focused compile.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- Exact original-versus-extracted 511-line body diff: empty.
- Original-versus-split function-name multiset: all 182 entries identical.
- The child module uses explicit imports and contains no `use super::*`.
- Boundary inspection confirms that the preceding sign-mapping test and
  following ignored local-sidecar door parity gate remain aggregate-owned.
- Independent read-only review confirmed the extraction and found one
  documentation overclaim about downward-flow coverage; the wording now
  describes only the tested sideways flow with a blocked downward path.

No manual-client gate was run because this checkpoint changes only test-module
ownership and has no runtime behavior to exercise.
