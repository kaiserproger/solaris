# Solaris Project Index

Solaris is one Rust 1.94/Cargo workspace implementing a Minecraft Java Edition
26.1-compatible server. The client mod and tools in this repository support
testing; they are not separate repositories.

| Surface | Current owner | Focused evidence | Canonical docs | Memory route |
| --- | --- | --- | --- | --- |
| Play packets and player sessions | `crates/mc-net/src/play/` | `crates/mc-net` tests and `crates/mc-test-harness/tests/` | `docs/playable/ACTIVE.md`, ADR 0006 | [`domains/gameplay.md`](domains/gameplay.md) |
| World/chunks/save/light | `crates/mc-world/`, coordinated by `mc-net` simulation | `mc-world` tests plus persistence/client harness paths | ADR 0004 and `docs/CORE_INTERNALS_FOR_OWNER.md` | [`domains/architecture.md`](domains/architecture.md) |
| Entities, AI, ECS, regional ownership | `crates/mc-entity/` and `crates/mc-net/src/play/simulation*` | focused entity/net tests; measured benchmarks only for performance claims | ADR 0004/0005 | [`domains/architecture.md`](domains/architecture.md) |
| Lua host and plugin adapters | `crates/mc-script/` and `crates/mc-net/src/script/` plus play/session endpoints | `mc-script` tests and `crates/mc-test-harness/tests/plugin_*.rs` | `docs/PLUGINS.md`, ADR 0006 | [`domains/plugins.md`](domains/plugins.md) |
| Protocol/NBT/data/worldgen/physics | matching `mc-protocol`, `mc-nbt`, `mc-data`, `mc-worldgen`, `mc-physics` crates | matching crate tests and exact harness/oracle path | ADR index and `docs/CORE_INTERNALS_FOR_OWNER.md` | [`domains/architecture.md`](domains/architecture.md) |
| Client automation/MCP | `client-mod/solaris-client-agent/` and `tools/run-minecraft-client-mcp.sh` | client-mod tests and named real-client scenario | `docs/AGENT_TOOLING.md` | [`workflow/goal-continuity.md`](workflow/goal-continuity.md) |

Current architecture is a staged modular monolith. Large orchestration files
still exist; ADRs describe both current authority and desired migration. Never
document a desired boundary as already implemented.
