# Agent Guide - Solaris

Solaris is a custom Minecraft Java Edition 26.1-compatible server,
written in Rust. `CLAUDE.md` is a symlink to this file.

Owner/local git identity: `kaiserproger <kaisergrobe@gmail.com>`.
Do not change git config.

## Operating Mode

Default to autonomous, terse, evidence-first engineering.

Do not blanket-read the whole docs stack at startup. Start from the user
prompt, branch/status, and the smallest local context that can answer the
task. Read broader docs only when the task route below calls for them.

For long `/goal` work, keep moving across checkpoints instead of stopping
on every uncertainty. Make a reasonable local decision, record the
assumption in the next update or milestone note, run validation at slice
boundaries, and continue on independent work when one gate is degraded.
Mark a task blocked only after repeated concrete attempts cannot make
meaningful progress without owner input or an external state change.

Keep the quality bar high without turning it into ceremony: no protocol
guesses, no fake validation, no hidden parity claims, no unrelated
rewrites, no untracked local artifacts in commits, and no invented
abstractions that tests do not need.

Optimization hacks and deliberately narrow fast paths are allowed when they
move a measured bottleneck. Document the reason, applicability boundary,
correctness fence, measured effect, and removal or fallback path in the same
slice. Prefer a reversible local hack over a broad speculative abstraction.
This permission does not override the non-negotiables below.

Apply the Pareto rule to delivery. Prioritize the small set of changes that
unlocks most real gameplay, multiplayer correctness, and scaling value.
Cover common and costly edge cases, but do not spend extended time polishing
rare cases while the next critical gameplay or core-architecture path is still
missing. Stop a hardening pass once its dominant risks are proved and move the
main objective forward.

The `superpowers` Codex plugin is intentionally left alone. Do not add
project instructions that require extra local plugin layers unless the
owner explicitly asks.

## Context Routing

Start from the prompt, `git status`, and `rg`. Read one primary document below;
follow its links only when the task needs them.

- Long-goal recovery after compaction: `docs/MEMORY.md`.
- No specific owner prompt: `docs/NEXT_SESSION.md`.
- Playable or 20-minute-loop work: `docs/playable/README.md`, then the current
  queue in `docs/playable/ACTIVE.md`.
- Milestone implementation: only the file matching the active milestone
  (currently `docs/milestones/M100.md`).
- Closeout or readiness claim: `docs/DEFINITION_OF_DONE.md`, the relevant
  milestone, and `docs/VALIDATION_LEDGER.md`.
- Roadmap or target shape: the relevant `docs/PROJECT_SPEC.md` section. Use
  `docs/CORE_M77_M100_ROADMAP.md` only for M77-M100 sequencing.
- Architecture, ownership, threading, or policy: `docs/decisions/README.md`,
  then the exact ADR. Update that ADR in the same slice.
- Performance, regional ownership, ECS, or autoscale: the exact active
  milestone plus ADR 0004/0005. Use `docs/M52_OPERATOR_PERFORMANCE_NOTES.md`
  only for metric definitions.
- Minecraft client MCP or agent-tool wiring: read `docs/AGENT_TOOLING.md` and
  `client-mod/solaris-client-agent/README.md`.
- Server Lua plugin/API work: `docs/PLUGINS.md` and the exact ownership ADR.
- Protocol or packet work: read ADR 0002, use
  `.analysis/protocol-dump.txt`, `tools/dump-vanilla-protocol.sh`, and
  `crates/mc-test-harness/src/bin/wire_probe.rs` as needed.
- Build/run questions: read `README.md` and `example.toml`.

Never read milestone ranges or `docs/archive/` as startup context. Archives are
for targeted evidence lookup only.

Prefer `rg`/`rg --files` for discovery. Use Serena or other symbol tools
when they are already useful, not as mandatory startup work.

## Playable Spike Mode

When the owner says "playable", "20-minute loop", "играбельно", or
"start over", route to `docs/playable/README.md` and
`docs/playable/ACTIVE.md`.

In this mode:

- Optimize for a real-client 20-minute playable loop, not M100 replacement
  readiness.
- Do not read `docs/NEXT_SESSION.md`, `docs/VALIDATION_LEDGER.md`,
  `docs/VALIDATION_COVERAGE_AUDIT.md`, or
  `docs/REPLACEMENT_READINESS.md` unless the task explicitly asks for
  readiness/ledger work.
- Do not edit readiness/ledger docs unless the owner asks.
- Use focused runtime tests and real-client/manual checks as feedback, but
  do not try to promote rows to `ready`.
- Prefer deleting/de-scoping broken breadth over adding new subsystems.
- Manual/client checks in this mode use
  `cargo run --bin mc-server -- --config playable.toml` unless the owner
  asks for another config.
- Navigation starts with `rg`. CodeGraph MCP is configured for Codex and may
  be used for targeted callers/callees, mutation paths, and blast-radius
  checks; do not use it as mandatory startup context.

## Non-Negotiables

- Agents never `git push`, merge into `main`, or create tags unless the
  owner explicitly instructs it.
- Never use wall-clock sleeps (`std::thread::sleep`, `tokio::time::sleep`,
  Python `time.sleep`, shell `sleep`, or equivalents) in production code,
  tests, harnesses, or tools. Synchronize through channels, notifications,
  process readiness, observed protocol/world state, or simulation events.
- All waiting must be push-driven: await the exact channel message,
  notification, socket packet, process-state change, or simulation event that
  proves readiness. Never treat guessed elapsed time, a quiet period, polling,
  or an arbitrary tick count as success. A timeout may only fail a stuck
  operation; it must never be the success condition. Waiting for simulation
  ticks is allowed only when tick progression itself is the behavior being
  tested or a protocol rule, not as a proxy for some other event.
- Use push, not pull: the producer that changes state must notify its consumers.
  Waiting code must block on that notification instead of periodically reading
  state to discover that something may have happened.
- Do not reintroduce operator-configured worker-thread percentages. Derive the
  process capacity once, then let runtime measurements and the autoscaler shift
  bounded work budgets and admissions between subsystems.
- Prefer the simplest direct implementation that is easy to read and prove,
  even when it takes more lines. Do not introduce indirection, generic helpers,
  or compact abstractions merely to make the diff shorter.
- Mojang bytes never enter the repo. Keep `.analysis/*` and
  `data/vanilla/*` gitignored.
- Packet IDs and field layouts come from `wire-probe`/`javap` against the
  bundled Mojang server, never memory or guesses.
- Gameplay parity claims need an oracle: vanilla capture, decompiled
  source inspection, or side-by-side harness evidence.
- Do not use `--release` for the dev loop; debug builds are the default.
- Do not skip hooks/signing flags unless the owner asks.
- Do not commit `Cargo.lock` changes without a concrete dependency reason.

Local-only paths that must not be staged unless the owner explicitly asks:
`.analysis/`, `data/vanilla/`, `.serena/`, `.opencode/`, `x-ui-pro/`,
`YOLO_MODE.md`, `log.log`, and local `opencode.json` overlays.

## Repo Map

- `crates/` - workspace members.
- `crates/mc-test-harness/tests/` - wire-level integration tests and the
  canonical harness gate.
- `docs/` - design notes, milestone plans/closeouts, ADRs, and DoD.
- `tools/` - vanilla extraction and protocol dump scripts.
- `example.toml` - development config, pointing at local vanilla sidecars.

## Validation

Full baseline before commits, release-ready language, and final milestone
closeout:

```sh
cargo run -p xtask -- code-health
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

For inner-loop work, run the focused real-path tests first, plus
`cargo fmt --all -- --check` and `cargo run -p xtask -- code-health`
when practical. Run workspace `test`/`clippy` after the final slice or
before a checkpoint commit.

If a higher-level gate was not run, say exactly that. Use the DoD labels
for vanilla oracle, harness, manual/client, performance, and concurrency;
never compress skipped or manual-pending coverage into "green". When a
state is degraded only because a gate was skipped or stale, proactively
restore normal by rerunning or replacing that gate before making readiness
claims.

`xtask code-health` is a fail-only architecture tripwire. It is useful,
but it is not gameplay, oracle, client, performance, or soak evidence.

## Development Workflow

For non-trivial code changes:

1. Scout the exact files/symbols.
2. Choose one independently revertible slice.
3. Identify focused real-path tests before editing.
4. Implement the smallest defensible diff.
5. Enumerate and test every reachable failure branch changed by the slice, not
   only the happy path: stale preconditions, owner rejection, partial/failed
   mutation, empty/no-op input, mode-specific behavior, and cleanup/publication
   failure where applicable.
6. Review the diff for negative-code issues: duplication, fake
   abstractions, broad config, unrelated churn, and unsupported claims.
7. Run focused validation, then broader gates at the checkpoint.

Treat `mc-net` as a modular monolith. Root orchestration files such as
`play.rs`, `session.rs`, and their aggregate test files route work but do not
own new domain behavior. Put each state machine, authority boundary, and
transaction policy in a focused domain module with a narrow explicit interface.
When changing legacy behavior still embedded in a root file, move that touched
domain into its own module in the same slice when the extraction is mechanical
and bounded. Keep substantial unit-test modules in sibling `*_tests.rs` files;
do not grow `play/tests.rs`, `session/tests.rs`, or inline production-file test
blocks further.

Subagents are useful for scouting, implementation, and review when file
ownership is separable. The main agent remains responsible for inspecting
their diff and reporting exact validation evidence.

Timebox each concrete independently revertible task to 90-120 minutes. If the
scope will not fit, split it early into disjoint write sets and delegate those
slices to subagents instead of stretching one task indefinitely. The timebox
limits scope; it never permits skipped correctness fences or fake validation.

Clean up old worktrees after their task ends. Remove clean worktrees immediately;
for dirty worktrees, first verify or archive any unique source changes, then remove
the worktree. Do not leave reproducible `target`, Gradle, build, or client run
artifacts consuming disk in inactive worktrees.

Fast pipeline mode: if the prompt contains the exact token
`quaka-whaka-zaka-du`, split independent domains across parallel agents.
This increases throughput only; it does not relax the non-negotiables or
final validation.

## Milestones

Milestone branches are `dev/MX-<short-name>` from `main` at the previous
milestone tag. The owner merges and tags.

For a new milestone, the first commit is normally
`docs/milestones/MX.md` with goal, strategy, sub-milestones, acceptance,
pitfalls, and open questions. If the owner has already authorized a fast
implementation pass, agents may commit logical draft checkpoints, but the
milestone doc and final response must label the state as `draft`,
`stabilization`, or `release-ready` according to actual evidence.

Use Conventional Commits. Commit bodies should explain "why" in a few
sentences, not repeat the diff.

## Manual Gates

Manual/client gate: PrismLauncher 26.1.2 against
`cargo run --bin mc-server -- --config example.toml` in debug mode.
The owner normally runs the real client; agents prepare the server and
say "ready, connect."

For client-visible or gameplay mechanics, plan the manual/client check
and record whether it was owner-run, agent-run through approved client
automation, or not run.

## Memory And Tooling

Do not treat Codex, Serena, opencode session logs, or generated memories
as mandatory startup context. Load memory only for a concrete need:
recovering prior project state, validating an owner preference, finding
local oracle/tool paths, or continuing a long goal after compaction.

Update memory only for durable future value: milestone state, oracle
paths, validation workflow, owner preferences, or stable tooling setup.
Do not store transient command output, guessed parity, secrets, or
Mojang/vendor artifacts.

Keep local plugin/tooling surface lean. Project-specific `.opencode`
plugins, MCP overlays, and large node_modules trees should stay disabled
unless the owner asks for that exact workflow. Prefer repo-native
commands and focused searches over extra always-on guardrails.

## Communication

Be terse. Updates to the owner are 1-3 sentences. If a command runs for
more than about 5 minutes, report the wait and keep useful work moving.

Before saying work is ready/done/parity/replacement-ready, state what was
actually proved and what was not. Hard readiness language requires the
DoD evidence matrix.
