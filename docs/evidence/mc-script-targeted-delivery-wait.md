# `mc-script` targeted-delivery wait evidence

Date: 2026-07-30

Checkpoint base: `c98de3dbdf07fb65ac22b96cedc9e9bc0b921052`

## Wait dependency

`targeted_event_delivery_waits_for_host_consumer_progress` filled a capacity-one
script event channel, spawned a required targeted delivery, yielded to the
Tokio scheduler, and then inspected the spawned task. The yield did not prove
that the task had actually polled the send future or registered for channel
capacity, so the test's observation depended on scheduler choice.

The test now pins the exact `enqueue_targeted_event` future and polls it with a
no-op waker. The first poll must return `Pending` while the first event occupies
the only queue slot. After the host receives that buffered event, a second poll
must return `Ready(Ok(()))`, and the host then receives the targeted result.
This observes the bounded channel state directly without sleeping, polling a
snapshot, or relying on another task being scheduled.

No production queue API, capacity, wakeup, delivery, or closure behavior
changed. The existing receiver-closure regression remains executable.

## Validation

- `cargo test -p mc-script --lib
  targeted_event_delivery_waits_for_host_consumer_progress -- --nocapture`:
  passed.
- `cargo test -p mc-script`: passed.
- `cargo run -p xtask -- code-health`: `0 fail`, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.

An exact qualified-call inventory across first-party Rust, Java, Kotlin,
Python, and shell sources finds no remaining test-side sleep or scheduler-yield
call. `xtask` still contains the words `sleep(` and `yield_now(` only as
forbidden-token strings used by structural ownership checks.

Independent read-only review verdict: `pass`; no findings.

## Evidence boundary

This checkpoint proves deterministic observation of one `mc-script`
backpressure test class. It does not by itself close the broader Phase 1
polling inventory, because snapshot loops without an explicit sleep/yield token
still require a separate bounded audit.

Benchmark: not applicable. This is a test-observation change with no production
performance contract.

Next: inventory snapshot/counter polling loops that do not contain an explicit
sleep or yield, then select the first real progress dependency rather than
closing Phase 1 from token absence alone.
