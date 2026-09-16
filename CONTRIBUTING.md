# Contributing to Solaris

Solaris is developed by the owner; `AGENTS.md` is the working contract for
every change, human or agent.

## Before you change anything

- Read [`AGENTS.md`](AGENTS.md) — owner contract, validation tiers, and the
  project invariants.
- Read [`docs/AGENT_ROUTES.md`](docs/AGENT_ROUTES.md) to see which document owns
  the area you touch.
- Core builds must never require the sibling repositories; integration gates
  explicitly need the one they exercise.

## Local checks

Every gate runs through the canonical harness from the repository root, which
leaves a receipt under `.analysis/validation/`:

```sh
python3 -m tools.harness list
python3 -m tools.harness run correctness   # L2: fmt, code-health, clippy, workspace tests
```

Focused development runs the affected crate's own tests instead of the
workspace scope. Graphical, oracle, and scale gates run only for the scope they
cover; see [`docs/AGENT_TOOLING.md`](docs/AGENT_TOOLING.md) before launching
one. Do not hand-roll the flags the harness owns, and do not widen a bound or
add a retry to turn a red gate green.

## Commits

Conventional Commits, one revertible commit per completed checkpoint. Never
push, merge to `main`, or tag without explicit owner authorization, and never
skip hooks or signing.

## Documentation

`docs/` root holds the documents that describe current behaviour; dated
evidence, milestone logs, and phase reviews live under `docs/milestones/`,
`docs/evidence/`, `docs/memory/`, and `docs/performance/`. Update the owning
document — architecture, ADR, or operator guide — in the same change as the
behaviour it describes, and delete what the change supersedes instead of
leaving a second version behind.
