# Contributing to Solaris

Solaris is a draft project developed by the owner. Read [AGENTS.md](AGENTS.md)
for the working contract, project invariants and validation tiers, and use the
[documentation index](docs/README.md) to find the guide for the area you change.
Keep changes focused; discuss material scope or design choices before expanding
an issue. Core production builds must not require sibling repositories.

## Development setup

Follow the [source quick start](README.md#quick-start-from-source). Use debug
builds for normal development. The workspace's [Cargo config](.cargo/config.toml)
selects Clang/mold on Linux x86-64 and the debug test stack size. Content import
requires JDK 25; downloaded/extracted Mojang content must never enter Git.

## Validation

Run the canonical harness from the repository root:

```sh
python3 -m tools.harness list
python3 -m tools.harness run test -p mc-edge --lib <filter>
python3 -m tools.harness run correctness
```

Use focused tests for the edit loop. `correctness` is the L2 baseline: formatting,
code-health, strict workspace Clippy and workspace/all-target nextest tests.
Nextest policy lives in [`.config/nextest.toml`](.config/nextest.toml). Runs leave
receipts under `.analysis/validation/`; preparation-only output is not a pass.
On the shared owner workstation, use the memory runner described in AGENTS.md.
Documentation-only changes need static path/link checks, not a Cargo build.

Graphical, oracle and performance gates prove only their named scope. Consult
[agent tooling](docs/AGENT_TOOLING.md) and the [break-it QA protocol](docs/QA_AGENT_PROTOCOL.md):
player-visible changes and releases need a separate breaker, not just their own
happy-path pass. Do not widen a bound, add retries or replace real-client
coverage with protocol bots to turn a failing gate green.

## Changes and commits

- Add focused regression coverage for behavior changes and update the owning
  operator guide or ADR in the same change. Remove superseded paths rather than
  keeping duplicate authorities or compatibility shims.
- Preserve unrelated local changes. Do not stage ignored agent state, runtime
  worlds or Mojang content; change `Cargo.lock` only for a concrete dependency reason.
- Commits require explicit owner authorization. When authorized, use one
  revertible Conventional Commit per completed checkpoint. Pushing, merging to
  `main`, tagging and publication each require explicit permission; preserve hooks
  and signing.

## Documentation

The [documentation index](docs/README.md) separates operator/developer guides,
ADRs, harness inputs and release notes. Historical process logs are local archives,
not current specifications. Architecture, authority, threading or persistence
policy changes must update the owning ADR; do not treat a planned migration as
current runtime behavior.
