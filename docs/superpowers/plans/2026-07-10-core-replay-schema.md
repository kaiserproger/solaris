# Core Replay Schema Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add one strict, versioned scenario/result contract that can be shared
by protocol, vanilla-oracle, and real-client replay adapters.

**Architecture:** A new public `mc-test-harness::replay` module owns serde DTOs,
semantic validation, and manifest/result cross-validation. Existing parity,
load, and real-client runners remain unchanged in this slice; later adapters
consume the contract instead of duplicating it.

**Tech Stack:** Rust 1.94, existing workspace `serde`/`serde_json`,
`mc-test-harness` unit tests, checked JSON fixture.

## Global Constraints

- Exact schemas are `solaris.core_replay.scenario.v1` and
  `solaris.core_replay.result.v1`.
- Every struct and enum parser rejects unknown fields/variants.
- A scenario records a seed, exact ordered actions, at least one execution
  lane, expected invariants, and every gate required by each lane.
- A result records the same seed/actions, one lane, invariant results,
  observations, commit/config/hardware/build/sidecar provenance, and an outcome
  for every required gate.
- Skipped, degraded, blocked, and failed checks require a concrete reason.
- A passed result is invalid unless all required gates and invariants passed.
- Paths in evidence metadata are repository-relative and cannot traverse
  parents.
- No world, screenshot, Mojang data, or runtime output enters the fixture.
- This slice defines the contract only; it does not claim a replay, oracle,
  real-client, profile, or soak pass.

---

### Task 1: RED Contract Tests

**Files:**
- Create: `crates/mc-test-harness/src/replay.rs`
- Modify: `crates/mc-test-harness/src/lib.rs`

- [x] **Step 1: Add failing schema tests**

Require:

- a minimal valid scenario parses;
- unknown schema fields and unknown action variants fail;
- empty lanes/actions/invariants/gates fail;
- a result missing provenance or a required gate fails;
- non-passing gates without reasons fail;
- a falsely passed aggregate outcome fails;
- result seed/action/lane mismatches fail against the scenario.

- [x] **Step 2: Verify RED**

```sh
cargo test -p mc-test-harness --lib replay -- --nocapture
```

Expected: compilation fails because the contract types and parsers do not yet
exist.

### Task 2: Minimal Strict Schema

**Files:**
- Modify: `crates/mc-test-harness/Cargo.toml`
- Modify: `crates/mc-test-harness/src/replay.rs`
- Modify: `crates/mc-test-harness/src/parity.rs`

- [x] **Step 1: Implement DTOs and validation**

Add strict serde DTOs, exact schema constants, identifier/path/fingerprint
validation, aggregate-outcome derivation, and result-to-manifest validation.
Reuse `CoreAction`, `ObservationFact`, and `ObservationSet`; serialize them with
stable tagged representations rather than introducing parallel fact types.

- [x] **Step 2: Verify GREEN**

```sh
cargo fmt --all -- --check
cargo test -p mc-test-harness --lib replay -- --nocapture
cargo test -p mc-test-harness --lib parity -- --nocapture
```

### Task 3: Checked Replay Fixture

**Files:**
- Create: `tools/core-replay-scenarios/core-actions-seed-81.json`
- Test: `crates/mc-test-harness/src/replay.rs`

- [x] **Step 1: Add fixture coverage**

Check in one small protocol/oracle/real-client-capable core-action manifest with
no local artifacts. Parse it through the public API and assert seed, ordered
actions, lanes, invariants, and required gates.

- [x] **Step 2: Verify fixture and fail-closed mutations**

The test must mutate schema version and remove one required field, proving both
shapes fail rather than defaulting.

### Task 4: Evidence And Full Gates

**Files:**
- Modify: `docs/milestones/M79.md`
- Modify: `docs/milestones/M96.md`
- Modify: `docs/VALIDATION_LEDGER.md`

- [x] **Step 1: Record exact scope**

Document schema capability and the absence of runner, oracle, client, load,
profile, or soak evidence. Do not promote Q1, Q2, Q3, O1, O2, or S2.

- [x] **Step 2: Run full validation and review**

```sh
cargo fmt --all -- --check
cargo run -p xtask -- code-health
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

Inspect all scoped source, fixture, Cargo, and evidence changes without staging
unrelated dirty-worktree files.
