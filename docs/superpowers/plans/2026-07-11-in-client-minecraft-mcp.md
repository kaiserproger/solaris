# In-Client Minecraft MCP Server Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Embed a reusable, authenticated Streamable HTTP MCP server in the real Minecraft client mod with structured world observation and typed controls.

**Architecture:** A Minecraft-free MCP transport/tool layer in `bridge-core` dispatches to the existing `ClientFacade`; the NeoForge-backed facade performs bounded reads/actions on the client thread. A dedicated Gradle run configuration injects runtime credentials and launches the ordinary client.

**Tech Stack:** Java 25, Gson, JDK `HttpServer`, NeoForge ModDev, JUnit 5, MCP Streamable HTTP 2025-11-25/2025-06-18.

## Global Constraints

- Bind only to IPv4 loopback and require a nonblank bearer token.
- Preserve the existing `/rpc` bridge and real-client runner.
- No unbounded world/entity scans and no screenshot-based assertions.
- No Minecraft types in `bridge-core`.
- All Minecraft state access runs on the client thread; timed input sleeps do not.

---

### Task 1: MCP Transport And Tool Dispatch

**Files:**
- Create: `client-mod/solaris-client-agent/bridge-core/src/main/java/dev/solaris/agent/mcp/McpHttpServer.java`
- Create: `client-mod/solaris-client-agent/bridge-core/src/main/java/dev/solaris/agent/mcp/McpToolDefinition.java`
- Create: `client-mod/solaris-client-agent/bridge-core/src/main/java/dev/solaris/agent/mcp/ClientMcpTools.java`
- Test: `client-mod/solaris-client-agent/bridge-core/src/test/java/dev/solaris/agent/mcp/McpHttpServerTest.java`

**Interfaces:**
- Produces: `McpHttpServer.start(String token, int port, CommandRegistry commands, List<McpToolDefinition> tools)`.
- Produces: `ClientMcpTools.definitions()` mapping public MCP names to existing command names and JSON schemas.

- [x] Write failing HTTP tests for auth, initialize/session headers, tools/list, tools/call, notification `202`, origin rejection, session delete, and `isError` tool results.
- [x] Run `./gradlew :bridge-core:test --tests dev.solaris.agent.mcp.McpHttpServerTest` and verify RED because MCP classes do not exist.
- [x] Implement the minimal sessionful JSON-only Streamable HTTP subset and serialized command dispatcher.
- [x] Rerun the focused bridge-core tests and verify GREEN.

### Task 2: Structured Client Observation And Input Commands

**Files:**
- Modify: `client-mod/solaris-client-agent/bridge-core/src/main/java/dev/solaris/agent/client/ClientFacade.java`
- Modify: `client-mod/solaris-client-agent/bridge-core/src/main/java/dev/solaris/agent/client/ClientCommands.java`
- Modify: `client-mod/solaris-client-agent/bridge-core/src/test/java/dev/solaris/agent/bridge/ClientCommandsTest.java`
- Modify: `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/MinecraftClientFacade.java`

**Interfaces:**
- Produces commands: `observe`, `read_block`, `scan_blocks`, `list_entities`, `press_inputs`, and `send_chat`.
- Consumes bounded JSON payloads defined by `ClientMcpTools`.

- [x] Add failing command tests for client-thread observation, 4096-cell scan cap, entity cap/radius, allowed input keys, five-second duration cap, and chat bounds.
- [x] Run focused `ClientCommandsTest` and verify RED on missing facade/commands.
- [x] Implement default facade contracts, strict command validation, and Gson serialization.
- [x] Implement Minecraft 26.1.2 observation/input adapters using loaded client state only.
- [x] Run bridge-core and java-agent tests and verify GREEN.

### Task 3: In-Mod Lifecycle And Gradle Adapter

**Files:**
- Create: `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/ClientMcpConfig.java`
- Modify: `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/SolarisClientAgent.java`
- Modify: `client-mod/solaris-client-agent/fabric-agent/build.gradle.kts`
- Test: `client-mod/solaris-client-agent/java-agent/src/test/java/dev/solaris/agent/javaagent/ClientMcpConfigTest.java`
- Test: `crates/mc-test-harness/tests/real_client_manifest.rs`

**Interfaces:**
- Runtime properties: `solaris.clientMcp.token`, `solaris.clientMcp.port`, `solaris.clientMcp.runDir`.
- Gradle task: `:fabric-agent:runClientMcp`.

- [x] Add RED config/Gradle contract tests for an MCP-only launch and mandatory token/port/gameDir validation.
- [x] Implement MCP lifecycle alongside the legacy bridge and add the `clientMcp` run configuration.
- [x] Run focused Java and Rust manifest tests and verify GREEN.

### Task 4: Reusable Launch, Registration, And Live Smoke

**Files:**
- Create: `tools/run-minecraft-client-mcp.sh`
- Modify: `docs/AGENT_TOOLING.md`
- Modify: `client-mod/solaris-client-agent/README.md`

**Interfaces:**
- Launch: `SOLARIS_CLIENT_MCP_TOKEN=... bash tools/run-minecraft-client-mcp.sh`.
- Register: `codex mcp add minecraft-client --url http://127.0.0.1:39095/mcp --bearer-token-env-var SOLARIS_CLIENT_MCP_TOKEN`.

- [x] Add a fail-closed shell `--check` path that validates Java, token, port, Gradle task, and loopback endpoint configuration without launching Minecraft.
- [x] Implement launch and operator docs without an injectable command hook.
- [x] Run all Gradle tests and `--check`.
- [x] Launch `runClientMcp`, initialize MCP, list tools, call observation/connect/world tools against a local Solaris server, and stop both processes cleanly.
- [x] Run `cargo fmt --all -- --check`, `cargo run -p xtask -- code-health`, focused Rust tests, and `git diff --check`.
