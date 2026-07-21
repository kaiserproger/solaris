package dev.solaris.agent.mcp;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;

import java.util.List;

public final class ClientMcpTools {
    private static final List<String> BLOCK_FACES = List.of(
        "down", "up", "north", "south", "west", "east"
    );
    private static final List<String> INPUT_KEYS = List.of(
        "forward", "back", "left", "right", "jump", "sneak", "sprint", "attack", "use",
        "swap_offhand"
    );
    private static final List<McpToolDefinition> DEFINITIONS = List.of(
        readOnly(
            "minecraft_observe",
            "Read the current client-visible player, inventory, target, screen, time, and recent chat state.",
            "observe",
            objectSchema(properties(), List.of())
        ),
        readOnly(
            "minecraft_read_block",
            "Read one loaded client-visible block with state, fluid, sky-light, and block-light values.",
            "read_block",
            blockPositionSchema()
        ),
        readOnly(
            "minecraft_wait_for_loaded_block",
            "Wait for one block's chunk to become client-loaded, waking on applied packet events.",
            "wait_loaded_block",
            objectSchema(
                properties(
                    "x", integer(),
                    "y", integer(),
                    "z", integer(),
                    "timeout_seconds", number(0.1, 120.0, 30.0)
                ),
                List.of("x", "y", "z")
            )
        ),
        new McpToolDefinition(
            "minecraft_wait_for_block_state",
            "Wait for a loaded block to match an exact id and optional state properties, waking only on applied client state events.",
            "wait_loaded_block",
            blockStateWaitSchema(),
            true,
            false,
            true,
            true,
            McpToolDefinition.Execution.WAIT_FOR_BLOCK_STATE
        ),
        readOnly(
            "minecraft_scan_blocks",
            "Read a bounded inclusive box of loaded client-visible blocks, never more than 4096 cells.",
            "scan_blocks",
            scanBlocksSchema()
        ),
        readOnly(
            "minecraft_list_entities",
            "List bounded client-visible entities near the player with stable ids, types, and positions.",
            "list_entities",
            listEntitiesSchema()
        ),
        readOnly(
            "minecraft_read_recipe_book",
            "Read bounded recipe display ids accepted by the real client.",
            "recipe_book",
            objectSchema(
                properties("limit", integer(1, 8192, 2048)),
                List.of()
            )
        ),
        readOnly(
            "minecraft_wait_for_visible_entity",
            "Wait for an entity type to become client-visible within a bounded radius, waking only on client state events.",
            "wait_visible_entity",
            visibleEntityWaitSchema()
        ),
        readOnly(
            "minecraft_wait_for_entity_motion",
            "Wait for one UUID- and type-fenced entity to move, returning bounded aggregate motion fields.",
            "wait_entity_motion",
            entityMotionWaitSchema()
        ),
        readOnly(
            "minecraft_wait_for_entity_removed",
            "Wait for one UUID- and type-fenced entity to leave client-visible state.",
            "wait_entity_removed",
            entityRemovedWaitSchema()
        ),
        readOnly(
            "minecraft_wait_for_health_below",
            "Wait for client-visible player health to fall below a threshold, waking only on client state events.",
            "wait_health_below",
            healthWaitSchema()
        ),
        readOnly(
            "minecraft_wait_for_inventory",
            "Wait for an exact client inventory count, waking only on client state events.",
            "wait_inventory",
            inventoryWaitSchema()
        ),
        readOnly(
            "minecraft_wait_for_visible_item",
            "Wait for an item entity to become client-visible near a block position.",
            "wait_visible_item",
            itemVisibilityWaitSchema()
        ),
        readOnly(
            "minecraft_wait_for_no_visible_item",
            "Wait for an item entity to stop being client-visible near a block position.",
            "wait_no_visible_item",
            itemVisibilityWaitSchema()
        ),
        nonIdempotentControl(
            "minecraft_connect",
            "Connect the real Minecraft client to a server.",
            "connect",
            connectSchema()
        ),
        readOnly(
            "minecraft_wait_for_play",
            "Wait until the client reaches an active world or the bounded timeout expires.",
            "wait_play",
            objectSchema(
                properties("timeout_seconds", number(0.1, 3600.0, 30.0)),
                List.of()
            )
        ),
        readOnly(
            "minecraft_wait_for_state_change",
            "Wait for the next exact client lifecycle or packet state event.",
            "wait_state_change",
            objectSchema(
                properties(
                    "observed_version", integer(0, Integer.MAX_VALUE),
                    "timeout_seconds", number(0.1, 120.0, 30.0)
                ),
                List.of("observed_version")
            )
        ),
        mutating(
            "minecraft_disconnect",
            "Disconnect the real Minecraft client from its current server.",
            "disconnect",
            objectSchema(properties(), List.of())
        ),
        control(
            "minecraft_set_hotbar_slot",
            "Select a zero-based hotbar slot.",
            "set_hotbar_slot",
            objectSchema(properties("slot", integer(0, 8)), List.of("slot"))
        ),
        mutating(
            "minecraft_select_hotbar_item",
            "Move a matching inventory stack into the hotbar and select it.",
            "select_hotbar_item",
            objectSchema(
                properties(
                    "item_id", string(128),
                    "count", integer(1, 64),
                    "timeout_seconds", number(0.1, 120.0, 8.0)
                ),
                List.of("item_id", "count")
            )
        ),
        mutating(
            "minecraft_navigate_to_block",
            "Walk to a loaded client-visible block within a bounded route, returning only after observed grounded arrival near the target.",
            "navigate_to_block",
            blockNavigationSchema()
        ),
        mutating(
            "minecraft_approach_entity",
            "Approach one client-visible entity through ordinary movement until it is in survival reach.",
            "approach_entity",
            approachEntitySchema()
        ),
        mutating(
            "minecraft_interact_entity",
            "Dispatch the vanilla entity interaction for one UUID- and type-fenced entity and return the local interaction result without inferring server gameplay success.",
            "interact_entity",
            entityInteractionSchema()
        ),
        mutating(
            "minecraft_attack_entity_once",
            "Dispatch one ordinary vanilla attack against a UUID- and type-fenced visible entity.",
            "attack_entity_once",
            entityIdentitySchema()
        ),
        mutating(
            "minecraft_attack_entity_until_drop_collected",
            "Attack one client-visible entity and collect its expected visible item drop.",
            "attack_entity_until_drop_collected",
            attackEntitySchema()
        ),
        control(
            "minecraft_look",
            "Set the player's absolute yaw and pitch in degrees.",
            "look",
            objectSchema(
                properties(
                    "yaw_deg", integer(-180, 180),
                    "pitch_deg", integer(-90, 90)
                ),
                List.of("yaw_deg", "pitch_deg")
            )
        ),
        control(
            "minecraft_look_at_block",
            "Aim at a face of a loaded client-visible block.",
            "look_at_block",
            blockFaceSchema()
        ),
        mutating(
            "minecraft_use_item_on",
            "Use the selected item on a face of a loaded client-visible block.",
            "use_item_on",
            blockFaceSchema()
        ),
        mutating(
            "minecraft_press_inputs",
            "Activate vanilla client inputs for exact client ticks; swap_offhand clicks once.",
            "press_inputs",
            pressInputsSchema()
        ),
        readOnly(
            "minecraft_wait_ticks",
            "Wait for a bounded number of real client ticks without blocking the client thread.",
            "wait_ticks",
            objectSchema(properties("ticks", integer(1, 255)), List.of("ticks"))
        ),
        control(
            "minecraft_close_screen",
            "Close the current client GUI screen or container.",
            "close_screen",
            objectSchema(properties(), List.of())
        ),
        control(
            "minecraft_open_inventory",
            "Open the vanilla survival inventory and 2x2 crafting screen.",
            "open_inventory",
            objectSchema(properties(), List.of())
        ),
        nonIdempotentControl(
            "minecraft_respawn",
            "Send the vanilla respawn action and wait for confirmed active player state.",
            "respawn",
            objectSchema(
                properties("timeout_seconds", number(0.1, 120.0, 10.0)),
                List.of()
            )
        ),
        mutating(
            "minecraft_quick_move_container_slot",
            "Quick-move one visible menu slot and wait for the server-confirmed container state update.",
            "quick_move_container_slot",
            objectSchema(
                properties(
                    "slot", integer(0, Short.MAX_VALUE),
                    "timeout_seconds", number(0.1, 120.0, 8.0)
                ),
                List.of("slot")
            )
        ),
        mutating(
            "minecraft_click_container_slot",
            "Primary- or secondary-click one visible menu slot and wait for a server-confirmed container update.",
            "click_container_slot",
            objectSchema(
                properties(
                    "slot", integer(0, Short.MAX_VALUE),
                    "button", enumString(List.of("primary", "secondary")),
                    "timeout_seconds", number(0.1, 120.0, 8.0)
                ),
                List.of("slot", "button")
            )
        ),
        mutating(
            "minecraft_click_container_button",
            "Click one menu button and wait for the server-confirmed container state update.",
            "click_container_button",
            objectSchema(
                properties(
                    "button_id", integer(0, Integer.MAX_VALUE),
                    "timeout_seconds", number(0.1, 120.0, 8.0)
                ),
                List.of("button_id")
            )
        ),
        nonIdempotentControl(
            "minecraft_send_chat",
            "Send a chat message or command through the current client connection.",
            "send_chat",
            objectSchema(
                properties(
                    "message", string(256),
                    "command", bool(false)
                ),
                List.of("message")
            )
        ),
        mutating(
            "minecraft_drop_selected_item",
            "Drop an exact selected stack and wait for its confirmed inventory debit and visible item entity.",
            "drop_selected_item",
            objectSchema(
                properties(
                    "item_id", string(128),
                    "count", integer(1, 64),
                    "timeout_seconds", number(0.1, 120.0, 8.0)
                ),
                List.of("item_id", "count")
            )
        ),
        mutating(
            "minecraft_run_scenario",
            "Run one deterministic in-client regression scenario and return its structured observations.",
            "run_scenario",
            objectSchema(
                properties(
                    "id", string(128),
                    "artifacts_dir", string(1024)
                ),
                List.of("id")
            )
        ),
        mutating(
            "minecraft_screenshot",
            "Capture optional visual context to a path inside a screenshots directory.",
            "screenshot",
            objectSchema(properties("path", string(1024)), List.of("path"))
        )
    );

    private ClientMcpTools() {
    }

    public static List<McpToolDefinition> definitions() {
        return DEFINITIONS;
    }

    private static McpToolDefinition readOnly(
        String name,
        String description,
        String command,
        JsonObject schema
    ) {
        return new McpToolDefinition(name, description, command, schema, true, false, true, true);
    }

    private static McpToolDefinition control(
        String name,
        String description,
        String command,
        JsonObject schema
    ) {
        return new McpToolDefinition(name, description, command, schema, false, false, true, true);
    }

    private static McpToolDefinition nonIdempotentControl(
        String name,
        String description,
        String command,
        JsonObject schema
    ) {
        return new McpToolDefinition(name, description, command, schema, false, false, false, true);
    }

    private static McpToolDefinition mutating(
        String name,
        String description,
        String command,
        JsonObject schema
    ) {
        return new McpToolDefinition(name, description, command, schema, false, true, false, true);
    }

    private static JsonObject blockPositionSchema() {
        return objectSchema(
            properties(
                "x", integer(),
                "y", integer(),
                "z", integer()
            ),
            List.of("x", "y", "z")
        );
    }

    private static JsonObject blockFaceSchema() {
        return objectSchema(
            properties(
                "x", integer(),
                "y", integer(),
                "z", integer(),
                "face", enumString(BLOCK_FACES)
            ),
            List.of("x", "y", "z", "face")
        );
    }

    private static JsonObject blockNavigationSchema() {
        return objectSchema(
            properties(
                "x", integer(),
                "y", integer(),
                "z", integer(),
                "timeout_seconds", number(0.1, 120.0, 8.0)
            ),
            List.of("x", "y", "z")
        );
    }

    private static JsonObject blockStateWaitSchema() {
        JsonObject stateProperties = new JsonObject();
        stateProperties.addProperty("type", "object");
        stateProperties.addProperty("maxProperties", 32);
        JsonObject stateValue = string(128);
        stateProperties.add("additionalProperties", stateValue);

        return objectSchema(
            properties(
                "x", integer(),
                "y", integer(),
                "z", integer(),
                "block_id", string(128),
                "properties", stateProperties,
                "timeout_seconds", number(0.1, 120.0, 8.0)
            ),
            List.of("x", "y", "z", "block_id")
        );
    }

    private static JsonObject scanBlocksSchema() {
        return objectSchema(
            properties(
                "min_x", integer(),
                "min_y", integer(),
                "min_z", integer(),
                "max_x", integer(),
                "max_y", integer(),
                "max_z", integer(),
                "max_blocks", integer(1, 4096, 4096)
            ),
            List.of("min_x", "min_y", "min_z", "max_x", "max_y", "max_z")
        );
    }

    private static JsonObject listEntitiesSchema() {
        return objectSchema(
            properties(
                "radius", number(0.0, 128.0, 32.0),
                "limit", integer(1, 512, 128)
            ),
            List.of()
        );
    }

    private static JsonObject inventoryWaitSchema() {
        return objectSchema(
            properties(
                "item_id", string(128),
                "count", integer(0, 4096),
                "timeout_seconds", number(0.1, 120.0, 8.0)
            ),
            List.of("item_id", "count")
        );
    }

    private static JsonObject visibleEntityWaitSchema() {
        return objectSchema(
            properties(
                "entity_type", string(128),
                "radius", number(0.0, 128.0, 32.0),
                "timeout_seconds", number(0.1, 120.0, 8.0)
            ),
            List.of("entity_type")
        );
    }

    private static JsonObject entityMotionWaitSchema() {
        return objectSchema(
            properties(
                "entity_id", integer(0, Integer.MAX_VALUE),
                "entity_uuid", string(36),
                "entity_type", string(128),
                "minimum_horizontal_distance", number(0.001, 128.0, 0.01),
                "minimum_vertical_rise", number(0.0, 128.0, 0.0),
                "timeout_seconds", number(0.1, 120.0, 8.0)
            ),
            List.of("entity_id", "entity_uuid", "entity_type")
        );
    }

    private static JsonObject entityRemovedWaitSchema() {
        return objectSchema(
            properties(
                "entity_id", integer(0, Integer.MAX_VALUE),
                "entity_uuid", string(36),
                "entity_type", string(128),
                "timeout_seconds", number(0.1, 120.0, 8.0)
            ),
            List.of("entity_id", "entity_uuid", "entity_type")
        );
    }

    private static JsonObject approachEntitySchema() {
        return objectSchema(
            properties(
                "entity_id", integer(0, Integer.MAX_VALUE),
                "timeout_seconds", number(0.1, 120.0, 30.0)
            ),
            List.of("entity_id")
        );
    }

    private static JsonObject entityInteractionSchema() {
        JsonObject hand = enumString(List.of("main_hand", "off_hand"));
        hand.addProperty("default", "main_hand");
        return objectSchema(
            properties(
                "entity_id", integer(0, Integer.MAX_VALUE),
                "entity_uuid", string(36),
                "entity_type", string(128),
                "hand", hand
            ),
            List.of("entity_id", "entity_uuid", "entity_type")
        );
    }

    private static JsonObject entityIdentitySchema() {
        return objectSchema(
            properties(
                "entity_id", integer(0, Integer.MAX_VALUE),
                "entity_uuid", string(36),
                "entity_type", string(128)
            ),
            List.of("entity_id", "entity_uuid", "entity_type")
        );
    }

    private static JsonObject attackEntitySchema() {
        return objectSchema(
            properties(
                "entity_id", integer(0, Integer.MAX_VALUE),
                "expected_drop_item_id", string(128),
                "expected_drop_count", integer(1, 64),
                "timeout_seconds", number(0.1, 120.0, 30.0)
            ),
            List.of("entity_id", "expected_drop_item_id", "expected_drop_count")
        );
    }

    private static JsonObject healthWaitSchema() {
        return objectSchema(
            properties(
                "health", number(0.001, 2048.0),
                "timeout_seconds", number(0.1, 120.0, 8.0)
            ),
            List.of("health")
        );
    }

    private static JsonObject itemVisibilityWaitSchema() {
        return objectSchema(
            properties(
                "item_id", string(128),
                "x", integer(),
                "y", integer(),
                "z", integer(),
                "timeout_seconds", number(0.1, 120.0, 8.0)
            ),
            List.of("item_id", "x", "y", "z")
        );
    }

    private static JsonObject connectSchema() {
        return objectSchema(
            properties(
                "server_addr", string(512),
                "host", string(253),
                "port", integer(1, 65_535)
            ),
            List.of()
        );
    }

    private static JsonObject pressInputsSchema() {
        JsonObject keys = new JsonObject();
        keys.addProperty("type", "array");
        keys.add("items", enumString(INPUT_KEYS));
        keys.addProperty("minItems", 1);
        keys.addProperty("maxItems", 8);
        keys.addProperty("uniqueItems", true);
        return objectSchema(
            properties(
                "keys", keys,
                "ticks", integer(1, 255)
            ),
            List.of("keys", "ticks")
        );
    }

    private static JsonObject objectSchema(JsonObject properties, List<String> required) {
        JsonObject schema = new JsonObject();
        schema.addProperty("type", "object");
        schema.add("properties", properties);
        JsonArray requiredFields = new JsonArray();
        required.forEach(requiredFields::add);
        schema.add("required", requiredFields);
        schema.addProperty("additionalProperties", false);
        return schema;
    }

    private static JsonObject properties(Object... entries) {
        JsonObject properties = new JsonObject();
        for (int index = 0; index < entries.length; index += 2) {
            properties.add((String) entries[index], (JsonObject) entries[index + 1]);
        }
        return properties;
    }

    private static JsonObject integer() {
        JsonObject schema = new JsonObject();
        schema.addProperty("type", "integer");
        return schema;
    }

    private static JsonObject integer(int minimum, int maximum) {
        JsonObject schema = integer();
        schema.addProperty("minimum", minimum);
        schema.addProperty("maximum", maximum);
        return schema;
    }

    private static JsonObject integer(int minimum, int maximum, int defaultValue) {
        JsonObject schema = integer(minimum, maximum);
        schema.addProperty("default", defaultValue);
        return schema;
    }

    private static JsonObject number(double minimum, double maximum, double defaultValue) {
        JsonObject schema = number(minimum, maximum);
        schema.addProperty("default", defaultValue);
        return schema;
    }

    private static JsonObject number(double minimum, double maximum) {
        JsonObject schema = new JsonObject();
        schema.addProperty("type", "number");
        schema.addProperty("minimum", minimum);
        schema.addProperty("maximum", maximum);
        return schema;
    }

    private static JsonObject string(int maxLength) {
        JsonObject schema = new JsonObject();
        schema.addProperty("type", "string");
        schema.addProperty("minLength", 1);
        schema.addProperty("maxLength", maxLength);
        return schema;
    }

    private static JsonObject enumString(List<String> values) {
        JsonObject schema = new JsonObject();
        schema.addProperty("type", "string");
        JsonArray choices = new JsonArray();
        values.forEach(choices::add);
        schema.add("enum", choices);
        return schema;
    }

    private static JsonObject bool(boolean defaultValue) {
        JsonObject schema = new JsonObject();
        schema.addProperty("type", "boolean");
        schema.addProperty("default", defaultValue);
        return schema;
    }
}
