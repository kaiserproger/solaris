# NEXT_SESSION - Fallback Start

Use this file only when the owner gives no specific task. `AGENTS.md` owns the
rules and context routing; do not build a second startup checklist here.

## Start

1. Read `.memory/MEMORY.md`, then `docs/MEMORY.md`.
2. Inspect `git status --short --branch` without cleaning unrelated changes.
3. Declare one explicit checkpoint route and outcome from the current queue.
4. Read only the route named by `AGENTS.md` for that item.

Do not pre-read the roadmap, all milestones, readiness documents, or archives.

## Default Mission

The second-alpha plan is closed. Use [`PUBLIC_ALPHA3_PLAN.md`](PUBLIC_ALPHA3_PLAN.md) as the current carry-over queue unless the owner gives a newer explicit task. Move the highest-value current blocker through one finite checkpoint using the protocol and budgets in `AGENTS.md`. Apply this order:

1. A red real-client or wire-level playable regression.
2. A common gameplay path missing authoritative mutation or sad-path coverage.
3. A production Lua plugin API or gameplay-adapter slice after the common
   playable loop is green.
4. A measured multiplayer, lock, queue, persistence, regional, ECS, or
   autoscale bottleneck.
5. A bounded module extraction that removes a real ownership backedge.
6. Rare error-path hardening or uncommon parity edges.

Do not continue a lower item merely because it was open before compaction.
Record its remaining non-blocking debt, then resume the highest unfinished
item. In particular, do not return to deferred save `fsync` interleavings while
common vanilla gameplay or production plugin API work remains.

Prefer a complete vertical path:

```text
input or simulation event
-> validation and authoritative state read
-> one fenced mutation
-> follow-up scheduling/publication
-> exact client-visible result or rejection
```

Do not claim improvement from helper-only tests. Add a real-path test for the
touched behavior and its reachable failure branches.

## Checkpoint

Before a code commit, run the selected validation tier from `AGENTS.md`. Record skipped
vanilla, client, performance, concurrency, and soak gates exactly. These records are
checkpoint summaries, not per-command progress reports. Update only
the canonical owner document for the task; do not copy the same status into
every roadmap, milestone, readiness file, and archive.

Agents prepare code, commits, docs, and evidence. The owner merges, tags,
pushes, and normally performs the PrismLauncher manual gate.
