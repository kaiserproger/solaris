# Solaris Minecraft Client MCP

This NeoForge development mod embeds an authenticated Streamable HTTP MCP
server in the real Minecraft Java 26.1.2 client. MCP hosts can inspect the
client-visible world and drive ordinary client inputs without using screenshots
as state assertions or an external command-string launcher.

## Start

Use Java 25 and choose a fresh bearer token:

```sh
export SOLARIS_CLIENT_MCP_TOKEN="$(openssl rand -hex 32)"
export SOLARIS_CLIENT_MCP_PORT=39095
tools/run-minecraft-client-mcp.sh
```

The launcher calls the fixed Gradle task
`:fabric-agent:runClientMcp`. Optional settings are:

- `SOLARIS_CLIENT_MCP_GAME_DIR`: isolated Minecraft game directory.
- `SOLARIS_CLIENT_MCP_USERNAME`: 1..16 ASCII letters, digits, or underscores.
- `SOLARIS_CLIENT_MCP_PORT`: IPv4 loopback port, default `39095`.

Validate Java, configuration, MCP transport tests, and the Gradle adapter
without launching Minecraft:

```sh
SOLARIS_CLIENT_MCP_TOKEN=local-check-token \
  tools/run-minecraft-client-mcp.sh --check
```

The same run can be started directly:

```sh
cd client-mod/solaris-client-agent
SOLARIS_CLIENT_MCP_TOKEN="$SOLARIS_CLIENT_MCP_TOKEN" \
  ./gradlew --no-configuration-cache :fabric-agent:runClientMcp
```

## Codex

Register the endpoint once. Start Codex from an environment containing the
same bearer token used by the Minecraft process:

```sh
codex mcp add minecraft-client \
  --url http://127.0.0.1:39095/mcp \
  --bearer-token-env-var SOLARIS_CLIENT_MCP_TOKEN
```

No proxy process is required. The MCP endpoint is the mod inside the Minecraft
JVM at `http://127.0.0.1:39095/mcp`.

## Smoke

The protocol smoke waits for client startup, initializes MCP, checks the tool
catalog, and calls `minecraft_observe`:

```sh
tools/minecraft-client-mcp-smoke.py
```

To test a real connection and structured world reads against Solaris:

```sh
tools/minecraft-client-mcp-smoke.py \
  --server-address 127.0.0.1:25565 \
  --exercise-input \
  --disconnect
```

## Tool Surface

Read-only tools:

- `minecraft_observe`: player, health/food, pose, inventory, active container,
  screen, target, clocks, and recent chat.
- `minecraft_read_block`: one loaded block with state, fluid, sky-light, and block-light values.
- `minecraft_wait_for_loaded_block`: wait for an applied packet event that makes
  one block's chunk client-loaded, then return that block.
- `minecraft_wait_for_block_state`: wait for an exact block ID and optional
  state properties, rechecking only after applied client state events.
- `minecraft_scan_blocks`: an inclusive loaded box with the same fields, capped at 4096 cells.
- `minecraft_list_entities`: visible entities within 128 blocks, capped at 512.
- `minecraft_read_recipe_book`: bounded recipe display IDs accepted into the
  real client's recipe book.
- `minecraft_wait_for_visible_entity`: wait for an entity type within a bounded radius.
- `minecraft_wait_for_health_below`: wait for observed player damage.
- `minecraft_wait_for_inventory`: wait for an exact item count.
- `minecraft_wait_for_visible_item`: wait for an item entity near a position.
- `minecraft_wait_for_no_visible_item`: wait for that item entity to disappear.

Controls:

- Connection: `minecraft_connect`, `minecraft_wait_for_play`,
  `minecraft_disconnect`.
- Player: `minecraft_set_hotbar_slot`, `minecraft_select_hotbar_item`,
  `minecraft_drop_selected_item`, `minecraft_navigate_to_block`, `minecraft_approach_entity`,
  `minecraft_attack_entity_until_drop_collected`, `minecraft_look`,
  `minecraft_look_at_block`, `minecraft_use_item_on`.
- Input: `minecraft_press_inputs`, `minecraft_wait_ticks`,
  `minecraft_close_screen`, `minecraft_send_chat`.
- Containers: `minecraft_quick_move_container_slot`,
  `minecraft_click_container_slot`, and `minecraft_click_container_button`;
  ordinary clicks accept primary or secondary input and also confirm a
  server-side close/reopen, while quick moves and buttons require the active
  container state ID to advance.
- Regression: `minecraft_run_scenario` runs an existing deterministic
  in-client scenario and returns its structured report through MCP.
- Optional visual context: `minecraft_screenshot`.

Item observations include exact enchantment IDs and levels. All Minecraft reads execute on the client thread. Applied packets and client
login/logout lifecycle events wake state waits; client ticks use a separate
condition and wake only tick-driven input, movement, and duration waits.
Timeouts only fail stalled operations. Inventory and entity waits sample once,
then block until the producer publishes a state change. Held inputs advance on
client tick events and release every pressed key in `finally`. World tools expose only
loaded client-visible state and fail on unloaded positions; they do not reveal
hidden server state. The endpoint binds to `127.0.0.1`, requires bearer auth,
validates `Host`/`Origin`, limits request bodies, and serializes MCP tool calls.

`minecraft_navigate_to_block` accepts `x`, `y`, `z`, and optional
`timeout_seconds` (0.1 to 120, default 8). It only targets a client-loaded
block within 48 horizontal and 8 vertical blocks of the player. It uses
ordinary movement inputs and collision detours, waits on client tick events,
and succeeds only after the client observes the player grounded and
collision-free within 1.5 horizontal blocks and 1.25 vertical blocks of the
target. Invalid, unloaded, blocked, and timed-out targets return errors.

The existing `/rpc` client-agent bridge remains available for the historical
real-client regression runner and can be launched independently with
`:fabric-agent:runClientAgent`.
