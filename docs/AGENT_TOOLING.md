# Agent Tooling

This file keeps agent setup details out of milestone docs. `AGENTS.md`
contains the rules; this file contains the local wiring.

## Current Tools

| Tool | Status | Use |
|---|---|---|
| CodeGraph | installed globally via npm as `@colbymchenry/codegraph@1.2.0`; Codex MCP server `codegraph` registered; telemetry disabled; Solaris index lives in ignored `.codegraph/` | Targeted symbol graph questions: callers/callees, mutation paths, lock holders, affected tests, and blast-radius checks. Refresh with `codegraph sync .` after edits before relying on it. |
| Serena | enabled globally through opencode MCP | Symbol search/editing and project memories. Prefer it for Rust code navigation before full-file reads. |
| Context7 | enabled globally through opencode MCP and verified 2026-06-11 | External library/framework docs. Use `resolve-library-id` before `query-docs`. |
| RTK | installed at `/home/kaiserroman/.cargo/bin/rtk` | Compact shell output. OpenCode plugin installed globally at `~/.config/opencode/plugins/rtk.ts`; restart opencode before relying on auto-rewrite. |
| Headroom | installed by `uv tool install "headroom-ai[all]"` at `/home/kaiserroman/.local/bin/headroom` | Optional context compression/proxy/MCP/learning. Do not route opencode provider traffic through Headroom unless explicitly asked. |
| Agent harness | installed globally under `~/.config/opencode/bin/agent-harness` | Spec-first implementation, refactor, cleanup, and slop-review flows. Prefer native opencode slash commands over external LLM subprocesses. |
| Minecraft client MCP | embedded in the repo's NeoForge 26.1.2 development mod; loopback Streamable HTTP endpoint | Structured real-client observation, connection, inventory/entity waits, input, selected-item drop, and reusable multi-client core gates without screenshot assertions. |

## Minecraft Client MCP

Validate the fixed launcher without starting Minecraft:

```sh
SOLARIS_CLIENT_MCP_TOKEN=local-check-token \
  bash tools/run-minecraft-client-mcp.sh --check
```

Launch one client with an isolated game directory and username:

```sh
export SOLARIS_CLIENT_MCP_TOKEN="$(openssl rand -hex 32)"
export SOLARIS_CLIENT_MCP_PORT=39095
export SOLARIS_CLIENT_MCP_GAME_DIR=.analysis/minecraft-mcp-primary
export SOLARIS_CLIENT_MCP_USERNAME=SolarisMcpA
bash tools/run-minecraft-client-mcp.sh
```

Use a second port, token, game directory, and username for multiplayer gates.
The endpoint is `http://127.0.0.1:<port>/mcp`. Inventory, entity, login, and
client lifecycle waits block on packet/lifecycle state notifications. A
separate tick notification is used only when tick progression itself drives
the operation. Every timeout is failure, not a success condition.

The current bridge also provides push-driven motion and entity-removal waits
over the virtual-thread transport. Ordinary primary/secondary container-slot
clicks wait for an applied server container update, so plugin inventory menus
can be exercised without coordinate clicks. Canonical interaction checks fence
reach, raycast, and authoritative world state before reporting success. Focused
bridge, Java, and client-mod tests are tooling-path evidence only. Actual
`runClient` evidence and its remaining gameplay gaps are tracked in
`docs/playable/ACTIVE.md`.

The regression runner is fail-closed on scenario provenance. For `--check` and
`--run`, `SOLARIS_REAL_CLIENT_AGENT_SCENARIO` must name exactly one scenario in
`SOLARIS_REAL_CLIENT_MANIFEST`; an implemented but undeclared debug scenario is
not valid evidence for a no-debug playable manifest.

## Useful External Candidates

| Tool | Decision |
|---|---|
| `lean-ctx` | Researched 2026-06-11. It offers MCP, shell compression, memory, and context governance, including OpenCode setup. Do not install it on top of RTK+Headroom by default; overlapping hooks/context layers can conflict. Revisit only if RTK/Headroom are not enough. |
| Headroom bundled tools | Available through `headroom sg`, `headroom diff`, and `headroom loc` for AST search, structural diffs, and LOC/repo-shape probes. Use explicitly when they add value; do not replace normal repo validation. |

## OpenCode Commands

Global commands already present in `~/.config/opencode/commands/`:

| Command | Purpose |
|---|---|
| `/agent-harness` | Router that points to the native harness commands. |
| `/harness-run` | Spec-first implementation with native subagent cards. |
| `/harness-refactor` | Behavior-preserving refactor with review gates. |
| `/harness-cleanup` | Behavior-preserving repo slop cleanup. |
| `/harness-cleanup-cli` | Old all-in-CLI cleanup path; use only when explicitly requested. |
| `/harness-preflight` | Deterministic agent/config/repo checks. |
| `/harness-dry-run` | Generate harness prompts/artifacts without LLM phases. |

Project command added here:

| Command | Purpose |
|---|---|
| `/negative-code-review` | Read-only review of the current diff for negative-code, fake abstraction, and slop issues. |

## Headroom Notes

Headroom's own CLI help says `headroom wrap opencode` does not exist. The
supported opencode route is `headroom proxy` plus provider base URL overrides.
Do not enable that automatically in this repo because the global opencode setup
uses provider auth/plugins and a forced proxy can break model access.

Headroom MCP is not enabled in `opencode.json`. If the owner asks for it, use
`/home/kaiserroman/.local/bin/headroom mcp serve` as the local command and
check startup cost before leaving it enabled.

## Session Logs

Use both layers when prior-session evidence matters:

| Source | Command |
|---|---|
| OpenCode session list | `opencode session list` from `/home/kaiserroman/solaris` |
| OpenCode SQLite DB | `sqlite3 ~/.local/share/opencode/opencode.db` |
| Text parts | Query `part` joined with `message`/`session`; useful content is usually in `part.data` where `$.type == "text"`. |

Avoid `opencode export --sanitize` for detailed local forensic work because it
redacts the text/tool payloads that usually contain the useful facts.

## Negative-Code Gate

For any non-trivial code/config/doc change, finish with a negative-code review:

| Diff size | Gate |
|---|---|
| Small/single-file | Self-review the diff for deletion opportunities, one-use helpers, fake abstractions, wider-than-needed config, and stale docs. |
| Non-trivial or risky | Use the `harness-slop-reviewer` subagent or `/negative-code-review`. |
| Harness flow | Keep the existing `harness-slop-reviewer` and `harness-reviewer` phases; do not replace them with self-review. |

Final reports should say whether the negative-code review ran and what it found.
