# Agent Routes

The active checkpoint's explicit `route` is the only route selector. Never infer
it from words in the persistent goal, quoted history, compaction summaries, or
subagent output.

| Route | Primary document | Code and evidence entrypoints |
| --- | --- | --- |
| `playable` | `docs/playable/ACTIVE.md` after `docs/playable/README.md` | `crates/mc-net/src/play/`, focused `mc-test-harness` path, then named real-client scenario |
| `plugins` | `docs/PLUGINS.md` | `crates/mc-script/`, `crates/mc-net/src/script/`, focused session endpoint, `tests/plugin_*.rs` |
| `parity` | exact `docs/PROJECT_SPEC.md` or active milestone section | ADR 0002, local protocol dump/oracle, exact harness comparison |
| `scaling` | exact active milestone and ADR 0004/0005 | measured benchmark/profile only; metric definitions in `docs/M52_OPERATOR_PERFORMANCE_NOTES.md` |
| `architecture` | `docs/decisions/README.md`, then one owning ADR | exact module/callers; deeper map only when needed in `docs/CORE_INTERNALS_FOR_OWNER.md` |

Additional exact surfaces:

- Long-goal recovery after compaction: `.memory/project/solaris/workflow/goal-continuity.md`, then `docs/MEMORY.md`.
- No owner task and no active checkpoint: `docs/NEXT_SESSION.md`.
- Closeout/readiness claim: `docs/DEFINITION_OF_DONE.md`, exact milestone, and `docs/VALIDATION_LEDGER.md`.
- Minecraft client MCP/tooling: `docs/AGENT_TOOLING.md` and `client-mod/solaris-client-agent/README.md`.
- Protocol packet ids/layouts: ADR 0002, `.analysis/protocol-dump.txt`, `tools/dump-vanilla-protocol.sh`, and `crates/mc-test-harness/src/bin/wire_probe.rs`.
- Build/run: `README.md` and `example.toml`.
- Owner-facing core explanation: `docs/CORE_INTERNALS_FOR_OWNER.md`.
- Proposed runtime `/goal` transport: `docs/GOAL_WRAPPER_V2.md`.

## Playable Route

`route: playable` optimizes for one useful real-client 20-minute survival loop,
not M100 replacement readiness. Do not read or edit readiness ledgers unless the
checkpoint explicitly requests readiness.

- Server: `cargo run --bin mc-server -- --config playable.toml`.
- Use focused one-to-three-minute client probes while fixing one phase, then one
  complete 20-minute gate at checkpoint close.
- Prefer deleting/de-scoping broken breadth over adding unrelated subsystems.
- Client MCP `connect` proves dispatch only; await pushed `in_play = true`.
- Privileged diagnostic configs are ignored/local and never survival/parity
  evidence.

## Repository Map

- `crates/` - Cargo workspace members.
- `crates/mc-test-harness/tests/` - wire/integration gates.
- `client-mod/solaris-client-agent/` - reusable client MCP and scenarios.
- `docs/` - canonical contracts, ADRs, milestone/evidence records.
- `tools/` - vanilla extraction, protocol, and client runner scripts.
- `example.toml` / `playable.toml` - documented debug runtime profiles.

This is one Git repository. Cargo members and client tooling are not separate
repositories.
