# NEXT_SESSION - Solaris Agent Start Prompt

Use this for the next Claude / opencode session unless the owner gives a
more specific milestone prompt.

## 1. Read First

1. `AGENTS.md`
2. `docs/DEFINITION_OF_DONE.md`
3. `docs/PROJECT_SPEC.md` §9-10
4. `docs/CORE_M77_M100_ROADMAP.md` for M77-M100 core work
5. Latest `docs/milestones/M*.md` closeout and the active milestone plan
6. `docs/REPLACEMENT_READINESS.md` when touching vanilla/client-visible behavior
7. Relevant ADRs in `docs/decisions/`

## 2. Autonomous Preflight

Before code or claims, run the preflight from `docs/DEFINITION_OF_DONE.md`:

```sh
pwd
git rev-parse --show-toplevel
git status --short --branch
rustc --version
cargo --version
cargo fmt --version
java --version
javap -version
test -f .analysis/server.jar
test -d data/vanilla/reports || test -d data/vanilla
```

Report it as `full`, `degraded`, or `blocked`. If degraded, say which
gate becomes weaker.

## 3. Current Quality Mode

Default mode is `draft` unless the owner explicitly asks for
stabilization or release readiness.

- `draft`: fast breadth-first implementation is allowed, but known gaps
  must be written down.
- `stabilization`: stop adding breadth; reduce parity, performance,
  multithreading, and test-quality debt.
- `release-ready`: only claim this after the full evidence matrix is
  satisfied.

## 4. Hard DoD For Closeout

Every closeout or final answer must include:

- Quality label: `draft`, `stabilization`, or `release-ready`.
- Cargo baseline: exact commands run after the final change.
- Focused tests: what behavior they prove and why they are real-path.
- Vanilla oracle: run/cited or explicitly not run.
- Client/manual gate: owner-run, agent-run through real-client automation,
  prepared only, or not run.
- Performance/concurrency: measured or explicitly not relevant.
- Known gaps: concrete list, not generic "future work".

Do not claim vanilla parity, replacement readiness, or production-quality
behavior without this matrix.

## 5. Vanilla Parity Target

The project is not bit-perfect vanilla, but the release target is at
least 80% of scoped overworld-survival mechanics covered and implemented
well enough for a normal vanilla 26.1.2 client. The remaining behavior
must be explicit non-goal, deferred debt, or documented Solaris
semantics.

## 6. Client Automation Backlog

If asked to improve validation infrastructure, a high-value direction is
an MCP server or equivalent tool that drives a real PrismLauncher/vanilla
client and records reproducible observations. Protocol-only bots do not
replace this gate.

## 7. Owner Boundaries

Agents prepare branches, commits, docs, and validation. The owner merges,
tags, pushes, and performs default PrismLauncher gates unless explicitly
delegated in the current session.
