# Contributing to Solaris

This is currently a solo project in early bootstrap (M0). External contributions
are not yet being accepted; the structure below describes the intended workflow
once the project opens up.

## Workflow

- `main` is always green — CI must pass before merge.
- Feature work happens on `dev/MX-name` branches per milestone, where `MX`
  matches the milestone in [`docs/PROJECT_SPEC.md`](docs/PROJECT_SPEC.md) §9.
- Milestones are tagged on `main`: `m0`, `m1`, …, `v1.0`.

## Commit style

Conventional commits:

- `feat:` new functionality
- `fix:` bug fix
- `refactor:` no behavior change
- `test:` tests only
- `docs:` documentation only
- `chore:` tooling, build config

## Local checks before opening a PR

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Architectural decisions

Nontrivial decisions are recorded as ADRs under `docs/decisions/`. If a change
affects something documented in `PROJECT_SPEC.md`, update the spec in the same
PR.
