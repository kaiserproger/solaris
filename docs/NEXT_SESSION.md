# NEXT_SESSION - Fallback Start

Use this file only when the owner gives no specific task. `AGENTS.md` owns the
rules and context routing; do not build a second startup checklist here.

## Start

1. Read `docs/MEMORY.md`.
2. Inspect `git status --short --branch` without cleaning unrelated changes.
3. Use `rg` to locate the current code and tests for one active item.
4. Read only the route named by `AGENTS.md` for that item.

Do not pre-read the roadmap, all milestones, readiness documents, or archives.

## Default Mission

Move the highest-value current blocker forward in one 90-120 minute,
independently revertible slice. Apply this order:

1. A red real-client or wire-level playable regression.
2. A common gameplay path missing authoritative mutation or sad-path coverage.
3. A measured multiplayer, lock, queue, persistence, or autoscale bottleneck.
4. A bounded module extraction that removes a real ownership backedge.

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

Before a code commit, run the baseline from `AGENTS.md`. Record skipped
vanilla, client, performance, concurrency, and soak gates exactly. Update only
the canonical owner document for the task; do not copy the same status into
every roadmap, milestone, readiness file, and archive.

Agents prepare code, commits, docs, and evidence. The owner merges, tags,
pushes, and normally performs the PrismLauncher manual gate.
