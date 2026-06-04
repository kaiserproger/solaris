# Real-Client Regression Pack

This directory tracks reproducible real-client scenarios for M94+ evidence.
These manifests are not protocol harnesses and do not count as client evidence
until a normal vanilla 26.1.2 client or PrismLauncher-launched client executes
them and the required artifacts are recorded.

Artifacts from a run stay local under `.analysis/real-client-runs/<run_id>/`:

- `manifest.json` copied from the tracked scenario manifest.
- `client.log` from the real client.
- `server.log` from `cargo run --bin mc-server -- --config example.toml`.
- `observations.json` with structured pass/fail notes for each step.
- `screenshots/` containing at least one screenshot for every scenario whose
  manifest entry has `screenshots_required: true`.
- `git.txt` with commit id and `git status --short --branch`.
- `toolchain.txt` with the autonomous preflight output.

Protocol bots, `wire-probe`, and `mc_test_harness::client::Client` runs may be
linked as supporting harness evidence, but they must not be recorded as the
`client_gate` for these manifests.

## Current Pack

- [`manifests/m94-regression-pack.json`](manifests/m94-regression-pack.json) is
  the bounded M94 scaffold. It covers the M94 checklist with focused scenarios
  and marks each one `manual-pending` until a real client run exists.
- `scoped_rows_manual_pending` records scoped rows that are not runnable as part
  of the bounded scenario pack yet; they stay non-green until a later real-client
  or owner-accepted degraded gate supplies artifacts.
