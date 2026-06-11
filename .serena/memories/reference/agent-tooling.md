Agent/tooling setup as of 2026-06-11:

- OpenCode global MCP has `context7` and `serena` enabled in `~/.config/opencode/opencode.json`.
- Context7 was verified with `resolve-library-id` + `query-docs` against Tokio docs on 2026-06-11.
- Serena project memories are organized under `.serena/memories/{project,feedback,reference}` plus top-level `style-and-conventions`, `suggested_commands`, `task-completion`, and `user-profile`.
- RTK is installed at `/home/kaiserroman/.cargo/bin/rtk` from `rtk-ai/rtk` branch `develop` (`rtk 0.42.2`, commit `6785a6c7`). `rtk init -g --opencode --auto-patch` installed global OpenCode plugin at `~/.config/opencode/plugins/rtk.ts`. Restart opencode before relying on Bash auto-rewrite.
- Headroom is installed via `uv tool install "headroom-ai[all]"` at `/home/kaiserroman/.local/bin/headroom` (`headroom 0.24.0`). Headroom help says there is no `headroom wrap opencode`; opencode use requires `headroom proxy` plus provider base URL overrides, so do not route opencode through it unless the owner explicitly asks.
- Project `opencode.json` now loads `AGENTS.md`, references `rtk-ai/rtk` and `chopratejas/headroom`, and loads `.opencode/plugins/negative-code-guard.ts`. `docs/AGENT_TOOLING.md` is referenced from `AGENTS.md` but is not startup-loaded to keep context lean. Headroom MCP is not enabled by default.
- `lean-ctx` was researched on 2026-06-11 as another context/MCP/shell-compression layer with OpenCode support. Do not install it by default on top of RTK+Headroom because overlapping hooks/context layers can conflict; revisit only if RTK/Headroom are not enough.
- Headroom provides explicit wrappers `headroom sg`, `headroom diff`, and `headroom loc` for ast-grep, difftastic, and scc. Use them when useful, not as replacements for normal repo validation.
- Project command `.opencode/commands/negative-code-review.md` provides a read-only negative-code/slop review path. Non-trivial diffs should use `harness-slop-reviewer` or this command before finalizing.
- Prior opencode session evidence: start with `opencode session list`; detailed text is in `~/.local/share/opencode/opencode.db`, especially `part.data` rows where JSON `$.type == "text"`. `opencode export --sanitize` redacts useful text/tool payloads.
