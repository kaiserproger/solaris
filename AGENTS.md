# Agent guide — Solaris

Custom Minecraft Java Edition 26.1-compatible server, written in
Rust. This file is read by AI coding agents (Claude Code,
opencode, etc.) at session start. `CLAUDE.md` is a symlink to
this file.

The owner is `kaiserproger <kaisergrobe@gmail.com>` — that is the
local git identity, do not change it.

## Read these first

1. `docs/PROJECT_SPEC.md` (especially §9 "Milestone roadmap") —
   the target shape. Note: delivery has drifted from spec
   starting at M4 (lighting in place of "Block physics +
   fluids"); the milestone docs are authoritative for what
   actually shipped.
2. `docs/milestones/MX.md` for the most recent milestone(s) and
   the one currently on the active `dev/` branch — every doc
   has a "What landed where" table at the bottom.
3. `docs/decisions/` — ADRs in force:
   - **ADR 0001** — vanilla data sidecar (Mojang bytes never
     enter the repo; live in `.analysis/` and `data/vanilla/`
     and are gitignored).
   - **ADR 0002** — protocol metadata comes from `wire-probe`
     + `javap` against the bundled Mojang server, never from
     guessing or memory.
4. `README.md` for the build + run summary (mirrors what's in
   `example.toml`).
5. `docs/DEFINITION_OF_DONE.md` — the hard DoD, autonomous
   preflight, validation labels, and stabilization rules. Read this
   before claiming readiness or closing a milestone.
6. `docs/NEXT_SESSION.md` when starting a fresh session without a
   more specific milestone prompt.
7. `docs/CORE_M77_M100_ROADMAP.md` for core-MVP stabilization work
   through the M100 validation milestone.

## Repo layout

- `crates/` — workspace members. Cross-references in commit
  bodies use the crate name (`mc-net`, `mc-world`, …).
- `crates/mc-test-harness/tests/` — wire-level integration
  tests; the canonical CI gate for each milestone.
- `docs/` — design + per-milestone plans/closeouts +
  PROJECT_SPEC + ADRs.
- `tools/` — vanilla-data extraction + protocol-dump scripts.
- `.analysis/` — local-only: bundled Mojang `server.jar`,
  extracted test-world, protocol-dump.txt. Gitignored.
- `data/vanilla/` — extracted vanilla data sidecar (reports +
  registries). Gitignored.
- `x-ui-pro/` — ignored accidental nested checkout; not part of Solaris.
- `example.toml` — the dev config; points at
  `.analysis/test-world` and the vanilla data dir.

## Build / test / lint baseline

Every commit must keep the following green:

```sh
cargo run -p xtask -- code-health
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

If a commit breaks the baseline, fix it in the same commit or
the next `fix:` — don't leave the tree broken between commits.

Baseline green is necessary, not sufficient. A milestone closeout must
also say which higher-level gates were run: vanilla oracle, harness,
manual/client, performance, and concurrency. Use the labels in
`docs/DEFINITION_OF_DONE.md`; never collapse skipped/manual-pending
coverage into "green".

`xtask code-health` is a fail-only architecture tripwire. It is part of
the normal post-change gate, but it is not gameplay, oracle, client,
performance, or soak evidence by itself.

**Always use debug builds for dev.** Release has hung at the
`mc-server` binary linking step in past sessions; debug is
plenty fast for the manual gate. CI flips to release only when
the owner asks.

## Rust toolchain

Pinned to `1.94` in `rust-toolchain.toml`. Known trap: if cargo
errors with `could not compile sharded-slab` or
`cannot find module u32x4x2_avx2`, the toolchain is half-removed.
Reinstall:

```sh
rustup toolchain uninstall 1.94 && \
  rustup toolchain install 1.94 --profile minimal -c rustfmt -c clippy
```

## Milestone workflow

Each milestone X gets its own branch and a tag on `main`:

1. Branch: `dev/MX-<short-name>` cut from `main` at the
   previous milestone's tag.
2. **First commit is the plan**: `docs/milestones/MX.md` —
   goal, strategy, sub-milestones, acceptance, pitfalls, open
   questions. `docs: MX plan — <title>`. Wait for owner review
   before any code.
3. Sub-milestones land one commit at a time
   (`feat: MX.a …`, `feat: MX.b …`, `docs: MX.f closeout …`).
   Conventional Commits; each commit's body explains "why" in
   1–3 short paragraphs.
4. **Owner merges + tags.** Agents never run `git push`, never
   `git merge` into `main`, never `git tag`. The owner does
   these explicitly.

Fast implementation passes are allowed when the owner asks for them,
but they are draft work unless the hard DoD says otherwise. Label the
state as `draft`, `stabilization`, or `release-ready` in the milestone
doc and final response. Do not let a draft closeout sound like vanilla
parity or production readiness.

## Development workflow

Default development mode during core readiness work is negative-code
stabilization: reduce duplication, lock scope, fake abstraction, noisy
tests, and unsupported claims before adding gameplay breadth.

Use a bounded pipeline for non-trivial code work:

1. Scout the candidate and name the exact files/symbols.
2. Plan one independently revertible slice.
3. Identify focused real-path tests before editing.
4. Implement the smallest behavior-preserving diff.
5. Run focused tests plus `cargo fmt --all -- --check` and
   `cargo run -p xtask -- code-health`.
6. Run a negative-code review.
7. Run workspace `test`/`clippy` after the final slice.

Do not chase an arbitrary chunk count. If the remaining candidates are
only churn or behavior-risky without strong tests, stop and report that
frontier. A smaller kept diff is better than sixteen forced rewrites.

For risky cleanup, use one slice at a time. Keep packet-specific guards,
protocol labels, ordering, lock boundaries, and accounting local unless
the tests prove the extraction preserves them. If review says `REWORK`,
fix the small issue or revert your own slice; do not stack more changes
on top of a questionable risky diff.

Subagents are useful for scouting, planning, implementation, and review,
but the main agent remains responsible for checking their diff, rerunning
the required gates, and reporting exact evidence. Empty subagent output is
not a verdict.

## Fast pipeline mode

If a user prompt contains the exact token `quaka-whaka-zaka-du`, switch
to the fast parallel pipeline mode used in the June 2026 stabilization
session: split independent domains across multiple `task()` agents. This
means launching independent agents in parallel; each agent must run its
own complete scout -> plan -> test -> review -> implement -> review ->
verify loop inside that agent session. Do not replace this with one
main-thread plan plus helper searches. Fast-pipeline agents must run on
`gpt-5.5` with high reasoning. Parallelize implementation, validation,
and review where file ownership can be kept separate.

This mode increases throughput only; it does not relax repository rules.
Still avoid protocol guesses, fake validation, hidden parity claims,
unreviewed slop, untracked local artifacts, and unrelated rewrites. The
main agent remains responsible for merging the pipelines, resolving
conflicts, running final workspace validation, and reporting exact
evidence.

Before any milestone code, run the autonomous preflight from
`docs/DEFINITION_OF_DONE.md` and paste a terse result into the plan or
session update. If a preflight item is missing, either fix it, mark the
validation coverage as degraded, or stop when it invalidates the task.

## Style

- **Terse.** Updates to the owner in 1–3 sentences, not
  paragraphs. No "let me now…" / "I'll analyse…" — do the work
  and report the result.
- **No silent stalls.** If a command takes >5 minutes
  (compiles, big test runs, long searches), say so and proceed
  instead of blocking.
- **No fake validation.** Tests must exercise real code paths;
  manufactured data that passes by construction is worse than
  no test.
- **Hard DoD wording.** Say exactly what was proved and what was not.
  Phrases like "ready", "parity", "replacement-ready", and "done"
  require the evidence matrix from `docs/DEFINITION_OF_DONE.md`.
- **Comments only when "why" is non-obvious.** Identifiers
  carry the "what." Don't write "added for MX" / "used by Y"
  comments — those rot. PR/commit bodies are where this
  context lives.
- **No emojis** in code, docs, or commit messages unless the
  owner asks. Same for CLI output.

## Things to never do

- Push to remote, merge to `main`, or create tags without
  explicit owner instruction.
- Modify `git config` (local user is set deliberately —
  see [[feedback-git-author]]).
- Loosen `.gitignore` entries for `.analysis/*` or
  `data/vanilla/*` — Mojang bytes stay local.
- Commit `Cargo.lock` changes without a reason (gratuitous
  bumps clutter the diff).
- Use `--release` for the dev loop.
- Skip pre-commit hooks (`--no-verify`) or signing
  (`--no-gpg-sign`) unless the owner asks.
- Ask clarifying questions every 5 minutes. If stuck, try
  something reasonable, document the choice in the next
  message, and move on.

## Manual gates

PrismLauncher 26.1.2 client against a debug-build
`cargo run --bin mc-server -- --config example.toml`. The
owner runs these; agents prepare the server and say "ready,
connect."

Manual gates are no longer an afterthought. For client-visible or
gameplay mechanics, plan the manual/client check before implementation
and record whether it was run by the owner, run by an agent through an
approved client automation path, or not run. A future Minecraft-client
MCP server is an approved direction for making this autonomous, but it
must exercise a real vanilla client and report reproducible evidence.

## Protocol & data oracles

- `.analysis/server.jar` — bundled Mojang server (any 26.1.x).
- `tools/dump-vanilla-protocol.sh` → `.analysis/protocol-dump.txt`
  (javap dump of clientbound + serverbound IDs and shapes).
- `crates/mc-test-harness/src/bin/wire_probe.rs` — typed
  async driver that connects to a real vanilla server and
  dumps frames. Use this before adding any new packet.
- `mc_test_harness::client::Client` — same driver, used in
  integration tests (see `chunk_stream.rs`,
  `block_edit.rs`, etc.).

Packet IDs and field layouts are **cited from the javap dump
or a wire-probe capture, never guessed**. See ADR 0002.

Gameplay parity claims also need an oracle. Prefer vanilla captures,
decompiled source inspection, or side-by-side harness scenarios before
Solaris fixes. Solaris-only tests are useful scaffolding, not vanilla
parity evidence.

## Agent memory

Use Serena memories on demand, not as a blanket startup read. First call
Serena's onboarding check, then load only the memories needed for the
active domain.

| Domain | Load these memories |
|---|---|
| Any code change | `project/status`, `project/structure`, `style-and-conventions`, `feedback/terse-no-stalls`, `feedback/use-subagents-and-verify` |
| Protocol or packets | `project/adrs`, `project/26-1-2-is-real`, `reference/validation-workflow`, `feedback/verify-claims`, `feedback/protocol-oracles-prefer-decompiled` |
| Vanilla data, assets, or local artifacts | `project/adrs`, `reference/local-paths`, `feedback/no-external-artifacts` |
| Milestone plan or closeout | `project/status`, `task-completion`, `reference/validation-workflow`, `feedback/verify-claims` |
| Validation, CI, or gates | `suggested_commands`, `task-completion`, `reference/validation-workflow`, `feedback/verify-claims` |
| Agent/tooling setup | `reference/agent-tooling`, `feedback/terse-no-stalls`, `feedback/use-subagents-and-verify` |
| User communication only | `user-profile`, `feedback/terse-no-stalls` |

Update memory only when the new fact is likely to help future sessions:
milestone state, oracle paths, validation workflow, owner preference, or
tooling setup. Do not store transient command output or guessed parity.

## Agent tooling

- `docs/AGENT_TOOLING.md` is the detailed local setup reference for
  opencode commands, harnesses, RTK, Headroom, Context7, and session logs.
- Serena is available through opencode MCP and should be preferred for
  Rust symbol navigation/edits before full-file reads.
- Context7 is available and was verified on 2026-06-11. Use it for
  external library/framework docs; call library resolution before docs
  queries. If Context7 is down, fall back to local docs or direct web
  sources and state that fallback.
- RTK is installed at `/home/kaiserroman/.cargo/bin/rtk`; the global
  opencode RTK plugin rewrites Bash commands after opencode restart.
- Headroom is installed at `/home/kaiserroman/.local/bin/headroom` via
  `uv tool install "headroom-ai[all]"`. Do not route opencode provider
  traffic through `headroom proxy` unless the owner explicitly asks.
- Prior opencode sessions are part of project evidence when prior-session
  context matters. Start with `opencode session list`; for details query
  `~/.local/share/opencode/opencode.db`, especially `session`, `message`,
  and `part` tables. Useful text usually lives in `part.data` with JSON
  `type == "text"`.
- For non-trivial diffs, run a negative-code review before finalizing.
  Prefer the `harness-slop-reviewer` subagent or `/negative-code-review`;
  for small diffs, a direct self-review of `git diff` is acceptable.
