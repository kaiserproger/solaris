# Goal Continuity Workflow

Use this note after compaction, interruption, retry, or a long unattended run.

1. Re-read the active request, any newer queued request, and
   `git status --short --branch`. A newer request replaces active work only when
   the owner explicitly interrupts or replaces it.
2. Read `docs/MEMORY.md` for the current checkpoint and delivery priority.
3. Inspect the exact diff before assuming which changes belong to the agent.
4. Resume the active request. Within that request, choose its highest-priority
   unfinished slice; do not switch to unrelated older work because its files
   are already open.
5. Stop active processes safely on an explicit interruption. On `Retry` or
   `продолжай`, verify process, worktree, and external state before resuming.
6. Keep progress in coherent feature checkpoints. Use narrow checks while
   editing; run the full workspace baseline once at the end of code work.
7. Self-review, then request exactly one independent read-only second opinion.
8. Commit only owned files when the owner has authorized commits.

Before a planned reboot, stop active processes and agents, classify every
active slice as committed, complete-but-unverified, or partial, and write an
ignored continuation checkpoint with the exact next command. After restart,
revalidate Git, processes, artifacts, and external state before resuming.

The binding delivery order lives in `docs/MEMORY.md`: common gameplay and
multiplayer, production Lua API, measured optimization/regions/ECS/autoscale,
then rare hardening. Update that canonical record when the owner changes it.

The worktree is often dirty with owner files and ignored runtime artifacts.
Never clean, reset, stage, or rewrite them. Remove inactive worktrees only after
checking for unique source changes; generated build artifacts may be removed.

Treat owner-written review files and WAL-style issue queues as concurrent
external state. Reread before selecting or closing an item, inspect the tail
again afterward, and never overwrite concurrent appends from a cached copy.

Codex logs are known under `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
Inspect `session_meta.payload.cwd` first and select at most three matching
sessions for rule extraction. Parse large logs line by line with a tolerant
form such as `jq -R 'fromjson?'`, count and report malformed records, and never
dump the full file into context. Store user-requested exports under ignored
`.analysis/`, scan before sharing, and never copy secrets or transient output
into repo memory.
