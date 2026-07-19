# Core Replay Real-Client Adapter Plan

**Goal:** Execute the checked core replay manifest through the existing
repo-native Gradle `runClientAgent` path and emit a result accepted by the Rust
replay contract.

**Architecture:** Keep process ownership in `run-real-client-regression.sh` and
the fixed `:fabric-agent:runClientAgent` Gradle task. Add bounded, typed
`wait_ticks`, `move_by`, and `look` bridge commands, then let the existing
Python driver parse the checked manifest, issue those commands in order, and
write `solaris.core_replay.result.v1`. A small Rust validator cross-checks the
captured manifest/result pair. No command-string launcher hook is introduced.

## Tasks

- [x] Add RED bridge-core tests for bounded replay commands and client-thread
  ownership.
- [x] Implement the bridge facade and Minecraft 26.1.2 action adapter using the
  real client tick and movement-packet paths.
- [x] Add a RED fake-bridge integration test for exact manifest action order,
  normalized observations, and replay result cross-validation.
- [x] Implement strict manifest parsing and replay artifact emission in the
  existing agent driver.
- [x] Add a dedicated checked real-client pack/wrapper and fail-closed Rust
  artifact validator over the fixed Gradle runner.
- [x] Run focused Gradle/Rust tests, one approved real-client replay, full Cargo
  gates, and scoped review; record exact evidence limits.
