# Phase 1 progress-wait inventory

Date: 2026-07-30

Checkpoint base: `ef982c6e8c20007a50f56cf534400eed16a8bc69`

## Scope

This checkpoint closes `PUBLIC_ALPHA_PLAN.md` Phase 1 item 2 by inspecting
first-party Rust, Java, Kotlin, Python, and shell test sources for progress
that depends on wall-clock sleeps, scheduler yields, or repeated state
sampling. It classifies the remaining loop candidates by the event that
advances them.

The immediately preceding checkpoints replaced the concrete debts in the
`mc-net` shutdown wait, harness chunk-pipeline idle wait, harness chunk-stream
cancellation wait, and `mc-script` targeted-delivery backpressure test. This
inventory rechecks the current tree rather than inferring closure from those
individual fixes.

## Current-tree result

An exact qualified-call search finds no first-party **test/gate** invocation of
`tokio::time::sleep`, `tokio::task::yield_now`, `std::thread::sleep`,
`Thread.sleep`, `time.sleep`, or `asyncio.sleep`. Two production regional-owner
coordination loops still call `std::thread::yield_now()`; they are runtime
backoff points, not test progress synchronization, and are outside this Phase-1
item's test/gate wait contract. `xtask` also retains forbidden-token string
literals used by structural ownership checks.

The state-loop candidate inventory has these dispositions:

| Candidate class | Current synchronization | Disposition |
| --- | --- | --- |
| Raw-TCP and parity frame loops | `read_frame_with_timeout`, `next_non_keepalive`, or the socket read itself | Exact packet arrival advances the test; the deadline is a failure watchdog |
| Fake bridge and HTTP server loops | blocking `accept`, request body reads, and a finite expected-request count | Exact process/socket activity, not state polling |
| Entity-scale load helpers | `tokio::sync::watch::Receiver::changed` followed by `borrow_and_update` | Exact published simulation tick |
| Outbound-pressure waits | generation-fenced session pressure notification | Notification registration plus a state recheck closes the race |
| Chunk-pipeline idle and cancellation waits | enabled `Notify` futures registered before authoritative counter reads | Exact permit/task drop or cancellation publication |
| Script-storage shutdown | enabled `Notify` registered before the stopped atomic read | Exact actor-stop publication |
| Synchronous concurrency tests | `recv_timeout` on one-shot or standard channels | Exact probe/commit signal; timeout is fail-only |
| `try_recv` loops | finite draining of already-produced command queues after the action | Collection inspection, not waiting for future progress |
| Atomic revision and generation retry loops | production lock-free snapshot/reconciliation code | Runtime consistency retry, not a test wait |
| Filesystem, parser, root-cause, and parent-directory loops | finite traversal with a shrinking collection or parent path | Bounded iteration, not progress polling |
| Non-Rust client/tool candidates | `CountDownLatch`, `wait_state_change`, `inotify`/`select`, timed pipe reads, or finite free-port collision retry | Exact event waits or bounded setup retry |

No candidate advances by repeatedly observing unchanged state. Timeouts remain
only around a packet, channel, notification, process, or filesystem event and
therefore bound failure rather than manufacture success.

## Reproduction

The exact sleep/yield call inventory is:

```sh
rg -l -m 1 \
  'tokio::task::yield_now\(|tokio::time::sleep\(|std::thread::sleep\(|Thread\.sleep\(|time\.sleep\(|asyncio\.sleep\(' \
  crates client-mod tools \
  --glob '*.rs' --glob '*.java' --glob '*.kt' --glob '*.py' --glob '*.sh' \
  --glob '!**/build/**' --glob '!**/target/**'
```

It returns no path. Candidate loops were then projected around atomic loads,
snapshots, nonblocking receives, process/file status, deadlines, and timeouts;
each hit was classified against its producer before being accepted as
event-driven or finite.

The alias/bare-call fence is:

```sh
rg -n -m 20 '\b(?:sleep|yield_now)[[:space:]]*\(' \
  crates client-mod tools \
  --glob '*.rs' --glob '*.java' --glob '*.kt' --glob '*.py' --glob '*.sh' \
  --glob '!**/build/**' --glob '!**/target/**'
rg -n -m 20 '(^|[;&|[:space:]])sleep[[:space:]]+[^=(]' \
  tools client-mod --glob '*.sh'
```

The qualified-call command returns no test/gate invocation. The broader bare-call
fence additionally reports the two production regional-owner
`std::thread::yield_now()` coordination points plus `xtask` forbidden-token
literals; neither is a test/gate wait. The shell-command form returns no match.

## Validation

- Exact sleep/yield call inventory: no matches.
- Bare-call inventory: only `xtask` forbidden-token literals; shell sleep
  command inventory: no matches.
- Rust state-loop candidate projection and disposition review: passed.
- Java/Kotlin/Python/shell candidate review: passed with no polling debt.
- Markdown links and scoped path checks: passed.
- `git diff --check`: passed.

This is a documentation/test-policy checkpoint. No production or test code
changed, so affected-package, workspace, Clippy, formatter, benchmark, and
graphical gates were not rerun.

Independent read-only review verdict: `pass`; no findings.

## Evidence boundary and next cursor

This proves closure only for Phase 1 item 2 on the current first-party source
tree. It does not classify ignored, flaky, feature-gated, or manual-only
tests, does not prove their behavior, and does not close Phase 1.

Benchmark: not applicable. The checkpoint changes documentation and test
policy status only.

Next: inventory Cargo features that alter workspace test discovery outside the
already classified `mc-script` `lua-runtime` and `mc-net` `load-bench`
boundaries, then select the first unexplained feature-gated test class.
