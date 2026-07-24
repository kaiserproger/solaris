# Solaris Spark Autopilot

Operating contract for one persistent `/goal`. The primary thread is a **dispatcher**, not a code explorer. The Python control plane reads the 74-card DAG so the model does not have to.

Rely on the automatically loaded root `AGENTS.md`. Read this file and `OWNER_AUTHORIZATION.md` once. Afterwards consume only compact command JSON, generated packets, compact agent YAML, and path-limited diffs. Never open full `BOARD.md`, `ALL_IN_ONE.md`, the whole manifest, old sessions, or broad source trees in the primary thread.

## Completion contract

Complete only when one integrated campaign HEAD proves all of the following and `T10-04` is `DONE`:

- **PLAYABLE-GREEN** — current no-debug real-client single-player and two-player survival/restart arcs pass without crash, disconnect, duplication, invisible loss, ghost state, or catastrophic stall.
- **PARITY-GREEN** — every counted row has current runtime proof and its required independent vanilla-oracle or real-client leg; M100 is recalculated from current evidence rather than prose.
- **PERF-GREEN** — frozen low/balanced/high profiles have clean provenance and required percentiles/queue/lock/memory/save/outbound data; balanced performance passes without gameplay regression.
- Final exact-tree L2 is green.

A green unit suite, empty normal ready list, one batch, or stale readiness document is not completion.

## Bootstrap once

From the owner checkout:

```sh
python3 docs/spark-team/scripts/board.py validate
python3 docs/spark-team/scripts/autopilot.py doctor
python3 docs/spark-team/scripts/autopilot.py bootstrap
```

Parse `campaign_root` from JSON and `cd` there. `bootstrap` creates a sibling `agent/spark-campaign-*` worktree from owner `HEAD`, copies/commits only the control plane, records owner dirty paths in durable shared-Git state, and leaves the owner checkout untouched. If it reports `already-bootstrapped`, recover that campaign instead of creating another.

Dirty owner paths are write conflicts for every card except the initial tree audit. Never mirror, reset, clean, stash, or overwrite them automatically.

## Autonomous state machine

Repeat until the completion or hard-blocked contract is met.

### 1. Dispatch

```sh
python3 docs/spark-team/scripts/autopilot.py dispatch --limit 2 --json
```

Interpret `status` literally:

| Status | Required action |
|---|---|
| `dispatched` | Spawn every returned custom `role` concurrently, each with **only** its packet. Wait for all returned workers before acceptance. |
| `coordinator-dispatched` | Primary executes the one returned packet itself. It may delegate only bounded read-only evidence slices. |
| `template-required` | Run the template flow below, then dispatch again. |
| `no-ready-work` | Process active candidates/blockers; audit stop conditions. Do not declare success from this status alone. |

The dispatcher already enforces dependencies, P0/P1 ordering, exact write overlap, locks, leases, owner dirty paths, exclusive validation/performance/tree/host work, and the two-subagent cap. Do not second-guess it by reading the backlog.

Every packet contains the finite `<goal_checkpoint>` required by root instructions. Packet `route` is the only route authority.

### 2. Worker return

For each returned task:

```sh
python3 docs/spark-team/scripts/autopilot.py candidate --task TASK_ID
```

A valid successful worker leaves `REVIEW`, all dispatch/live/Done-when checks through `SELF-REVIEW`, exact validation/evidence, inline-list closeout fields, and one or more local commits. It never checks independent review or DONE.

A hard blocker leaves `BLOCKED`, one stable fingerprint, proof/artifact, exact unlock action, one next boundary, and a useful local commit. `BLOCKED` never satisfies dependencies.

On candidate errors, send only that error list to the same worker for one bounded repair, then rerun `candidate`. Do not broaden the card.

### 3. Independent review

For a valid `REVIEW` candidate, spawn one fresh `solaris_reviewer` with only `reviewer_packet`. Reviewer is read-only and uses no subagents.

Record its verdict:

```sh
python3 docs/spark-team/scripts/autopilot.py review --task TASK_ID --verdict pass    --summary '<compact YAML/findings>'
python3 docs/spark-team/scripts/autopilot.py review --task TASK_ID --verdict changes --summary '<concrete findings>'
python3 docs/spark-team/scripts/autopilot.py review --task TASK_ID --verdict blocked --summary '<fingerprint and unlock action>'
```

`changes` returns only concrete findings to the original worker for one fix pass. Rerun `candidate`, then one direct verification; no reviewer carousel. A changed candidate must be reissued before any pass is recorded.

### 4. Integrate or checkpoint

Passing candidate:

```sh
python3 docs/spark-team/scripts/autopilot.py integrate --task TASK_ID
```

Worker-reported/reviewer-blocked checkpoint:

```sh
python3 docs/spark-team/scripts/autopilot.py checkpoint --task TASK_ID
```

`integrate` rechecks candidate identity and ownership, applies its commits without touching owner files, marks card/board, validates the DAG, creates one campaign commit, archives ignored evidence, and removes the task worktree. Integrate pair members sequentially. A conflict aborts safely; redispatch that card from the new campaign HEAD rather than resolving blindly.

### 5. Persist and continue

```sh
python3 docs/spark-team/scripts/autopilot.py dashboard --write-md
python3 docs/spark-team/scripts/autopilot.py event --kind checkpoint --message '<IDs; proof paths; blockers; next action>'
python3 docs/spark-team/scripts/autopilot.py dispatch --limit 2 --json
```

Do not end the turn after reporting progress. Dispatch again immediately.

## Template flow

When `template-required` lists a card:

```sh
python3 docs/spark-team/scripts/autopilot.py template-packet --task TASK_ID
```

Spawn one `solaris_explorer` with only that packet. Convert its measured facts to the exact JSON schema in the packet, then:

```sh
python3 docs/spark-team/scripts/autopilot.py materialize --task TASK_ID --spec /absolute/path/spec.json
```

The helper rejects placeholders/broad fake paths, updates only the card and manifest entry, validates and commits. Never materialize a speculative rewrite.

## External evidence

`CLIENT-RIG`, `ORACLE-RIG`, `CLEAN-HOST`, `TREE-FROZEN`, and `PAID-AUTH` are exclusive.

Launch long jobs once, consume completion once, and never poll. Timeout is failure. Keep full output in ignored files and return only status, failures, short tail, provenance, and artifact path. Never print/commit credentials or Mojang bytes. If one external leg is unavailable, block only that leg and finish all independent local cards first.

## Stop conditions

**Complete:** a separate final audit proves all three green gates on one HEAD, `T10-04` is DONE, L2 is green, and closeout names current artifacts plus non-blocking debt.

**Blocked:** no ready card, no template, no coordinator checkpoint, no active repairable worktree, every remaining chain ends at an external/irreducible blocker, and state records fingerprint, evidence, unlock command, and next unlocked card.

Ask the owner only for destructive permission, credential/paid-account access, unavailable external rig, or genuinely subjective manual judgment.

## Context ceiling

Per batch the primary may consume one dispatch JSON, at most two worker YAMLs, two candidate reports/path-limited diffs, two reviewer YAMLs, and one event. Raw code exploration and logs stay in worker threads/files. Durable state, not chat history, is the continuation source after compaction/restart.
