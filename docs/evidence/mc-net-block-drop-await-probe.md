# `mc-net` block-drop await-probe isolation

Scope: Phase 1 flaky-test closure for the block-drop journal await probes in
`mc-net`.

The first workspace validation run exposed a real test-isolation race:
`block_drop_rechecks_cancellation_and_session_after_each_journal_await` failed
because the process-wide probe attempted to notify a receiver that had already
closed. The existing async mutex serialized the three tests that installed the
probe, but it did not stop unrelated block-drop tests from reaching the same
production hook while a probe was installed.

The probe is now scoped to the exact Tokio owner task under test. Other owner
tasks cannot observe or consume it, and one atomic consumed flag permits only
the first matching await stage inside that task. The three probe tests no
longer require a serial-only mutex and can run with the rest of the `mc-net`
suite.

## Covered behavior

- cancellation after decision reservation closes the journal decision without
  mutating the block;
- stale-session rejection after reservation preserves the block;
- cancellation or stale-session rejection after append preserves the committed
  block edit without publishing a drop;
- an earlier reserved journal decision still drains before the later block-drop
  decision;
- a pending-fence clear mismatch fail-stops the owner without old publication.

The focused block-drop slice passed all eight selected tests. The final
workspace test, Clippy, formatter, code-health, and diff gates passed after the
isolation change.

`benchmark: not applicable`: this is test-only scheduling infrastructure and
does not change a production runtime path.

## Reproduction

Run the focused block-drop slice:

```sh
cargo test -p mc-net --lib \
  play::simulation::block_drop_tests::block_drop_
```

Run the workspace closeout gates:

```sh
cargo run -p xtask -- code-health
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

This evidence closes the observed in-process probe-isolation race. It does not
claim filesystem crash recovery, release-host performance, or real-client
block-drop behavior.
