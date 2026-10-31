# Agent Guide — Solaris

Solaris is a Rust Minecraft Java 26.1.2 server; 1.0.0-rc.1 "Pauline" is the 1.0 release candidate.
The current binary is `mc-edge`'s `solaris-v2` over native `mc-core`; v1 is retired.
`CLAUDE.md` is a symlink to this guide. Owner identity is
`kaiserproger <kaisergrobe@gmail.com>`; never change Git configuration.

## Layout and navigation

- `crates/mc-core/`: authoritative gameplay, native tick, snapshots/intents/merge, WAL/checkpoints.
- `crates/mc-edge/`: CLI/config, sockets/auth, console/dashboard, connection presentation.
- Other `crates/`: protocol, data, world, worldgen, physics, plugins, Loader and tooling.
- `sdk/rust/`: independent Rust guest SDK workspace; `tools/harness/`: canonical validation.
- `example.toml`: normal config; `playable.toml`: harness survival config.
- [Documentation index](docs/README.md), [architecture](docs/ARCHITECTURE.md), [ADRs](docs/decisions/README.md).
- Resume from ignored `.analysis/MEMORY.md` and the active queue, then only the named route/source.
  Code, tests, config and runtime evidence override memory; archives and `.ai-bridge/` are not startup context.

## Owner contract (binding)

> ONCE AN HOUR OR BEFORE COMMIT LOOKUP AT OWNER OPEN ISSUES IN REPOSITORY

> "Do not overthink/overengineer every possible case until you explain exactly WHY it is needed and user agrees with you. Write as simple and clear as possible. Always ask user first what they fucking want."

> "Always double-check your work. Spawn another agent to give clear, concise second POV on your work. Do not do anything that isn't needed right now as it probably won't ever be needed. Verification and reporting happen at meaningful checkpoints, not after every command, tiny fix, or intermediate discovery."

> "On receiving new user request, first finish what you've started before taking new user request UNTIL it is explicitly required by user to take right now. Continue execution through a meaningful checkpoint; do not stop only to report intermediate progress."

> "Pareto: result first. Deliver the simplest version that works end to end for the real case; improve, harden or complete it only when the owner explicitly asks. Do the 20% that gives 80%. Don't burn tokens on rare edges, exhaustive inventories, byte-exact oracles, extra reviews or re-verifying what already passed. A rare blocker gets a clear error, not a subsystem." (owner, 2026-10-16)

> An explicit unambiguous request needs no reconfirmation. Ask only about a material unresolved choice.
  Make the smallest direct complete change: no speculative abstractions, flags, hardening or unrelated churn.

>  Finish active work before newer requests unless explicitly interrupted/replaced (`Stop`, `стоп`, `забей`,
  `поправка`); inspect processes/worktrees safely before switching. `Retry`/`продолжай` revalidates state.

> Autonomous `/goal` means the entire persistent objective and queue, not a slice: continue without
  intermediate stops until verified or externally blocked with no actionable work. Finite mode needs explicit permission.
  Slices need disjoint interfaces, not separate full gates, snapshots, reviews, sessions or commits.

> Update the live cursor/active queue/owning ADR at major phase close or genuine interruption, not per tool call.
  Keep exact receipts; a context handoff does not pause or complete the objective.

> At most three substantive updates per checkpoint: non-obvious outcome, material finding/blocker, closeout.
  No routine-command narration. Progress-since reports cover only that interval's commits, capabilities, gates and time sinks.

> Priorities: ordinary vanilla-client gameplay/multiplayer; production plugin API/adapters; measured
  optimization/ECS/ownership/autoscaling; rare edges. Promote a lower item only if ordinary play, saves or plugins are blocked.

## Build and validation

From the repository root (debug by default):

```sh
cargo build -p mc-edge --bin solaris-v2
cargo run -p mc-edge --bin solaris-v2 -- content import --version 26.1.2 --download
cargo run -p mc-edge --bin solaris-v2 -- --config example.toml
python3 -m tools.harness list
python3 -m tools.harness run test -p mc-core --lib <filter>  # L0: affected scope
python3 -m tools.harness run fmt                          # L1, plus focused tests
python3 -m tools.harness run code-health                  # L1
python3 -m tools.harness run correctness                  # L2: major phase/code commit/release
```

- On the shared owner machine, wrap heavy Cargo/harness/Gradle commands in
  `.analysis/tools/mem-run.sh -m 6G <command> ...` for focused work (three shared slots, ≤8G each).
  Full `test`/`correctness` use `-x -m 12G` per tooling. `-p` is serialized timing/profiling, never builds;
  `-x` takes all slots for full gates or exclusive acceptance.
  Measurement windows require Main's GO on a frozen integrated tree; compare only matching lanes.
  Replace shared runner scripts atomically, never in place; cancel the parent job, not only its children.
- L0: focused tests plus targeted diff/syntax checks. L1: focused tests, fmt and code-health;
  `codegraph sync .` when navigation relied on it. L2 correctness adds strict workspace Clippy and
  nextest workspace/all-target tests. See [tooling](docs/AGENT_TOOLING.md) for profiles and prerequisites.
- Markdown/instruction-only changes use static/path/link checks, not Cargo/harness gates.
- Validation identity is command + tree fingerprint + environment + scope: never repeat unchanged green checks.
  After a failure, rerun only the affected gate after a relevant fix. Bug/perf work first records the exact
  command/seed repro in `.analysis/codex-logs/<slug>/`, then fixes and reruns it; cursor links receipts, not raw facts.
- Harness runs own isolated artifacts and server/client/Xvfb lifecycle. Read
  `.analysis/validation/<UTC>-<profile>-<unique>/result.json` (`status`, `commands`, `exits`, `logs`, `details`, `error`).
  `prepared`/`--check`/`--prepare` is not a pass; unknown/unlaunchable/failing commands and timeouts fail closed.
  `code-health` is an architecture tripwire, not behavior/parity/performance proof; structural checks must not compare Rust statement text/order.
- Actual-client surfaces need actual-client gates (graphical Minecraft under Xvfb, exact scenario + evidence) and a
  separate breaker under the [break-it QA protocol](docs/QA_AGENT_PROTOCOL.md): an attack-matrix pass with repro'd,
  severity-ranked findings before push to `main` or a release tag. A happy-path gate alone is not acceptance; new or
  changed gates need a negative control, no pre-existing pass, every new branch on a real client, and ≥3 runs when variable.
  PrismLauncher 26.1.2 owner testing or approved client MCP must be identified as owner-run, agent-run or not run.
  Graphical prerequisites include Linux inotify/pidfd/Xvfb, MCP events and client credentials; TCP/unit tests never substitute.
- Owner field testing precedes fixed-seed tuning, followed by varied-seed exploration with seeds/repros recorded.
  Terrain-blocked stays blocked. Failed owner/manual behavior stays blocked until the same or stricter real-client gate passes.
  Use only `draft`, `stabilization`, `release-ready` per [DoD](docs/DEFINITION_OF_DONE.md) and its evidence matrix;
  default to draft, never call skipped/manual-pending gates green. Broad gates prove only their covered scope.

## Execution and subagents

- Choose one navigation path: targeted CodeGraph for indexed code; bounded search/read for docs/config/logs or stale/unindexed files.
  Do not reconfirm graph answers with broad searches. Bound paths/output; project JSON and narrow truncated queries.
  Verbose output belongs in ignored `.analysis/codex-logs/`; report status, failures and receipt paths.
- Long work is event-driven: launch once, continue independent work, consume completion once. Never poll process/status/agent
  lists or use repeated waits. Never add wall-clock sleeps in production, tests, harnesses or tools: wait for the exact
  message, packet, notification, process/world/simulation state; producers wake consumers (push, not pull).
- Delegate only when owner/runtime authorizes; `quaka-whaka-zaka-du` explicitly authorizes parallel work.
  Larger pipelines require explicit owner permission. Give disjoint responsibilities/write sets; never delegate the immediate blocker then idle.
  No full conversation/history forks: pass bounded task, base commit, paths and evidence contract; close completed agents.
- Use canonical harness lifecycle/receipts for delegated validation/QA, not hand-rolled tmux/PID/log plumbing or polling.
  Self-check, then one independent read-only reviewer at major completed phases (not each slice).
  Reviewers neither edit nor delegate; fixing findings does not trigger another reviewer.
- One compact YAML result per agent/revision: `verdict: pass|changes|blocked`, `findings`, `changed_files`,
  `validation`, optional `report_path`. Long detail lives in a file; deduplicate notifications by agent_id + revision.
- In autonomous goal mode, do not load/announce generic brainstorming/TDD/debugging/verification skills unless requested;
  mandatory tool skills still apply. If higher-priority runtime requires generic skills, obey and report conflict; do not pretend disabled.

## Project invariants

- Mojang bytes never enter Git. `.analysis/` and all `data/vanilla/` are local and ignored.
  Packet IDs/layouts come from local `wire-probe`/`javap`, never memory; protocol and Anvil layouts need exact evidence.
  Gameplay/worldgen need minecraft.wiki-level vanilla similarity, not bit-exact RNG/AI cadence/Mth/terrain.
  Performance and correctness (no crashes/dupes/corrupt saves) outrank mob AI parity.
- Core v2 persists only native store + tick WAL: no dual-write or Anvil fallback; corrupt/unsupported state fails closed.
  Gameplay authority stays in native tick/intent merge; edge owns presentation/connection state.
  Root orchestration routes work, not domain policy; substantial tests belong in focused sibling `*_tests.rs` modules.
- No released Solaris compatibility surface: migrate callers and delete superseded APIs/schemas/authorities/adapters/flags/fallbacks.
  Compatibility targets are vanilla protocol and Anvil, not old Solaris storage. Architecture/authority/threading/waiting/
  persistence-order/module-policy changes update the owning ADR; desired migration is not runtime truth.
- Debug builds by default; `--release` only for an explicitly required performance/release gate.
  Measured narrow optimization hacks must document bottleneck, boundary, correctness fence, effect and fallback/removal path.
  Never restore operator worker-thread percentages; derive capacity once and let measured autoscaling move bounded budgets.

  Real-client gates run the server from a release build (owner, 2026-10-27): build
  `cargo build --release -p mc-edge --bin solaris-v2` and pass it as `SOLARIS_SERVER_BINARY`;
  a debug server drops to ~10 TPS while a joining player's chunks generate.
## Git, local state and sibling repositories

- Never commit/push/publish/merge to main/tag without explicit authorization; preserve hooks/signing.
  Authorized checkpoint commits are local, revertible Conventional Commits. Do not commit Cargo.lock without a concrete dependency reason.
- Preserve unrelated dirty files: never reset, clean or stage them. Inspect unique changes before removing inactive worktrees;
  discard only reproducible build/client artifacts. Before reboot stop processes/agents, classify slices as committed,
  complete-but-unverified or partial, write an ignored resume cursor with one next action, and revalidate after restart.
- Without commit permission record base_tree, diff_hash (SHA-256), changed_files, validation and one concrete next action;
  use path-limited status/diffs. Never stage local `.analysis/`, `data/vanilla/`, `.serena/`, `.opencode/`,
  `x-ui-pro/`, `YOLO_MODE.md`, `log.log`, `opencode.json` overlays or other ignored agent/tool artifacts unless explicitly requested.
- This repo owns core Rust/runtime and canonical harness. `../solaris-loader` owns Java/client scenarios
  (`SOLARIS_LOADER_ROOT` override allowed); never duplicate a client-mod source tree here.
- `../solaris-default-plugins` owns first-party Wasmtime packages (`solaris:plugin@0.8.0`), deployed via `[plugins].directory`;
  no compiled-in package list or `plugins.bundled` compatibility path. Core production builds require neither sibling;
  integration gates require the sibling they exercise. Each sibling owns its README/AGENTS/Git metadata;
  creating it locally never authorizes staging, commits, remotes, pushes or publication.
