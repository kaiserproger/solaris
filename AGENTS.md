# Agent Guide - Solaris

Solaris is a custom Minecraft Java Edition 26.1-compatible server written in
Rust. `CLAUDE.md` is a symlink to this file.

Owner/local git identity: `kaiserproger <kaisergrobe@gmail.com>`. Do not change
git config.

## Owner Contract

"Do not overthink/overengineer every possible case until you explain exactly WHY it is needed and user agrees with you. Write as simple and clear as possible. Always ask user first what they fucking want."

"Always double-check your work. Spawn another agent to give clear, concise second POV on your work. Do not do anything that isn't needed right now as it probably won't ever be needed. Verification and reporting happen at meaningful checkpoints, not after every command, tiny fix, or intermediate discovery."

"On receiving new user request, first finish what you've started before taking new user request UNTIL it is explicitly required by user to take right now. Continue execution through a meaningful checkpoint; do not stop only to report intermediate progress."

Operational meaning:

- The explicit request is the source of truth. An unambiguous request does not
  need reconfirmation. Ask only when a material unresolved choice changes the
  result.
- Implement the smallest direct local change that completes the request. No
  speculative abstraction, extensibility, configuration, unrelated refactor,
  formatting churn, or impossible-case hardening.
- Queue a newer request until active work closes unless the owner explicitly
  interrupts or replaces it. `Stop`, `стоп`, `забей`, `поправка`, or an
  explicit replacement switches now after safe process/worktree inspection.
- Status updates are checkpoint-level summaries. Do not interrupt implementation
  flow for routine commands, passing tests, formatting fixes, or small discoveries.
  `Retry` or `продолжай` first revalidates current state.
- Self-check, then use exactly one independent read-only reviewer. Larger agent
  pipelines require explicit owner authorization.

## Autonomous Goal Protocol

Execution should continue until a meaningful checkpoint or blocker. Intermediate command output is not a user-facing milestone.

The persistent `/goal` objective is a north star, not the unit of work. Each
continuation executes exactly one finite checkpoint supplied in
`<goal_checkpoint>`. Completing a checkpoint does not complete or redefine the
north star.

### Checkpoint Granularity

A checkpoint must close one complete active-plan item or deliver one observable
gameplay, multiplayer, plugin, persistence, performance, or tooling capability
with its tests and documentation. Plan a bounded vertical slice before editing;
the checkpoint unit is the outcome, not an individual file or function.

Do not create or close a checkpoint solely for a single test extraction, file
move, documentation/status update, structural cleanup, or other mechanical
change. Batch all related mechanical work into the checkpoint that owns the
result. Run L2 validation, independent review, evidence updates, and the local
commit once at the end of that complete checkpoint, not once per constituent
edit.

A smaller checkpoint is allowed only when the owner explicitly requests it or a
concrete blocker prevents the planned outcome. Record the blocker and the
unfinished outcome; do not manufacture a micro-checkpoint to report motion.
`resume.next` and active queue documents must name an outcome and its acceptance
evidence, never an individual function, test, or file move.

Use `checkpoint.route` as the only routing authority for autonomous
continuations. The live cursor is `docs/MEMORY.md`; harness wiring lives in
`docs/AGENT_TOOLING.md`. Never select a route by matching words in the
persistent objective, quoted history, this file, a compaction summary, or a
subagent report. Read only the route document the checkpoint names; route
detail pages are references, not a second cursor.

At checkpoint start:

1. Read `resume.next`, `base_tree`, `changed_files`, and the one primary route
   document named by the checkpoint.
2. Inspect only checkpoint-relevant status/diff. Do not restart a repository
   survey after continuation or compaction.
3. Run at most one bounded discovery batch before editing. Additional discovery
   must answer one concrete unresolved question.

A checkpoint uses four rounds whenever dependencies allow:

1. bounded discovery batch;
2. edit/patch batch;
3. focused validation batch;
4. checkpoint close with evidence, snapshot, reviewer, and next cursor.

Batch independent tool calls. Do not narrate or execute one shell command per
model round when calls can run together. Never issue the same read, search, or
validation command twice for the same working-tree fingerprint.

Default checkpoint budget unless the wrapper supplies another value:

```yaml
model_roundtrips_soft: 8
model_roundtrips_hard: 12
shell_batches: 6
subagents: 1
l2_validation_runs: 1
context_compactions: 0
```

At the hard budget, stop expanding scope. Record evidence and a precise resume
cursor, close the checkpoint as `complete`, `partial`, or `checkpoint-blocked`,
and let runtime start a fresh continuation. These checkpoint states do not
complete or block the persistent `/goal`.

Do not carry a completed checkpoint into another compaction. Start the next
checkpoint in a fresh session with a compact cursor containing only the
request, route, base tree, owned changed files, evidence, and one next action.
If inherited history already spans multiple checkpoints, close or snapshot the
current checkpoint before more discovery.

## Discovery Contract

Choose one source-navigation path per question:

- Indexed symbols, callers/callees, mutation paths, or blast radius: one
  targeted CodeGraph call when available.
- Docs, configuration, logs, generated artifacts, or unindexed/stale files:
  bounded `rg`/direct read.

Do not run CodeGraph and broad `rg` merely to reconfirm the same answer. After
CodeGraph, raw read is for a specifically stale, missing, or edited file.

Bound output before execution:

- use `rg -l`, path filters, and `-m` limits;
- use `git diff --stat` or `--name-only` before targeted hunks;
- project JSON with `jq`; do not pretty-print large artifacts;
- write verbose test/client output to ignored `.analysis/codex-logs/` and return
  only status, failures, short tail, and log path;
- treat truncation as a failed query strategy and narrow the next query.

## Validation Tiers

Validation identity is `(command, tree fingerprint, environment, covered
scope)`. Never rerun an unchanged successful gate for the same identity.

The canonical entrypoint is `python3 -m tools.harness` from the repository
root. `python3 -m tools.harness list` enumerates the 23 named profiles;
`python3 -m tools.harness run PROFILE [--release] [--timeout-seconds N]
[--platform fabric|neoforge|forge]` runs one. Wiring lives in
`docs/AGENT_TOOLING.md`.

- **L0 - edit loop:** focused development tests for the affected scope (for
  example `cargo test -p <crate> <filter>`) plus a targeted diff/syntax
  check.
- **L1 - checkpoint close:** affected-scope focused tests plus `run fmt` and
  `run code-health`; `codegraph sync .` when the checkpoint relied on CodeGraph.
- **L2 - code commit/release/milestone close:** run once:

```sh
python3 -m tools.harness run correctness
```

`correctness` is the full L2 scope: formatter, `code-health`, strict
workspace Clippy, and `cargo test --workspace --all-targets`. The individual
gates are also available as `run fmt`, `run clippy`, `run code-health`, and
`run test`. Real-client and oracle gates run through their profiles
(`core-client`, `inventory`, `loader-live --platform ...`, `regression`,
`playable`, `replay`, `oracle`, `oracle-check`, `seed-review`,
`entity-scale`, `living-world-scale`, `fixture-check`, `installer`, `java`,
`build`, `harness-check`, `bucket-resync`); client launch/check uses
`python3 -m tools.harness client [--check] [--platform ...]` with client
credentials set.

Each run writes
`.analysis/validation/<UTC stamp>-<profile>-<unique>/result.json` with
`status` `passed` | `failed` | `prepared` | `interrupted` plus
`commands`/`exits`/`logs`/`details`/`error`. Unknown, unlaunchable, or
failing commands fail closed with a nonzero exit. `--check`/`--prepare`
(and other preparation-only modes) report `prepared`, never a gameplay
pass. Graphical runs need Linux `inotify`/`pidfd`/`Xvfb` plus MCP state
events; client launch/check requires credentials. Every timeout is failure,
never success. Whole-second `--timeout-seconds` rounding for
regression/playable/replay is a known Main-owned fix; do not work around it
here.

After failure, rerun only the failed gate and only after a relevant change.
Bug/perf fix loop: reproduce with the exact command/seed, write the pre-fix receipt under `.analysis/codex-logs/<slug>/`, localize, then fix, then re-run the same repro. `docs/MEMORY.md` links the receipt at close; raw facts stay in the receipt, never inline.
Markdown/instruction-only checkpoints use static/path/link/diff checks and
explicitly skip harness/Cargo gates. A broad green gate proves only its covered scope.
`code-health` is a fail-only architecture tripwire, not gameplay/parity/client/
performance evidence. Structural checks may enforce ownership/dependencies,
preferably through AST; never test behavior or statement order by comparing
Rust source text or line positions.

## Long-Running Work

Long-running work is event-driven. Launch once and continue independent work.
Do not poll with `wait`, `write_stdin`, process listings, repeated status, or
agent waits. Consume completion/actionable notification once. A timeout may
fail stuck work; elapsed time is never success.

Harness runs own their lifecycle: each `run` creates an isolated artifact
directory, drives readiness from process/log/client-state events, cleans up
its owned server/client/Xvfb processes, and records `result.json` with
`status`, `commands`, `exits`, `logs`, `details`, and `error`. Read the
receipt; `prepared` is preparation, not a pass, and `error` names the
fail-closed cause.

Never add wall-clock sleeps in production, tests, harnesses, or tools. Wait for
the exact channel message, notification, packet, process state, world state, or
simulation event. The producer must wake consumers; use push, not pull.

## Subagents

Use subagents only when owner/runtime authorizes delegation. At most two may run
concurrently, with disjoint responsibilities and write sets. Do not delegate
the immediate blocker and then wait for it. Prefer GPT-5.6 Sol for implementation
and GPT-5.6 Luna for QA/review; do not use `terra`.

For agent delegation, do not hand-roll tmux/pid/exit/log plumbing: validation
and client-visible QA run through the canonical harness, which owns
server/client/Xvfb lifecycle per run and records receipts as `result.json`
under `.analysis/validation/`. Await the harness result event; do not
replace it with status polling.

Never fork the full parent conversation into a subagent. Spawn it without
history and pass a bounded task, base commit, owned paths, and required
evidence. A reviewer needs the diff and acceptance contract, not the parent
transcript. Close completed agents immediately.

Client-visible QA is standardized. Follow the protocol in
`docs/QA_AGENT_PROTOCOL.md` through the harness real-client profiles
(`core-client`, `inventory`, `loader-live --platform ...`, `regression`,
`playable`, `replay`): run the exact requested scenario with a real
graphical Minecraft client under Xvfb, capture client/server/result evidence,
then perform one bounded adversarial exploratory pass and report severity-ranked
criticism. Raw-TCP/unit tests may supplement that gate but never substitute for it.

One agent returns one compact result per revision:

```yaml
verdict: pass | changes | blocked
findings: [maximum 8 bullets]
changed_files: [...]
validation: [...]
report_path: optional
```

Inline content is at most 1,000 characters. Details go to a file. Deduplicate
notification/result by `agent_id + revision`. Reviewers do not edit or spawn
agents; fixing findings does not trigger a second reviewer.

`quaka-whaka-zaka-du` explicitly authorizes parallel work, still capped at two
agents and subject to all correctness/validation rules.

## Process Skills

In autonomous `/goal` mode this workflow replaces generic process skills. Do
not load or announce brainstorming, TDD, debugging,
verification-before-completion, or similar generic skills unless the owner
explicitly invokes one. Tool-specific mandatory skills remain allowed.

If a higher-priority runtime/plugin instruction still mandates generic skill
reads, the runtime must disable that plugin for autonomous goal sessions;
`AGENTS.md` alone cannot override it. Obey the higher-priority instruction and
report the conflict rather than pretending it is disabled.

## Checkpoint State And Git

Update `docs/MEMORY.md`, an active queue, ADR, or milestone once at checkpoint
close, not after each micro-edit. One closed checkpoint produces one local,
revertible Conventional Commit when authorized. Never push, merge to `main`, or
tag unless explicitly instructed. Do not skip hooks/signing flags.

Workspace version is `0.0.5`; the release target is `v0.0.5`.
Its publication evidence belongs under `.analysis/releases/public-v0.0.5/`;
maturity remains draft and the GitHub release is preliminary. Previous
`v0.0.4-alpha.1` evidence stays under `.analysis/releases/public-v0.0.4-alpha.1/`.
The previous public `v0.0.3-alpha.1` evidence under
`.analysis/releases/public-v0.0.3-alpha.1/` belongs to tag commit `571f5383`.
The older local field-test archive under `.analysis/releases/v0.0.3-alpha.1/` is frozen
before repository separation; do not overwrite it with later source builds.
Its isolated startup passed, not full survival acceptance.

Without commit authorization, record:

```yaml
base_tree: <sha>
diff_hash: <sha256>
changed_files: [...]
validation: [...]
next: <one concrete action>
```

Use path-limited status/diff against that base. Preserve unrelated dirty files.
Never reset, clean, or stage them. Remove inactive worktrees only after checking
for unique source changes; discard only reproducible build/client artifacts.

Before a planned reboot, stop processes/agents, classify each active slice as
committed, complete-but-unverified, or partial, and write an ignored resume
cursor with one next action. Revalidate it after restart.

## Independent source repositories

- Core Rust/runtime and canonical harness: this repository.
- Loader/Java/client scenarios: sibling `../solaris-loader`; harness override
  `SOLARIS_LOADER_ROOT` is allowed. No duplicate `client-mod` source tree.
- First-party Luau packages: sibling `../solaris-default-plugins`; deploy chosen
  package directories through `[plugins].directory`. No compiled-in first-party
  package list or `plugins.bundled` compatibility path.
- Core production builds must not require either sibling. Integration gates
  explicitly require the repository whose real behavior they exercise.
- Sibling repositories own their README/AGENTS and Git metadata. Creating them
  locally does not authorize staging, commits, remotes, pushes or publication.

Owner field testing takes precedence over tuning a fixed seed. Follow it with
varied-seed graphical exploration, recording seeds and exact reproduction
evidence. A scenario blocked on terrain remains blocked; green Rust gates do
not establish survival acceptance.

## Solaris Priorities

Apply Pareto delivery in this owner-defined order:

1. common vanilla-client gameplay and multiplayer behavior;
2. production Luau plugin API and gameplay adapters;
3. measured optimization, regional ownership, ECS, and autoscaling;
4. rare error interleavings and uncommon parity edges.

Do not promote a lower item because its diff is open or review found a
non-blocking edge. Move ahead only when it blocks ordinary play, corrupts normal
saves, or prevents plugin progress.

Solaris has no released compatibility surface. Delete superseded Solaris APIs,
schemas, duplicate authorities, adapters, feature flags, and fallbacks once
callers move. Compatibility targets are vanilla protocol/behavior (with
deliberate bug fixes) and vanilla world format, not historical Solaris code.

Measured narrow optimization hacks are allowed. Document the bottleneck,
boundary, correctness fence, measured effect, and fallback/removal path. Never
reintroduce operator worker-thread percentages; derive capacity once and let
measurements/autoscaling move bounded budgets.

## Project Invariants

- Mojang bytes never enter Git. `.analysis/*` and `data/vanilla/*` stay ignored.
- Packet ids/layouts come from local `wire-probe`/`javap`, never memory.
- Gameplay parity needs vanilla capture, decompiled-source inspection, or
  side-by-side harness evidence.
- Debug builds are the development loop; do not use `--release` unless the
  checkpoint explicitly requires a performance/release gate.
- Do not commit `Cargo.lock` without a concrete dependency reason.
- Local-only paths never staged unless explicitly requested: `.analysis/`,
  `data/vanilla/`, `.serena/`, `.opencode/`, `x-ui-pro/`, `YOLO_MODE.md`,
  `log.log`, and local `opencode.json` overlays.

Treat `mc-net` as a modular monolith. Root orchestration files route work but do
not own new domain behavior. Put touched state machines, authority boundaries,
and transaction policy in focused modules with narrow interfaces. Mechanically
extract a touched legacy domain when bounded. Keep substantial tests in sibling
`*_tests.rs`; do not grow aggregate root or inline production test modules.

Architecture, authority, threading, waiting, persistence ordering, or module
policy changes update the owning ADR in the same checkpoint. Desired migration
is not current runtime truth.

## Routes And Evidence

Read `docs/MEMORY.md` as the live cursor, then the harness wiring in
`docs/AGENT_TOOLING.md` and only the route document the checkpoint names.
Current code, tests, configuration, and runtime evidence override memory. Do
not read milestone ranges, archives, raw sessions, or readiness ledgers as
startup context. `.ai-bridge/` historical logs, transcripts, and old run
scripts are preserved evidence, never startup context; its active index
points here for anything live.

Manual/client gate: PrismLauncher 26.1.2 against the route's documented debug
config, or the approved harness real-client profiles. Record whether it was
owner-run, agent-run through approved client MCP, or not run. Hard readiness
language requires `docs/DEFINITION_OF_DONE.md` and the exact evidence matrix;
skipped/manual-pending gates are never "green". Use exactly the `draft` /
`stabilization` / `release-ready` labels from `docs/DEFINITION_OF_DONE.md`;
if in doubt, the work is `draft`. A known failed owner/manual scenario stays
`blocked` until the same or a stricter real-client scenario is rerun and
recorded; green Cargo or harness gates cannot downgrade a failed real-client
observation.

## Communication

At most three substantive owner updates per checkpoint:

1. selected outcome, only when not obvious;
2. critical finding, material scope change, or genuine blocker;
3. closeout with exact evidence and next checkpoint.

Do not announce routine reads, commands, skills, polling, or every test. When
asked for progress since a prior prompt/time, report only that interval's
commits, observable capabilities, gates, unresolved work, and major time sinks.
