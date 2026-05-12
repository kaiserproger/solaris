# Claude Code Prompts — Template + M0

This file contains:
1. **Prompt template** for Claude Code, to be filled in for each milestone
2. **M0: Project Bootstrap** — the first milestone, fully expanded and ready to copy into Claude Code

---

## Part 1: Prompt Template

Used for each milestone after M0. You fill it in based on the current state of the project and the requirements of the specific milestone.

```markdown
# Milestone M{N}: {Title}

## Context

You are implementing milestone M{N} of a Rust-based Minecraft 26.1-compatible server engine. The project is owned by a strong-Rust solo developer working part-time (~15h/week). Detailed project context is in `docs/PROJECT_SPEC.md` — read it before starting work.

**Previous milestones completed:**
- M0: Project bootstrap (workspace, CI, base crates)
- M1: Network handshake (status + login states)
- ... (list)

**Current state of repo:**
- Branch: `dev/M{N}-{name}`
- Base: `main` at commit {sha}
- Relevant existing code: {paths}

## Goal

{One paragraph: what does this milestone produce. Be concrete: a binary that does X, a crate that exposes API Y, a test suite that validates Z.}

## Scope

### In scope
- {bullet list of what's included}

### Out of scope
- {bullet list of what's explicitly NOT in this milestone, with a pointer to which milestone covers it}

## Acceptance criteria

This milestone is done when:

1. {Specific testable criterion. E.g., "running `cargo test -p mc-protocol` passes all tests"}
2. {Another criterion. E.g., "vanilla 26.1 client can connect to `localhost:25565` and reach Play state without disconnect"}
3. ...

Each criterion must be **mechanically verifiable** — either passes a test, runs to completion, or produces a specific observable output.

## Technical specification

### Modules to create/modify

```
crates/{crate}/src/
├── {file1}.rs         — {what it contains}
├── {file2}.rs         — {what it contains}
└── ...
```

### Public API surface

```rust
// crates/{crate}/src/lib.rs
pub fn ... // signature with doc comment
pub struct ... // signature
```

### Key invariants
- {invariant 1, e.g., "All public types implement Debug"}
- {invariant 2, e.g., "No unsafe in this milestone"}

### External references
- {wiki.vg link}
- {Minecraft Wiki link}
- {existing implementation in another open-source project as a reference, NOT to copy from}

## Implementation guidance

### Order of work
1. {Step 1: what to do first, why}
2. {Step 2}
3. ...

### Pitfalls to avoid
- {Specific gotcha 1, often based on prior debugging experience}
- {Specific gotcha 2}

### Testing approach
- {Where to put tests, what kinds: unit / integration / property}
- {Specific test cases to include}

## Out-of-scope adjustments

If during implementation you find that achieving acceptance criteria requires changes to modules outside this milestone's scope:
- DO NOT make those changes silently.
- STOP and add a comment in code (`// TODO(M{N}): need to revisit X`).
- Continue with what's in scope, leaving a clear marker.
- Report blockers in the final summary.

## Deliverables

When complete, provide:
1. List of files created/modified
2. Test results (`cargo test` output)
3. Brief explanation of any deviation from this spec
4. List of TODOs/follow-ups discovered
5. Recommendations for M{N+1} based on what you learned
```

---

## Part 2: Principles for using these prompts

To make prompts work instead of producing garbage:

### 2.1 What you do BEFORE the prompt

- **Write the milestone document** at `docs/milestones/MX.md` by hand. It contains details that aren't in the general PROJECT_SPEC. This document then becomes the prompt.
- **Read fresh references.** wiki.vg may have changed; Pumpkin may have done something new. Freshness of knowledge is your job, not Claude's.
- **Set a baseline** — current main is stable, tests are green, benchmarks recorded.
- **Create the feature branch** `dev/MX-name` from clean main. Claude Code works in it.

### 2.2 What you do DURING Claude Code work

- **Don't interrupt every 10 minutes.** Let the agent run for a meaningful amount of work (1-3 hours) before intervening, otherwise it can't get to a coherent solution.
- **Watch `git diff` periodically.** If you see something weird — stop, investigate.
- **Don't let the agent commit to main.** Feature branch only.

### 2.3 What you do AFTER Claude Code work

- **Code review like an external PR.** Check every file. Especially: error handling, unsafe, performance hot paths.
- **Run the tests yourself**, don't trust the agent's report.
- **Run the benchmarks** if the milestone touches them.
- **Verify integration** with existing code — API not broken, no surprise dependencies appeared.
- **Sleep on it**, then merge. Don't merge same-day as you wrote.

### 2.4 When NOT to use Claude Code

- Architectural decisions (library choice, pattern choice)
- Debugging subtle race conditions
- Performance tuning of hot paths
- Parity tasks against vanilla (need a diff against an oracle)
- Anywhere fail-silent is more dangerous than fail-loud

---

## Part 3: M0 — Project Bootstrap (full prompt)

This is the first milestone, ready to copy into Claude Code. Replace `{REPO_NAME}` with your chosen name.

```markdown
# Milestone M0: Project Bootstrap

## Context

You are implementing the foundational milestone of a Rust-based Minecraft 26.1-compatible server engine. This is M0 — there are no prior milestones. Project specification is in `docs/PROJECT_SPEC.md` (already created). Read it carefully before starting.

The owner is a strong-Rust solo developer. They will review every line of code you produce. Do not over-engineer; do not under-engineer. Lean toward standard patterns and explicit code over clever abstractions.

## Goal

Set up the workspace structure, basic crate scaffolding, CI pipeline, and a "hello world" binary that proves the project skeleton works end to end. Produces no game functionality, only foundation.

## Scope

### In scope
- Cargo workspace at repository root with crates per PROJECT_SPEC §3.1
- Each crate has a minimal `lib.rs` with a crate-level doc comment, version, and one trivial public function or type
- `mc-server` crate has `main.rs` with a CLI accepting a `--config` flag, parsing a TOML config, and printing parsed config to stdout
- `Cargo.toml` workspace with shared dependencies (tokio, serde, tracing, etc.) defined at workspace level
- `.github/workflows/ci.yml` running `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `cargo build --release` on Ubuntu
- `rust-toolchain.toml` pinning Rust version
- `.gitignore` for Rust + IDE files
- `README.md` with project name, brief description, build instructions, license placeholder
- `LICENSE` file (MIT/Apache-2.0 dual, the standard Rust ecosystem default)
- `CONTRIBUTING.md` placeholder with structure
- `docs/` folder with `PROJECT_SPEC.md` (assume it exists), `milestones/M0.md` (this document)
- All crates compile with `cargo build` and pass `cargo test` (each has at least one trivial test)

### Out of scope
- Any actual Minecraft protocol implementation (M1)
- Any world data structures (M2)
- Networking code beyond a TODO comment (M1)
- Differential testing infrastructure (M2.5)
- Java client mod (M12)

## Acceptance criteria

1. `git clone <repo> && cd <repo> && cargo build --release` succeeds on a clean Ubuntu 24.04 system with Rust stable installed
2. `cargo test --workspace` reports all tests passing (≥ 1 test per crate)
3. `cargo fmt --check` exits 0
4. `cargo clippy --workspace -- -D warnings` exits 0
5. `cargo run --bin mc-server -- --config example.toml` reads `example.toml` and prints parsed contents
6. CI workflow runs successfully on a test PR (verifiable by the developer post-merge)
7. Each crate's `Cargo.toml` has `description`, `version` (0.0.1), and shared workspace metadata
8. No `unsafe` code in this milestone
9. No external dependencies beyond standard ecosystem (tokio, serde, tracing, clap, anyhow, thiserror, toml — no project-specific Minecraft crates yet)

## Technical specification

### Crate list (from PROJECT_SPEC §3.1)

```
crates/
├── mc-protocol/         # placeholder, no impl yet
├── mc-nbt/              # placeholder
├── mc-world/            # placeholder
├── mc-worldgen/         # placeholder
├── mc-physics/          # placeholder
├── mc-entity/           # placeholder
├── mc-net/              # placeholder
├── mc-data/             # placeholder
├── mc-extension/        # placeholder
├── mc-script/           # placeholder
├── mc-server/           # main binary, CLI scaffolding
└── mc-test-harness/     # placeholder
```

### Workspace Cargo.toml

Use workspace inheritance for common settings:

```toml
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.package]
version = "0.0.1"
edition = "2024"
rust-version = "1.{latest}"
license = "MIT OR Apache-2.0"
authors = ["{author}"]
repository = "{repo url}"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
clap = { version = "4", features = ["derive"] }
anyhow = "1"
thiserror = "2"
```

### mc-server CLI

`mc-server` should:
- Use `clap` derive
- Accept `--config <path>` (default: `./config.toml`)
- Parse the TOML config into a struct `ServerConfig`
- Print parsed config in pretty format (use `serde_json::to_string_pretty` for output)
- Exit 0 on success, exit 1 on parse error with a clear error message

`ServerConfig` skeleton:
```rust
#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub server: ServerSection,
    pub network: NetworkSection,
}

#[derive(Debug, Deserialize)]
pub struct ServerSection {
    pub name: String,
    pub motd: String,
}

#[derive(Debug, Deserialize)]
pub struct NetworkSection {
    pub bind_address: String,
    pub port: u16,
}
```

`example.toml`:
```toml
[server]
name = "MyServer"
motd = "A Rust Minecraft Server"

[network]
bind_address = "0.0.0.0"
port = 25565
```

### CI workflow (`.github/workflows/ci.yml`)

Runs on `pull_request` and `push` to `main`:
- Job 1: `cargo fmt --check`
- Job 2: `cargo clippy --workspace --all-targets -- -D warnings`
- Job 3: `cargo test --workspace`
- Job 4: `cargo build --workspace --release`

Use `Swatinem/rust-cache` action for caching.

### Doc comments

Every `lib.rs` must start with:
```rust
//! # {crate-name}
//!
//! {one-line description from PROJECT_SPEC}
//!
//! Part of the {project-name} engine.
```

### Tests

Each crate gets a trivial smoke test in `lib.rs`:
```rust
#[cfg(test)]
mod tests {
    #[test]
    fn it_compiles() {
        assert_eq!(2 + 2, 4);
    }
}
```

(Yes, trivial. The point is to verify CI runs tests across all crates.)

### `mc-server` integration test

In `crates/mc-server/tests/cli.rs`, write a test that:
1. Creates a temp file with valid TOML
2. Runs the binary with `--config <tempfile>`
3. Asserts exit code 0 and output contains expected fields

Use `assert_cmd` and `tempfile` crates.

## Implementation guidance

### Order of work

1. Create root `Cargo.toml` workspace, `.gitignore`, `LICENSE`, `README.md`, `CONTRIBUTING.md`
2. Create all 12 crate directories, each with minimal `Cargo.toml` and `src/lib.rs`
3. Add a trivial `it_compiles` test to each crate
4. Verify `cargo build` and `cargo test` work for the whole workspace
5. Implement `mc-server` CLI with `--config` flag
6. Add `example.toml`
7. Implement the integration test in `mc-server/tests/cli.rs`
8. Add `.github/workflows/ci.yml`
9. Verify locally that all CI commands pass
10. Final pass: ensure all `Cargo.toml` files have proper metadata, all `lib.rs` have doc comments

### Pitfalls to avoid

- Do NOT add Minecraft-specific dependencies yet. No `valence`, no `azalea`, no NBT crates. Those come in M1+.
- Do NOT prematurely structure modules within crates. Each crate is just `lib.rs` with one function. Internal structure is up to future milestones.
- Do NOT write production code in `mc-server/src/main.rs` beyond CLI parsing. No tokio runtime setup yet (that's M1). No actual server logic.
- Workspace edition is 2024. If clippy complains about edition-specific lints, fix the code, don't downgrade the edition.
- `cargo clippy -- -D warnings` is strict on purpose. If clippy nags, fix the nag, don't suppress unless there's a strong reason documented in a comment.

### Testing approach

- Each crate: 1 trivial test (`it_compiles` style)
- `mc-server`: integration test using `assert_cmd`
- Total tests in M0: ~13 (one per crate + integration)

## Out-of-scope adjustments

If achieving M0 requires structural changes that don't fit "trivial scaffolding" (e.g., it turns out workspace dependency resolution needs special handling, or CI needs cross-platform support), STOP and document the issue rather than fixing silently. M0 should not be hard.

If you find yourself writing actual logic (parsing protocols, handling chunks, etc.) — STOP, you've crossed into M1+ scope.

## Deliverables

When complete, provide:

1. List of all files created (expect ~30-40 files)
2. Output of `cargo test --workspace` (paste verbatim)
3. Output of `cargo clippy --workspace -- -D warnings` (paste verbatim)
4. Confirmation that `cargo run --bin mc-server -- --config example.toml` produces the expected output
5. Any deviations from this spec, with rationale
6. Recommendations for M1 (network/handshake) based on what you observed during scaffolding
```

---

## Part 4: What to do once M0 is done

### 4.1 Checklist before starting M1

- [ ] M0 merged into main, tag `m0` placed
- [ ] CI green for 3+ commits
- [ ] You've actually run the binary and it works
- [ ] You've read 26.1 protocol spec on wiki.vg, at least the overview + handshake/status sections
- [ ] You've decided on `valence-protocol` vs roll-your-own protocol crate (recommendation: roll-your-own — you have the Rust skills, and the protocol layer is too central to delegate)
- [ ] You've created `docs/milestones/M1.md` from the template
- [ ] Branch `dev/M1-network-handshake` created, clean from main

### 4.2 Roadmap of milestone documents to write

After M0 — fill in one at a time, before starting work on it. Don't write them all at once: design reality changes.

- `docs/milestones/M1.md` — Network + handshake (status state, login state, encryption)
- `docs/milestones/M2.md` — World representation (blocks, chunks, NBT)
- `docs/milestones/M2_5.md` — Differential testing infrastructure
- `docs/milestones/M3.md` — Empty world end-to-end (client connects, sees flat world, walks)
- ... and so on per PROJECT_SPEC §9

### 4.3 When to update PROJECT_SPEC

At the end of each milestone you may discover something that requires changing the document. Examples:

- At M1 you decided to use `valence-protocol` instead of roll-your-own → update §2.4 dependencies, add an ADR
- At M3 you found bevy_ecs is too slow → update §3.2, add an ADR with measurements
- At M7 you decided worldgen scope is too big → update §6 modpack scope or §9 roadmap

PROJECT_SPEC.md is a **living document**, not a template. Update it. Tag versions.

---

## Part 5: Antipatterns — what NOT to put in prompts

To keep Claude Code from going off the rails:

**❌ "Implement a Minecraft server in Rust"** — too broad, the agent will try to do everything and do it badly.

**✅ "Implement M1: Network + handshake. See docs/milestones/M1.md."** — narrow, verifiable.

**❌ "Make it as fast as possible."** — not a criterion, won't stop the agent in the right place.

**✅ "Throughput must be ≥ 1000 packets/sec on test scenario X. Benchmark with `cargo bench --bench packet_throughput`."** — concrete.

**❌ "Follow Minecraft conventions."** — which? Specifically?

**✅ "Block IDs must be u32 per protocol §X.Y. Block state palette must be variable-bit-width packed per protocol §X.Z."** — exact.

**❌ "Add tests."** — which, how many, on what?

**✅ "Add unit tests for VarInt encode/decode covering: zero, max, min, random sample of 100 values. Add a property test for round-trip identity. Tests must achieve ≥ 95% line coverage of the codec module."** — measurable.

---

## Part 6: Final reminder

**This document is a starting point, not sacred text.** You will change it. You will find cases where the template is too unspecific, or where you need a new section. Do it.

**Main signs the approach is working:**
- Every 2-4 weeks you see a finished tangible artifact
- Tests stay green between milestones
- Architectural decisions are recorded in ADRs, not lost in your head
- When you come back after a break, `docs/milestones/MX_current.md` tells you what you were doing

**Main signs the approach is NOT working:**
- Code review of each Claude Code PR takes longer than just doing it yourself
- "Loose ends" appear between milestones (TODOs, broken tests)
- PROJECT_SPEC isn't updated, drifts from the code
- You feel like you don't understand your own code

If the first set — keep going. If the second — stop, reassess: either the milestones are too big, Claude Code isn't helping on this part of the task, or you need a different decomposition.

Good luck. This is a **realistic** project. It's long, but achievable.
