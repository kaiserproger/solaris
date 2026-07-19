# In-Client Minecraft MCP Server Design

Quality label: `stabilization`.

## Goal

Embed a reusable Streamable HTTP MCP server in the Solaris NeoForge client mod
so an MCP host can inspect and control the real Minecraft 26.1.2 client without
treating screenshots or scenario log strings as authoritative state.

## Architecture

The MCP endpoint runs inside the Minecraft JVM and binds only to
`127.0.0.1`. `bridge-core` owns MCP lifecycle, JSON-RPC, authentication,
tool schemas, bounded argument validation, and command dispatch. It has no
Minecraft imports. `MinecraftClientFacade` owns Minecraft-specific reads and
actions; every client-state access executes on the Minecraft client thread.

The existing `/rpc` regression bridge remains available and unchanged for old
artifact runners. A new `runClientMcp` Gradle task starts the same injected mod
with MCP-specific token, port, game directory, and username properties. Codex
connects directly to `http://127.0.0.1:<port>/mcp` as a Streamable HTTP MCP
server. No command-string client launcher is introduced.

## Protocol And Security

- Support MCP protocol versions `2025-11-25` and `2025-06-18` for the tools
  subset used here.
- Implement `initialize`, `notifications/initialized`, `ping`, `tools/list`,
  `tools/call`, and session `DELETE` on one `/mcp` endpoint.
- Return JSON responses directly; GET returns `405` because this server sends
  no unsolicited SSE messages.
- Require `Authorization: Bearer <token>` using a nonblank runtime token.
- Reject non-loopback `Origin` and invalid `Host` values; cap request bodies.
- Assign cryptographically random session ids and require them after
  initialization.
- Serialize tool execution so concurrent MCP calls cannot race client input.

## Tools

Observation tools are read-only and return `structuredContent`:

- `minecraft_observe`: player state, pose, health/food, selected item, screen,
  active container, inventory, target, time, and recent chat.
- `minecraft_read_block`: one loaded block with state properties and fluid.
- `minecraft_scan_blocks`: bounded loaded block box, maximum 4096 cells.
- `minecraft_list_entities`: bounded client-visible entity list, maximum 512.
- `minecraft_wait_for_inventory`, `minecraft_wait_for_visible_item`, and
  `minecraft_wait_for_no_visible_item`: event-driven state gates; their timeout
  is failure only.

Control tools:

- `minecraft_connect`, `minecraft_wait_for_play`, `minecraft_disconnect`.
- `minecraft_set_hotbar_slot`, `minecraft_select_hotbar_item`,
  `minecraft_drop_selected_item`, `minecraft_look`,
  `minecraft_look_at_block`, `minecraft_use_item_on`.
- `minecraft_press_inputs`: bounded simultaneous vanilla key inputs for up to
  five seconds, always released in `finally`.
- `minecraft_wait_ticks`, `minecraft_close_screen`, `minecraft_send_chat`.
- `minecraft_run_scenario`: execute an existing deterministic in-client
  regression and return its structured observations through MCP.

`minecraft_screenshot` remains optional visual context. It is never required
to prove inventory, blocks, entities, connectivity, or persistence.

## World Visibility Contract

The MCP server reports the world visible to the real client, not hidden server
state. Queries fail closed for unloaded positions. Large world reads are
paginated by caller-chosen bounded boxes rather than one unbounded dump. Entity
and block observations include enough stable identifiers and coordinates for
an MCP host to compare successive reads deterministically.

Applied packets plus login/logout lifecycle events publish the state condition.
Client ticks publish a separate condition used only by tick-driven input,
movement, and use-duration operations. State waits never wake merely because a
tick elapsed.

## Failure Semantics

Protocol/shape errors use JSON-RPC errors. A valid tool call that cannot run
because the client is not in Play, a position is unloaded, or Minecraft rejects
an action returns an MCP tool result with `isError=true` and structured error
content. The MCP server remains alive after tool failures. Client shutdown
closes both MCP and legacy bridge executors.

## Verification

- RED/GREEN bridge-core tests cover initialization, auth, origin/session
  validation, tool discovery/call, notifications, delete, and error results.
- RED/GREEN client-command tests cover bounded world queries and input release
  dispatch without holding the Minecraft client thread during waits.
- Java compile/tests verify the patched 26.1.2 client API.
- A live `runClientMcp` smoke initializes from a real MCP client, lists tools,
  calls `minecraft_observe`, connects to Solaris, and reads blocks/entities.
- Existing Gradle tests and real-client regression runner remain green.
