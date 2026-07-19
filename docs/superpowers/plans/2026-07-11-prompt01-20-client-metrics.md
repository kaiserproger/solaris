# Prompt 01: 20-Client Metrics Evidence Plan

**Goal:** Make the existing VD8 20-client workload emit complete, machine-readable
Prompt 01 evidence without changing chunk scheduling or autoscale behavior.

## Task 1: Runtime and queue telemetry contracts

- [x] Add failing unit coverage for publishing and reading the latest bounded
  runtime tick percentile snapshot.
- [x] Add failing unit coverage for the observed maximum chunk result-queue depth.
- [x] Implement the smallest read-only metrics fields and snapshots required by
  those tests.

## Task 2: Bound-server workload telemetry

- [x] Expose a cloneable telemetry handle before `BoundServer::serve` consumes
  the server.
- [x] Publish runtime tick percentiles on the existing metrics cadence even when
  DEBUG logging is disabled.
- [x] Include session/entity/chunk counts and runtime memory used/limit in the
  handle snapshot without exposing mutable runtime internals.

## Task 3: Structured 20-client evidence

- [x] Measure first chunk, ring 1, ring 2, and full-window completion for every
  client in the existing VD8 workload.
- [x] Emit one versioned JSON report containing tick, chunk, queue/worker, lock,
  RSS/limit, world/cache/save, entity/session, outbound-pressure, and provenance
  evidence.
- [x] Assert required report fields are present and internally consistent.

## Task 4: Validation and Prompt 01 closeout

- [x] Run focused telemetry unit tests and the ignored 20-client gate.
- [x] Run formatting, code-health, workspace tests, and clippy.
- [x] Review the diff and update only the Prompt 01 evidence/status documents
  supported by the measured run.
