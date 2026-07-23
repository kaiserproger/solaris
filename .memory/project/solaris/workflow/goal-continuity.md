# Goal Continuity Workflow

Use this note after compaction, interruption, retry, or a long unattended run.

1. Re-read the active request, any newer queued request, and
   `git status --short --branch`. A newer request replaces active work only when
   the owner explicitly interrupts or replaces it.
2. Read the Autonomous Goal Protocol in `AGENTS.md`, then `docs/MEMORY.md` for
   current evidence and delivery priority.
3. Inspect the exact diff before assuming which changes belong to the agent.
4. Resume the active request. Within that request, choose its highest-priority
   unfinished slice; do not switch to unrelated older work because its files
   are already open.
5. Stop active processes safely on an explicit interruption. On `Retry` or
   `продолжай`, verify process, worktree, and external state before resuming.
6. Resume the checkpoint phase and validation tier from its cursor. Do not
   repeat successful gates for an unchanged working-tree fingerprint.
7. Self-review, then request exactly one independent read-only second opinion.
8. Commit only owned files when the owner has authorized commits.

Keep one checkpoint within 8 soft and 12 hard model roundtrips, six shell
batches, one stateless subagent, one L2 run, and zero compactions. Close or
snapshot before crossing those limits, then start a fresh session from a
compact cursor. Never fork the long parent conversation into a subagent: pass
only its bounded task, base commit, owned paths, acceptance checks, and relevant
evidence. Close completed agents immediately.

One-tool-per-model-round is a failure mode when calls are independent. Batch
bounded discovery, edits, and focused validation. Do not run L2 until the tree
is a commit candidate. Launch long work once and consume one completion event;
do not create model rounds from progress polling.

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
