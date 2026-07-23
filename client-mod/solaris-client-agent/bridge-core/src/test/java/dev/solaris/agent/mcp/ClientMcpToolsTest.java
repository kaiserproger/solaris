package dev.solaris.agent.mcp;

import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class ClientMcpToolsTest {
    @Test
    void exposesStableObservationAndControlSurface() {
        List<McpToolDefinition> tools = ClientMcpTools.definitions();

        assertEquals(List.of(
            "minecraft_observe",
            "minecraft_read_block",
            "minecraft_wait_for_loaded_block",
            "minecraft_wait_for_block_state",
            "minecraft_scan_blocks",
            "minecraft_list_entities",
            "minecraft_read_recipe_book",
            "minecraft_wait_for_visible_entity",
            "minecraft_wait_for_entity_motion",
            "minecraft_wait_for_entity_removed",
            "minecraft_wait_for_health_below",
            "minecraft_wait_for_inventory",
            "minecraft_wait_for_container_slot",
            "minecraft_wait_for_visible_item",
            "minecraft_wait_for_no_visible_item",
            "minecraft_connect",
            "minecraft_wait_for_play",
            "minecraft_wait_for_state_change",
            "minecraft_disconnect",
            "minecraft_set_hotbar_slot",
            "minecraft_select_hotbar_item",
            "minecraft_navigate_to_block",
            "minecraft_approach_entity",
            "minecraft_interact_entity",
            "minecraft_attack_entity_once",
            "minecraft_attack_entity_until_drop_collected",
            "minecraft_look",
            "minecraft_look_at_block",
            "minecraft_use_item_on",
            "minecraft_break_block",
            "minecraft_press_inputs",
            "minecraft_wait_ticks",
            "minecraft_close_screen",
            "minecraft_open_inventory",
            "minecraft_respawn",
            "minecraft_quick_move_container_slot",
            "minecraft_click_container_slot",
            "minecraft_click_container_button",
            "minecraft_send_chat",
            "minecraft_drop_selected_item",
            "minecraft_run_scenario",
            "minecraft_screenshot"
        ), tools.stream().map(McpToolDefinition::name).toList());

        assertTrue(find(tools, "minecraft_observe").readOnly());
        assertTrue(find(tools, "minecraft_read_block").readOnly());
        assertTrue(find(tools, "minecraft_wait_for_loaded_block").readOnly());
        assertTrue(find(tools, "minecraft_wait_for_block_state").readOnly());
        assertTrue(find(tools, "minecraft_scan_blocks").readOnly());
        assertTrue(find(tools, "minecraft_list_entities").readOnly());
        assertTrue(find(tools, "minecraft_read_recipe_book").readOnly());
        assertFalse(find(tools, "minecraft_connect").readOnly());
        assertFalse(find(tools, "minecraft_press_inputs").readOnly());
        assertFalse(find(tools, "minecraft_open_inventory").readOnly());
    }

    @Test
    void publishesBoundsNeededToKeepClientQueriesFinite() {
        JsonObject scan = properties(find(ClientMcpTools.definitions(), "minecraft_scan_blocks"));
        JsonObject loadedBlock = properties(find(
            ClientMcpTools.definitions(),
            "minecraft_wait_for_loaded_block"
        ));
        JsonObject blockState = properties(find(
            ClientMcpTools.definitions(),
            "minecraft_wait_for_block_state"
        ));
        JsonObject entities = properties(find(ClientMcpTools.definitions(), "minecraft_list_entities"));
        JsonObject recipeBook = properties(find(
            ClientMcpTools.definitions(),
            "minecraft_read_recipe_book"
        ));
        JsonObject visibleEntity = properties(find(
            ClientMcpTools.definitions(),
            "minecraft_wait_for_visible_entity"
        ));
        JsonObject entityMotion = properties(find(
            ClientMcpTools.definitions(),
            "minecraft_wait_for_entity_motion"
        ));
        JsonObject entityRemoved = properties(find(
            ClientMcpTools.definitions(),
            "minecraft_wait_for_entity_removed"
        ));
        JsonObject health = properties(find(
            ClientMcpTools.definitions(),
            "minecraft_wait_for_health_below"
        ));
        JsonObject inputs = properties(find(ClientMcpTools.definitions(), "minecraft_press_inputs"));
        JsonObject inventory = properties(find(ClientMcpTools.definitions(), "minecraft_wait_for_inventory"));
        JsonObject visible = properties(find(ClientMcpTools.definitions(), "minecraft_wait_for_visible_item"));
        JsonObject select = properties(find(ClientMcpTools.definitions(), "minecraft_select_hotbar_item"));
        JsonObject navigate = properties(find(ClientMcpTools.definitions(), "minecraft_navigate_to_block"));
        JsonObject approach = properties(find(ClientMcpTools.definitions(), "minecraft_approach_entity"));
        JsonObject interact = properties(find(ClientMcpTools.definitions(), "minecraft_interact_entity"));
        JsonObject attack = properties(find(
            ClientMcpTools.definitions(),
            "minecraft_attack_entity_until_drop_collected"
        ));
        JsonObject drop = properties(find(ClientMcpTools.definitions(), "minecraft_drop_selected_item"));
        JsonObject breakBlock = properties(find(ClientMcpTools.definitions(), "minecraft_break_block"));
        JsonObject useItemOn = properties(find(ClientMcpTools.definitions(), "minecraft_use_item_on"));
        JsonObject respawn = properties(find(ClientMcpTools.definitions(), "minecraft_respawn"));
        JsonObject quickMove = properties(find(
            ClientMcpTools.definitions(),
            "minecraft_quick_move_container_slot"
        ));
        JsonObject containerSlot = properties(find(
            ClientMcpTools.definitions(),
            "minecraft_click_container_slot"
        ));
        JsonObject containerButton = properties(find(
            ClientMcpTools.definitions(),
            "minecraft_click_container_button"
        ));
        JsonObject scenario = properties(find(ClientMcpTools.definitions(), "minecraft_run_scenario"));

        assertEquals(4096, scan.get("max_blocks").getAsJsonObject().get("maximum").getAsInt());
        assertEquals(
            120.0,
            loadedBlock.get("timeout_seconds").getAsJsonObject().get("maximum").getAsDouble()
        );
        assertEquals(128, blockState.get("block_id").getAsJsonObject().get("maxLength").getAsInt());
        assertEquals("object", blockState.get("properties").getAsJsonObject().get("type").getAsString());
        assertEquals(
            "string",
            blockState.get("properties").getAsJsonObject()
                .getAsJsonObject("additionalProperties")
                .get("type")
                .getAsString()
        );
        assertEquals(
            List.of("x", "y", "z", "block_id"),
            find(ClientMcpTools.definitions(), "minecraft_wait_for_block_state")
                .inputSchema()
                .getAsJsonArray("required")
                .asList()
                .stream()
                .map(element -> element.getAsString())
                .toList()
        );
        assertEquals(512, entities.get("limit").getAsJsonObject().get("maximum").getAsInt());
        assertEquals(128.0, entities.get("radius").getAsJsonObject().get("maximum").getAsDouble());
        assertEquals(8192, recipeBook.get("limit").getAsJsonObject().get("maximum").getAsInt());
        assertEquals(
            128.0,
            visibleEntity.get("radius").getAsJsonObject().get("maximum").getAsDouble()
        );
        assertEquals(
            120.0,
            visibleEntity.get("timeout_seconds").getAsJsonObject().get("maximum").getAsDouble()
        );
        assertEquals(36, entityMotion.get("entity_uuid").getAsJsonObject().get("maxLength").getAsInt());
        assertEquals(
            128.0,
            entityMotion.get("minimum_horizontal_distance").getAsJsonObject().get("maximum").getAsDouble()
        );
        assertEquals(
            128.0,
            entityMotion.get("minimum_vertical_rise").getAsJsonObject().get("maximum").getAsDouble()
        );
        assertEquals(
            List.of("entity_id", "entity_uuid", "entity_type"),
            find(ClientMcpTools.definitions(), "minecraft_wait_for_entity_removed")
                .inputSchema()
                .getAsJsonArray("required")
                .asList()
                .stream()
                .map(element -> element.getAsString())
                .toList()
        );
        assertEquals(
            120.0,
            entityRemoved.get("timeout_seconds").getAsJsonObject().get("maximum").getAsDouble()
        );
        assertEquals(2048.0, health.get("health").getAsJsonObject().get("maximum").getAsDouble());
        assertEquals(255, inputs.get("ticks").getAsJsonObject().get("maximum").getAsInt());
        assertEquals(8, inputs.get("keys").getAsJsonObject().get("maxItems").getAsInt());
        assertTrue(
            inputs.get("keys").getAsJsonObject().getAsJsonObject("items").getAsJsonArray("enum")
                .asList()
                .stream()
                .anyMatch(value -> value.getAsString().equals("swap_offhand"))
        );
        assertEquals(4096, inventory.get("count").getAsJsonObject().get("maximum").getAsInt());
        assertEquals(120.0, inventory.get("timeout_seconds").getAsJsonObject().get("maximum").getAsDouble());
        assertEquals(120.0, visible.get("timeout_seconds").getAsJsonObject().get("maximum").getAsDouble());
        assertEquals(64, select.get("count").getAsJsonObject().get("maximum").getAsInt());
        assertEquals(120.0, navigate.get("timeout_seconds").getAsJsonObject().get("maximum").getAsDouble());
        assertEquals(
            List.of("x", "y", "z"),
            find(ClientMcpTools.definitions(), "minecraft_navigate_to_block")
                .inputSchema()
                .getAsJsonArray("required")
                .asList()
                .stream()
                .map(element -> element.getAsString())
                .toList()
        );
        assertEquals(Integer.MAX_VALUE, approach.get("entity_id").getAsJsonObject().get("maximum").getAsInt());
        assertEquals(120.0, approach.get("timeout_seconds").getAsJsonObject().get("maximum").getAsDouble());
        assertEquals(36, interact.get("entity_uuid").getAsJsonObject().get("maxLength").getAsInt());
        JsonObject attackOnce = properties(find(
            ClientMcpTools.definitions(),
            "minecraft_attack_entity_once"
        ));
        assertEquals(
            120.0,
            attackOnce.get("timeout_seconds").getAsJsonObject().get("maximum").getAsDouble()
        );
        assertEquals(
            8.0,
            attackOnce.get("timeout_seconds").getAsJsonObject().get("default").getAsDouble()
        );
        assertEquals(
            List.of("main_hand", "off_hand"),
            interact.get("hand").getAsJsonObject().getAsJsonArray("enum")
                .asList()
                .stream()
                .map(value -> value.getAsString())
                .toList()
        );
        assertEquals(
            List.of("entity_id", "entity_uuid", "entity_type"),
            find(ClientMcpTools.definitions(), "minecraft_interact_entity")
                .inputSchema()
                .getAsJsonArray("required")
                .asList()
                .stream()
                .map(value -> value.getAsString())
                .toList()
        );
        assertEquals(64, attack.get("expected_drop_count").getAsJsonObject().get("maximum").getAsInt());
        assertEquals(120.0, attack.get("timeout_seconds").getAsJsonObject().get("maximum").getAsDouble());
        assertEquals(64, drop.get("count").getAsJsonObject().get("maximum").getAsInt());
        assertEquals(120.0, drop.get("timeout_seconds").getAsJsonObject().get("maximum").getAsDouble());
        assertEquals(64, breakBlock.get("expected_drop_count").getAsJsonObject().get("maximum").getAsInt());
        assertEquals(
            List.of("main_hand", "off_hand"),
            useItemOn.get("hand").getAsJsonObject().getAsJsonArray("enum")
                .asList()
                .stream()
                .map(value -> value.getAsString())
                .toList()
        );
        assertEquals("main_hand", useItemOn.get("hand").getAsJsonObject().get("default").getAsString());
        assertEquals(120.0, breakBlock.get("timeout_seconds").getAsJsonObject().get("maximum").getAsDouble());
        assertEquals(3, respawn.size());
        assertEquals(0.1, respawn.get("timeout_seconds").getAsJsonObject().get("minimum").getAsDouble());
        assertEquals(120.0, respawn.get("timeout_seconds").getAsJsonObject().get("maximum").getAsDouble());
        assertEquals(10.0, respawn.get("timeout_seconds").getAsJsonObject().get("default").getAsDouble());
        assertEquals(8, respawn.get("keys").getAsJsonObject().get("maxItems").getAsInt());
        assertEquals(255, respawn.get("ticks").getAsJsonObject().get("maximum").getAsInt());
        JsonObject respawnSchema = find(ClientMcpTools.definitions(), "minecraft_respawn").inputSchema();
        assertEquals(
            "ticks",
            respawnSchema.getAsJsonObject("dependentRequired").getAsJsonArray("keys").get(0).getAsString()
        );
        assertEquals(
            "keys",
            respawnSchema.getAsJsonObject("dependentRequired").getAsJsonArray("ticks").get(0).getAsString()
        );
        assertEquals(32_767, quickMove.get("slot").getAsJsonObject().get("maximum").getAsInt());
        assertEquals(32_767, containerSlot.get("slot").getAsJsonObject().get("maximum").getAsInt());
        assertEquals(
            List.of("primary", "secondary"),
            containerSlot.get("button").getAsJsonObject().getAsJsonArray("enum")
                .asList().stream().map(value -> value.getAsString()).toList()
        );
        assertEquals(
            List.of("slot", "button"),
            find(ClientMcpTools.definitions(), "minecraft_click_container_slot")
                .inputSchema().getAsJsonArray("required")
                .asList().stream().map(value -> value.getAsString()).toList()
        );
        assertEquals(
            Integer.MAX_VALUE,
            containerButton.get("button_id").getAsJsonObject().get("maximum").getAsInt()
        );
        assertEquals(128, scenario.get("id").getAsJsonObject().get("maxLength").getAsInt());
        assertEquals(
            1024,
            scenario.get("artifacts_dir").getAsJsonObject().get("maxLength").getAsInt()
        );
        assertEquals(
            List.of("id"),
            find(ClientMcpTools.definitions(), "minecraft_run_scenario")
                .inputSchema()
                .getAsJsonArray("required")
                .asList()
                .stream()
                .map(element -> element.getAsString())
                .toList()
        );
    }

    @Test
    void publishesTruthfulSafetyAnnotations() {
        List<McpToolDefinition> tools = ClientMcpTools.definitions();

        assertTrue(find(tools, "minecraft_observe").readOnly());
        assertTrue(find(tools, "minecraft_wait_for_loaded_block").readOnly());
        assertTrue(find(tools, "minecraft_wait_for_block_state").readOnly());
        assertTrue(find(tools, "minecraft_read_recipe_book").readOnly());
        assertTrue(find(tools, "minecraft_wait_for_play").readOnly());
        assertTrue(find(tools, "minecraft_wait_for_state_change").readOnly());
        assertTrue(find(tools, "minecraft_wait_for_visible_entity").readOnly());
        assertTrue(find(tools, "minecraft_wait_for_entity_motion").readOnly());
        assertTrue(find(tools, "minecraft_wait_for_entity_removed").readOnly());
        assertTrue(find(tools, "minecraft_wait_for_health_below").readOnly());
        assertTrue(find(tools, "minecraft_wait_for_inventory").readOnly());
        assertTrue(find(tools, "minecraft_wait_for_container_slot").readOnly());
        assertTrue(find(tools, "minecraft_wait_for_visible_item").readOnly());
        assertTrue(find(tools, "minecraft_wait_for_no_visible_item").readOnly());
        assertTrue(find(tools, "minecraft_wait_ticks").readOnly());
        assertFalse(find(tools, "minecraft_respawn").readOnly());
        assertFalse(find(tools, "minecraft_respawn").destructive());
        assertFalse(find(tools, "minecraft_respawn").idempotent());
        assertTrue(find(tools, "minecraft_respawn").openWorld());
        assertFalse(find(tools, "minecraft_connect").destructive());
        assertTrue(find(tools, "minecraft_use_item_on").destructive());
        assertTrue(find(tools, "minecraft_press_inputs").destructive());
        assertTrue(find(tools, "minecraft_select_hotbar_item").destructive());
        assertTrue(find(tools, "minecraft_navigate_to_block").destructive());
        assertTrue(find(tools, "minecraft_approach_entity").destructive());
        assertTrue(find(tools, "minecraft_interact_entity").destructive());
        assertTrue(find(tools, "minecraft_attack_entity_once").destructive());
        assertTrue(find(tools, "minecraft_attack_entity_until_drop_collected").destructive());
        assertTrue(find(tools, "minecraft_drop_selected_item").destructive());
        assertTrue(find(tools, "minecraft_quick_move_container_slot").destructive());
        assertTrue(find(tools, "minecraft_click_container_slot").destructive());
        assertTrue(find(tools, "minecraft_click_container_button").destructive());
        assertTrue(find(tools, "minecraft_run_scenario").destructive());
        assertTrue(find(tools, "minecraft_observe").openWorld());
    }

    private static McpToolDefinition find(List<McpToolDefinition> tools, String name) {
        return tools.stream()
            .filter(tool -> tool.name().equals(name))
            .findFirst()
            .orElseThrow();
    }

    private static JsonObject properties(McpToolDefinition tool) {
        return tool.inputSchema().getAsJsonObject("properties");
    }
}
