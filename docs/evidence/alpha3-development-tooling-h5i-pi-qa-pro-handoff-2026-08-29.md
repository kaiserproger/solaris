# Alpha-3 development tooling / h5i / Pi / QA / Pro handoff evidence

Date: 2026-08-29

## Installed local toolchain

Installed or verified in user space:

- Rust stable components: `rustfmt`, `clippy`, `rust-analyzer`;
- nightly Rust toolchain for explicit fuzz runs;
- `cargo-nextest 0.9.143`, `cargo-audit 0.22.2`, `cargo-deny 0.20.2`;
- `cargo-llvm-cov 0.9.0`, `cargo-bloat 0.12.1`, `cargo-machete 0.9.2`;
- `cargo-flamegraph` / `flamegraph 0.6.14`, `cargo-fuzz 0.13.2`;
- `hyperfine 1.20.0`, `just 1.58.0`, `fd 10.5.0`, `ninja`;
- existing `rg`, `jq`, `cmake`, `pkg-config`, `uv`.

MCP stdio servers installed:

- `@modelcontextprotocol/server-filesystem`;
- `mcp-server-git 2026.8.18`;
- `mcp-shell-server 1.1.9`;
- `rust-analyzer-mcp 0.4.0`.

`supergateway` is intentionally **not installed** while CodexPro is the active connector. `cargo-mcp` is intentionally omitted because Cargo is already exposed through the bounded shell MCP and `just` recipes.

Prepared stdio wrappers:

- `~/.local/bin/solaris-mcp-files` — filesystem root restricted to this repository;
- `~/.local/bin/solaris-mcp-git` — git server pinned to this repository;
- `~/.local/bin/solaris-mcp-shell` — allowlisted command/environment wrapper;
- `~/.local/bin/solaris-rust-analyzer-mcp` — persistent semantic/LSP server from this repository root.

Live MCP smoke passed for all four wrappers: filesystem 14 tools, git 12, shell 1, rust-analyzer 11. Shell policy executed `cargo --version` and rejected `curl` as `Command not allowed: curl`.

## h5i

Installed `h5i 0.3.8` and installed its bundled skill for local Codex/Claude runtimes. Host probe reports Landlock ABI 8, user namespaces and seccomp, but the stronger process isolation tier cannot execute under the current host policy; workspace isolation is runnable.

Pi model calls were proven inside an h5i workspace box. Forum attach is available only through h5i's explicit `--allow-unconfined` fallback on this host; forum text is therefore treated as untrusted coordination evidence and cannot widen the h5i/write-set contract.

CodexPro now installs the h5i skill explicitly inside every Pi box, creates/uses the host-owned open `<Project> agent coordination` thread, seeds a bounded recent-run manifest at `.ai-bridge/parent-pi-evidence.md`, and auto-revokes terminal participants. QA/reviewer boxes additionally receive parent `data/vanilla` as a bounded runtime-only sidecar (64 MiB cap, symlink refusal, recorded file/byte count and tree SHA-256) so graphical server startup is reproducible without putting Mojang-derived bytes into Git. Agents are told not to close or mark the shared thread done.

Dirty seeding rejects symlinks whose resolved target leaves the repository; an injected `/etc/hosts` symlink was rejected with `untracked Pi seed symlink escapes workspace`, its created h5i box was automatically aborted, and the forum identity was revoked. Untracked file seeding is capped at 64 MiB and excludes `.ai-bridge/pi-agents/` recursive artifacts.

## CodexPro Pi lifecycle

The installed CodexPro 0.30.0 package now exposes in standard/full mode:

- `spawn_pi_detached`;
- `spawn_pi_qa_detached`;
- `wait_pi_agent`.

A fresh MCP process listed all three tools. `wait_pi_agent` accepts `max_wait_seconds` and waits for the terminal `done` marker through a filesystem event, up to 60 seconds, rather than requiring tmux/ps/tail polling.

Default agent policy:

- GPT-5.6 Sol — implementation;
- GPT-5.6 Luna — QA/review.

The lifecycle is h5i-boxed and detached; it writes an agent-only patch relative to the seeded snapshot plus h5i export/receipt evidence. It never auto-applies the agent patch to the parent repository.

Validation:

- `node --check dist/server.js`: PASS;
- `node --check dist/piAgentOps.js`: PASS;
- CodexPro package `scripts/smoke.mjs`: `✓ smoke test passed`;
- final no-edit lifecycle smoke: agent exit 0, h5i export exit 0, agent patch 0 bytes, explicit h5i skill loaded, bounded parent evidence present, forum read/post succeeded, terminal identity revoked;
- final open coordination thread remained open after the agent terminated.

The CodexPro connector was subsequently restarted/refreshed and the live MCP session now advertises and successfully invokes all three Pi lifecycle tools.

The customization is persisted as a version-pinned Solaris overlay:
`tools/codexpro-pi-overlay/codexpro-0.30.0.patch` plus
`tools/install-codexpro-pi-overlay.sh`. The installer hash-checks pristine and
patched CodexPro 0.30.0 files, refuses an unknown/partial build, and reruns syntax
checks plus the upstream CodexPro smoke. Patch dry-run against a freshly packed
`codexpro@0.30.0` passed, and the live installer reported `already applied` then
`✓ smoke test passed`.

## Standard graphical QA

`docs/QA_AGENT_PROTOCOL.md` is the canonical client-visible QA contract. `spawn_pi_qa_detached` uses Luna and requires:

1. the exact requested scenario;
2. a real graphical Minecraft Java 26.1.2 client under Xvfb;
3. server/client/result/screenshot evidence as appropriate;
4. one bounded adversarial exploratory pass;
5. severity-ranked criticism even when the requested scenario passes.

The infrastructure smoke itself proved the critical path instead of returning a ceremonial PASS. Run `.ai-bridge/pi-agents/20260829015734-1ec74354` reached Play under Xvfb and captured graphical evidence, then reported `QA_RESULT=CHANGES`, including 14 authoritative `SweptCollision` corrections during ordinary sprint+jump movement. `docs/PUBLIC_ALPHA3_PLAN.md` therefore reopens the collision P0 and its final real-client closeout row.

## Pro whole-core planning audit

Versioned canonical inputs:

- `docs/agents/PRO_CORE_AUDIT_PROMPT.md`;
- `docs/agents/PRO_CORE_AUDIT_RUNBOOK.md`.

`just pro-audit-context` refreshes ignored `.ai-bridge` convenience copies and generates `.ai-bridge/pro-context.md`. Current bundle generation passes at 906,778 bytes (45 files included, 23 skipped, intentionally truncated within the active CodexPro 1 MB write ceiling).

The Pro mission is planning-only: audit the complete core across protocol/parity, world/persistence, simulation/entities/physics/AI, worldgen, performance/concurrency, plugins/Luau/Loader, trust/security, operator UX/observability, QA/fuzz/benchmarks, portability/release and maintainability. It must produce evidence-backed findings, finite agent checkpoints, a dependency graph, parallel execution waves and one self-contained `FIRST HANDOFF`.

After reviewing the complete Pro response, only that first execution wave is installed:

```bash
python3 tools/apply-pro-first-handoff.py .ai-bridge/pro-core-audit-response.md
```

The helper was syntax-checked and tested against a synthetic multi-section Pro response; it extracts only `FIRST HANDOFF` up to `DEFERRED / DO NOT DO` and passes that finite plan through `codexpro pro-apply`.

## Independent review

One detached Pi/Luna read-only infrastructure review returned `CHANGES` with four findings. No second reviewer was run after fixes, per repository policy.

Reviewer findings and disposition:

1. **Pro inputs/evidence existed only in ignored `.ai-bridge`** — fixed by adding versioned canonical prompt/runbook under `docs/agents/`, refreshing convenience copies from the tracked sources, and adding the bounded prior-Pi evidence manifest to new boxes.
2. **Dirty seeding allowed external symlink targets** — fixed and failure-injected as described above.
3. **Spawn failure could leave forum/box state behind** — fixed with exception-safe forum revoke + h5i box abort; the external-symlink failure test exercised this path.
4. **`wait_pi_agent` was snapshot-only** — fixed with bounded filesystem-event waiting; the final lifecycle smoke completed through one `max_wait_seconds=60` wait.

No production/gameplay code was changed as part of the reviewer-fix pass.
