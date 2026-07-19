# Prompt 00 Baseline And Evidence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore an honest green development baseline for the current
`dev/M100-client-agent` worktree and reconcile its playable-client evidence
without broadening gameplay scope.

**Architecture:** Keep the current playable worldgen contract: embedded
production data must contain iron ore because the seed-0 no-debug progression
uses a generated iron outcrop. Correct the stale reduced-registry test fixture,
then treat Rust, Gradle, artifact validation, and evidence accounting as
separate gates so a failure identifies one boundary.

**Tech Stack:** Rust 1.94, Cargo workspace tests, Gradle client-agent tests,
repo-owned real-client validator, Markdown evidence docs.

## Global Constraints

- Follow `AGENTS.md` and the Shared Execution Contract in
  `docs/CORE_ITERATIVE_GOAL_PROMPTS.md`.
- Preserve all unrelated dirty-worktree changes and local-only artifacts.
- Do not push, merge, tag, or stage unrelated paths.
- Do not add gameplay breadth, ECS, SIMD, or autoscale behavior.
- Do not promote a ledger row without focused runtime evidence plus an exact
  vanilla-oracle or real-client evidence leg.

---

### Task 1: Required Worldgen Resource Contract

**Files:**
- Modify: `crates/mc-worldgen/src/terrain.rs`
- Test: `crates/mc-worldgen/src/terrain.rs`

**Interfaces:**
- Consumes: `TerrainGenerator::try_with_rules` and its documented required
  terrain block set.
- Produces: a reduced test registry containing every required block, including
  `minecraft:iron_ore`, while optional terrain blocks remain absent.

- [x] **Step 1: Strengthen the failing test**

Rename the optional-fallback test so its precondition is explicit. Resolve
`minecraft:iron_ore` from the reduced registry before constructing the
generator and assert that `generator.iron_ore` preserves that state.

- [x] **Step 2: Verify RED**

Run:

```sh
cargo test -p mc-worldgen --lib terrain::tests::try_with_rules_allows_missing_optional_blocks_when_required_resources_exist -- --exact --nocapture
```

Expected: FAIL because `required_only_registry()` does not contain
`minecraft:iron_ore`.

- [x] **Step 3: Fix the fixture**

Add `minecraft:iron_ore` to the required block names used by
`registry_without_block`; make no production behavior change.

- [x] **Step 4: Verify GREEN and adjacent worldgen behavior**

Run:

```sh
cargo test -p mc-worldgen --lib terrain::tests::try_with_rules_allows_missing_optional_blocks_when_required_resources_exist -- --exact --nocapture
cargo test -p mc-worldgen --lib
cargo test -p mc-worldgen --test core_rules default_seed_spawn_window_contains_basic_playable_resources -- --exact --nocapture
```

Expected: all executed tests pass; the debug throughput probe remains ignored
in the library suite.

### Task 2: Rust Baseline

**Files:**
- Modify only files implicated by independently reproduced failures.

**Interfaces:**
- Consumes: the Cargo baseline from `AGENTS.md`.
- Produces: exact pass/fail evidence after the final Rust edit.

- [x] **Step 1: Run formatting and code-health gates**

```sh
cargo fmt --all -- --check
cargo run -p xtask -- code-health
```

- [x] **Step 2: Run the full test baseline**

```sh
cargo test --workspace
```

- [x] **Step 3: Run the full lint baseline**

```sh
cargo clippy --workspace --all-targets -- -D warnings
```

For each failure, reproduce it focused and use a separate RED/GREEN cycle.
Do not bundle unrelated repairs.

### Task 3: Client-Agent And Artifact Gates

**Files:**
- Modify client-agent or runner files only for a reproduced gate failure.

**Interfaces:**
- Consumes: repo-owned Gradle runClient adapter and existing P4/P42 artifacts.
- Produces: current test and validation evidence without launching an
  environment-supplied client command.

- [x] **Step 1: Run client-agent tests**

From `client-mod/solaris-client-agent` run:

```sh
./gradlew test
```

- [x] **Step 2: Run runner contract tests**

```sh
cargo test -p mc-test-harness --test real_client_manifest -- --nocapture
```

- [x] **Step 3: Validate current playable artifacts**

```sh
bash tools/run-real-client-regression.sh --validate-run .analysis/real-client-runs/20260706T115222Z-real-client-playable-loop
bash tools/run-real-client-regression.sh --validate-run .analysis/real-client-runs/20260706T122802Z-real-client-playable-loop
```

The earlier cold P42 diagnostic artifact must continue to fail validation.
A fresh GUI client run is required only if current validation or touched
client-visible behavior makes the recorded artifacts stale.

### Task 4: Evidence Reconciliation And Review

**Files:**
- Modify: `docs/VALIDATION_LEDGER.md` only for exact row evidence changes.
- Modify: `docs/playable/ACTIVE.md` only for current playable checkpoint facts.
- Modify: `docs/milestones/M100.md` only when the canonical M100 claim changes.

**Interfaces:**
- Consumes: verified commands and artifacts from Tasks 1-3.
- Produces: honest evidence text and a reviewed final diff.

- [x] **Step 1: Re-run conservative coverage audit**

```sh
cargo run -p mc-test-harness --bin coverage-audit -- docs/VALIDATION_LEDGER.md
```

- [x] **Step 2: Reconcile only exact evidence**

Do not count artifact validators as gameplay evidence. Do not promote broad
M94 rows from focused P4/P42 playable scenarios. Record any newly current gate
with exact command, artifact, scope, and limitations.

- [x] **Step 3: Review scope and whitespace**

```sh
git diff --check
git status --short --branch
```

Inspect every touched diff. Do not commit while unrelated changes in the same
files cannot be isolated safely.
