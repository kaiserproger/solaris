# `mc-net` shutdown-wait test evidence

Date: 2026-07-30

Checkpoint base: `4f6a958616b204add84c830b6f168a708465fd83`

## Wait dependency

`server::tests::shutdown_wait_wakes_when_shutdown_is_requested` spawned a
waiter and called `tokio::task::yield_now()` to bias the runtime toward polling
that task before the shutdown request. A scheduler yield is not proof that the
wait future registered its notification, so the test depended on task
scheduling rather than the event it claimed to verify.

The test now keeps the `ShutdownHandle::wait_requested` future locally and
polls it once to `Pending`. That exact poll registers the `Notify` waiter before
`ShutdownHandle::request` stores the authoritative flag and wakes waiters. A
second test requests shutdown first and then awaits the same API, proving the
atomic state check closes the pre-registration notification race.

The one-second `timeout` remains a fail-only watchdog around the exact future;
it is not used to advance or poll state. A bounded source scan found no
remaining `sleep` or `yield_now` progression dependency in `mc-net`.

Benchmark: not applicable. This changes only deterministic test orchestration,
not production behavior or a performance contract.

## Validation

- `cargo test -p mc-net shutdown_wait_`: 2 passed.
- `cargo test -p mc-net`: 1,850 passed, 5 ignored; 3 doc tests passed.
- `cargo run -p xtask -- code-health`: `0 fail`, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.

Independent read-only review verdict: `pass`; no findings.

The graphical/client gate was not run. This checkpoint changes test
determinism only and makes no gameplay, parity, performance, or release
readiness claim.
