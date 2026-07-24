# Spark Team Flow

## 1. Campaign override

Owner explicitly authorizes a bounded multi-agent core campaign. Repository cap still applies: **maximum two concurrent workers**, disjoint responsibilities/write sets, then one independent read-only reviewer per integrated candidate.

Campaign priority: **ordinary playability → scoped parity evidence → measured performance → only measured architecture cutovers**. Plugin/Loader breadth pauses unless it blocks those gates.

## 2. Roles

### Coordinator

- owns `BOARD.md`, DAG, path locks, runtime leases, base SHA, integration and validation cache;
- gives a worker only `AGENTS.md`, one task card, exact worktree/base SHA/ports/run dir;
- never gives Spark the parent conversation, whole roadmap, full ledger or archive;
- materializes every `TEMPLATE` card before claim;
- turns every material blocker into one child card instead of widening the active task.

### Worker A / Worker B

- one card, one worktree, one branch;
- edit only `Owned write paths`; extra path requires coordinator handoff;
- one bounded discovery batch, one edit batch, one focused validation batch, one closeout;
- update status and checkboxes in the task card as work advances.

### Reviewer

- sequential, read-only, no subagents, no edits;
- receives acceptance contract, path-limited diff and validation summary only;
- returns `pass | changes | blocked`, maximum eight findings;
- implementer fixes concrete findings; no reviewer carousel.

## 3. Status protocol

```md
Status: `QUEUED | CLAIMED | IMPLEMENTING | TESTING | REVIEW | DONE | BLOCKED`

- [ ] CLAIMED
- [ ] BASELINE / RED
- [ ] IMPLEMENTED
- [ ] TESTING
- [ ] SELF-REVIEW
- [ ] INDEPENDENT REVIEW
- [ ] DONE
```

- Claim: fill agent/worktree/base SHA/start time, set `CLAIMED`, tick `CLAIMED`.
- Implementation begins only after a reproducible RED/gap; set `IMPLEMENTING` and tick `BASELINE / RED`.
- Testing: set `TESTING`, append exact command, result and log path; tick only after command actually ran.
- Review: set `REVIEW` only after self-review and compact closeout exist.
- Blocked: set `BLOCKED`, record one blocker fingerprint, proof and exact next command. **Blocked never satisfies a dependency.**
- Done: all boxes checked, integrated diff/commit/evidence present; coordinator then checks `BOARD.md`.

## 4. Worktree/Git protocol

```sh
git worktree add ../solaris-worktrees/<TASK_ID> -b agent/<TASK_ID> <BASE_SHA>
cd ../solaris-worktrees/<TASK_ID>
```

- Never `reset`, `clean`, rewrite or stage owner/unrelated files.
- Never stage `.analysis/`, `data/vanilla/`, run dirs, Mojang bytes, local logs or secrets unless the card explicitly owns a sanitized report.
- One coherent Conventional Commit only when authorized.
- Without commit authorization, return base tree, diff hash, changed files and one next action.

## 5. Spark context budget

A worker loads:

1. `AGENTS.md` once;
2. its task card;
3. at most one small route/ADR slice named by the card;
4. normally no more than four production/test files before editing.

Hard limits:

- one behavior or one evidence leg;
- max 3 production files + 2 test files + 1 owning doc;
- 8 soft / 12 hard model roundtrips;
- 6 shell batches;
- no context compaction;
- no full repo survey, milestone range, archive sweep or `ALL_IN_ONE.md`.

### Source paging protocol

For a file over 400 lines:

```sh
rg -n 'anchor_one|anchor_two|error_code' exact/path.rs
sed -n '<line-80>,<line+160>p' exact/path.rs
```

Open at most three windows of at most 160 lines each. Never `cat` `play.rs`, `simulation.rs`, `server.rs`, `regional.rs` or large generated data. Use `rg --files` only inside the card’s declared directories.

When limits are reached, close `partial` and create a child card. Do not compress several failures into one Spark session.

## 6. Source-of-truth precedence

1. current source + focused tests + current runtime artifact;
2. `docs/MEMORY.md` and route memory;
3. exact anchored section of `docs/playable/ACTIVE.md`;
4. current review/WAL exact finding;
5. old restart checkpoints, milestone prose and validation statuses.

An old unchecked box is not work until current code/evidence confirms the gap.

## 7. Write locks

Two cards may run together only when write-lock sets are disjoint and the special rules below pass.

| Lock | Paths / meaning |
|---|---|
| `COORD` | board, DAG, global routing |
| `COORD-DOCS` | canonical memory/ledger/milestone docs |
| `VALIDATION` | broad workspace/Gradle gates; tree frozen |
| `RUST-NET-ROOT` | `play.rs`, `simulation.rs`, `server.rs`, root orchestration |
| `RUST-NET-BLOCKS` | placement, fluids, plants, toggles, scheduled blocks |
| `RUST-NET-CONTAINERS` | inventory, recipes, crafting/chest/furnace/stonecutter |
| `RUST-NET-SESSION` | session/player/entity/outbound/publication authority |
| `RUST-NET-CHUNK` | chunk stream/pipeline/view authority |
| `RUST-ENTITY` | entity runtime/regional/AI/combat/projectiles |
| `RUST-WORLD` | resident/storage/Anvil/light/dirty flush |
| `RUST-DATA` | vanilla data, loot, recipes, block/item facts |
| `RUST-HARNESS` | replay/load/real-client Rust harness |
| `CLIENT-JAVA` | Java client-agent scenarios/tools |
| `RUNNER` | Python/shell real-client orchestration |
| `ORACLE` | vanilla capture/manifests/comparison |
| `PERF` | long workload/results |
| `EXTERNAL` | paid client/external service qualification |

Special rules:

- `RUST-NET-ROOT` is exclusive with every other Rust runtime edit.
- `VALIDATION` is always singleton.
- A task with `PERF` is singleton unless it is a pure report task explicitly approved by the coordinator.
- Exact path overlap overrides lock names: overlapping writes never run together.

## 8. Runtime leases

Path-disjoint tasks can still corrupt each other’s evidence. Coordinator assigns leases:

| Lease | Rule |
|---|---|
| `CLIENT-RIG` | one actual Minecraft/client-agent run at a time |
| `ORACLE-RIG` | one vanilla capture/oracle process at a time |
| `CLEAN-HOST` | exclusive host; no compilation, IDE indexing or other workload |
| `TREE-FROZEN` | no source integration until the task closes |
| `PAID-AUTH` | owner-controlled credentials/network; secrets never enter Git/log artifacts |

Every worker also gets a unique port range, world dir, run dir and bridge secret. Shared default ports or shared `.analysis/test-world` are forbidden under parallel execution.

## 9. DAG and dispatch

- `READY`: claim only when all dependencies are integrated `DONE`.
- `TEMPLATE`: do not claim. Replace placeholders/prose paths with exact paths, one RED command, one rerun command, then validate.
- `COORDINATOR-ONLY`: umbrella/audit/release control; discovered coding work becomes child cards.

Static `BOARD.md` is a valid baseline, not an excuse to ignore live status:

```sh
python3 docs/spark-team/scripts/board.py validate
python3 docs/spark-team/scripts/board.py ready
python3 docs/spark-team/scripts/board.py summary
```

After any blocker, child task, changed dependency or cancelled hotspot, regenerate/revalidate before launch.

## 10. Validation cache

Identity: `(command, tree fingerprint, environment, covered scope)`.

- L0: exact focused tests + targeted diff/syntax.
- L1: affected crate/package tests, formatter, `cargo run -p xtask -- code-health` as required.
- L2 once per wave/commit candidate:

```sh
cargo run -p xtask -- code-health
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Never rerun an unchanged successful gate. After failure, rerun only the failed gate after a relevant edit. Long stdout goes to `.analysis/codex-logs/`; card stores status, short tail and path.

## 11. Evidence vocabulary

Never collapse these into “green”:

- `unit` — local contract only;
- `wire` — Solaris protocol/runtime path;
- `oracle` — independent vanilla capture/decompile/side-by-side comparison;
- `real-client-agent` — completed actual 26.1.2 client run;
- `owner-manual` — subjective/visual owner run;
- `performance` — reproducible workload with provenance and metrics;
- `concurrency` — exact race/pressure/ownership proof;
- `persistence` — disk/restart/crash-window proof.

A parity row needs runtime evidence plus an independent oracle or real-client leg. A faster unit benchmark is not server performance.

## 12. Worker closeout

```yaml
verdict: pass | changes | blocked
status: complete | partial | checkpoint-blocked
base_tree: <sha>
diff_hash: <sha256>
changed_files: [exact list]
validation:
  - command: ...
    result: pass | fail | degraded | skipped
    log: ...
evidence:
  - requirement: ...
    proof: ...
known_gaps: [max 8]
next: <one exact action>
```

Inline result under 1,000 characters; details live in the task card/report.

## 13. Reviewer gate

Reviewer checks only:

- acceptance requirements actually proven;
- duplicate authority / stale CAS / publication ordering;
- broader-than-needed abstraction/config/compat layer;
- missing dominant failure boundary;
- fake parity (expected facts copied from implementation);
- pass-by-construction, sleeps, polling or timeout-as-success;
- changed paths outside ownership;
- stale docs/claims.

## 14. Merge cadence

1. Freeze base SHA; issue at most two compatible cards.
2. Workers finish L1, self-review and closeout.
3. Coordinator inspects path-limited diffs and integrates.
4. One read-only reviewer checks the integrated candidate.
5. Original implementer handles concrete findings.
6. Coordinator runs only missing integration gates.
7. At wave boundary run one L2, update canonical status once, then open the next batch.

## 15. Stop rules

Stop when acceptance is met, context/tool budget is reached, artifact/service is absent, a needed lock is owned elsewhere, or discovery reveals another root cause. Record exact proof and create one new card. Never continue “helpfully” into another subsystem.
