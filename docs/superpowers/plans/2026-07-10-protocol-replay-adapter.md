# Manifest-Driven Protocol Replay Adapter Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Consume the checked core replay manifest through the existing Solaris
protocol parity path and prove two fresh runs produce identical normalized
observations.

**Architecture:** `mc-test-harness::replay` validates the requested protocol
lane, then delegates ordered actions to the existing core-action observer in
`parity`. An integration test owns fresh in-memory Solaris servers, compares
both runs, and validates a complete in-memory result against the manifest.

**Tech Stack:** Existing `mc-test-harness`, `mc-net` integration-test dev
dependencies, Tokio, strict replay DTOs.

## Constraints

- Reuse the existing login/configuration/play and `CoreAction` implementation.
- Do not add a second protocol client or action interpreter.
- Each deterministic run starts a fresh server/world and uses the same checked
  manifest.
- The adapter refuses a server kind whose required manifest lane is absent.
- The checked fixture's wait/move/look actions all execute; no action may be
  silently skipped.
- In-memory result provenance is clearly test provenance and is not persisted
  or claimed as a profile/oracle/client artifact.
- No vanilla server, GUI client, profile, 20-client load, or soak in this slice.

---

### Task 1: RED Manifest Replay Test

**Files:**
- Modify: `crates/mc-test-harness/tests/parity_oracle.rs`

- [x] Add an integration test that loads
  `tools/core-replay-scenarios/core-actions-seed-81.json`, calls the missing
  manifest replay adapter against two fresh Solaris servers, and requires equal
  normalized observations plus a schema-valid result.
- [x] Run the focused test and record the expected missing-adapter compile RED.

### Task 2: Minimal Adapter

**Files:**
- Modify: `crates/mc-test-harness/src/parity.rs`
- Modify: `crates/mc-test-harness/src/replay.rs`

- [x] Expose the existing core-action observation function for harness reuse.
- [x] Add `run_protocol_replay` that validates the manifest, selects
  `solaris_protocol` or `vanilla_oracle` from `ScenarioContext`, rejects a
  missing lane, and executes exact ordered actions.
- [x] Run replay/parity library tests and the focused integration test GREEN.

### Task 3: Determinism And Result Validation

**Files:**
- Modify: `crates/mc-test-harness/tests/parity_oracle.rs`

- [x] Assert both normalized observation sets are exactly equal and contain
  action-order, action-count, post-action-liveness, and inventory facts.
- [x] Build one in-memory passed protocol result with explicit test provenance,
  validate it against the manifest, serialize it, parse it, and validate again.
- [x] Add a lane-removal negative control proving the adapter fails closed.

### Task 4: Evidence And Gates

**Files:**
- Modify: `docs/milestones/M79.md`
- Modify: `docs/VALIDATION_LEDGER.md`

- [x] Record one deterministic Solaris protocol replay, while explicitly
  leaving vanilla oracle, real-client, profile, load, and soak evidence open.
- [x] Run focused and full Cargo gates, coverage audit, and scoped diff review.
