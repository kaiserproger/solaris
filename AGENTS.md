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
- `example.toml` — the dev config; points at
  `.analysis/test-world` and the vanilla data dir.

## Build / test / lint baseline

Every commit must keep the following green:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

If a commit breaks the baseline, fix it in the same commit or
the next `fix:` — don't leave the tree broken between commits.

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

## Memory layout

Persistent agent memory lives under
`~/.claude/projects/-home-user-solaris/memory/`. The same
content also seeds opencode via `~/.config/opencode/` mirrors.

Important entries:
- `project-status.md` — current branch + cumulative milestone
  state. Read at session start; update at milestone closeout.
- `feedback-*.md` — owner conventions (terse + no-stalls,
  git author, no external artifacts, verify claims).
- `project-adrs.md` — ADRs 0001 + 0002 in force.
- `reference-*.md` — local paths and validation workflow.
