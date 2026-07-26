# Spark Copy-Paste Prompts

## Coordinator → worker

```text
Owner authorizes this bounded Solaris task under the repository's two-worker cap.
Work only in the supplied worktree.

TASK: <TASK_ID>
BASE_SHA: <sha>
CARD: docs/spark-team/tasks/<TASK_ID>.md
OWNED_WRITE_PATHS: <copy exact paths from card>
ACTIVE_LOCKS: <copy from card>
RUNTIME_LEASES: <copy from card>
PORT_RANGE / RUN_DIR: <assigned unique values>

Before work, confirm Dispatch is READY and every dependency is integrated DONE.
Read AGENTS.md once, then the card. Do not read the full roadmap, ledger, ACTIVE
log, ALL_IN_ONE.md or parent conversation. Never cat a file over 400 lines: one
rg anchor batch, at most three 160-line windows. Update the card status/checks.
Do not edit outside owned paths. One RED, one edit batch, focused validation,
self-review, compact YAML closeout. Timeout only fails. No sleeps/polling. If
scope grows or the root cause needs another lock, stop partial and name exactly
one child task.
```

## Coordinator → evidence-only worker

```text
Evidence/audit only. Do not change production code. Use TASK <TASK_ID> and write
only its declared report/artifact paths. Separate unit, wire, oracle, real-client,
performance, concurrency and persistence evidence. An old doc claim is not current
evidence. Return the smallest missing leg, not a broad plan.
```

## Coordinator → reviewer

```text
Read-only review. Do not edit or spawn agents.

TASK: <TASK_ID>
ACCEPTANCE: <exact outcome + done_when>
BASE: <sha>
DIFF: <path-limited diff or commit>
VALIDATION: <compact command/results>

Check correctness, authority/CAS/publication ordering, scope, dominant failure
boundary, fake parity, pass-by-construction, sleeps/polling, path ownership and
stale claims. Return YAML only:
verdict: pass | changes | blocked
findings: [maximum 8 concise items]
validation_gaps: [...]
```

## Worker → coordinator closeout

```yaml
verdict: pass | changes | blocked
status: complete | partial | checkpoint-blocked
base_tree: ...
diff_hash: ...
changed_files: [...]
validation:
  - command: ...
    result: pass | fail | degraded | skipped
    log: ...
evidence:
  - requirement: ...
    proof: ...
known_gaps: [...]
next: ...
```

## Create one child card

```text
Create one Spark card from this blocker: one observable failure, one root
authority, max 3 production files, one success test, one dominant rejection/race
test, and one focused real-client/oracle/perf gate where relevant. Declare exact
paths, locks, leases and dependencies. Do not widen the parent card.
```

## Batch close audit

```text
Audit only this batch's done_when against the integrated tree. Do not audit the
whole north star. List complete/partial/blocked tasks, exact evidence, skipped
gates, reviewer verdict, validation-cache identity and the next compatible pair.
```
