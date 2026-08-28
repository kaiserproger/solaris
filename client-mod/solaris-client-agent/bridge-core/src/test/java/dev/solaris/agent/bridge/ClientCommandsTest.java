package dev.solaris.agent.bridge;

import com.google.gson.JsonObject;
import dev.solaris.agent.client.ClientCommands;
import dev.solaris.agent.client.ClientFacade;
import dev.solaris.agent.client.ClientScenarioReport;
import dev.solaris.agent.client.ClientSnapshot;
import dev.solaris.agent.client.ClientTaskExecutor;
import org.junit.jupiter.api.Test;

import java.nio.file.Path;
import java.time.Duration;
import java.util.List;
import java.util.UUID;
import java.util.concurrent.Callable;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class ClientCommandsTest {
    @Test
    void pingReportsBridgeVersionWithoutClientThread() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        CommandRegistry registry = ClientCommands.create(executor, new FakeClient());

        BridgeCommand ping = registry.find("ping").orElseThrow();

        assertEquals("0.1.0", ping.execute(request("ping", "{}")).get("bridge_version").getAsString());
        assertEquals(0, executor.calls);
    }

    @Test
    void stateRunsThroughClientThreadExecutor() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        client.stateVersion = 7;
        CommandRegistry registry = ClientCommands.create(executor, client);

        BridgeCommand state = registry.find("state").orElseThrow();

        JsonObject snapshot = state.execute(request("state", "{}"));
        assertEquals("minecraft:overworld", snapshot.get("dimension").getAsString());
        assertEquals(7, snapshot.get("state_version").getAsLong());
        assertEquals(1, executor.calls);
    }

    @Test
    void waitStateChangeBlocksOnExactClientEvent() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        client.stateVersion = 7;
        CommandRegistry registry = ClientCommands.create(executor, client);

        JsonObject snapshot = registry.find("wait_state_change").orElseThrow().execute(request(
            "wait_state_change",
            "{\"observed_version\":7,\"timeout_seconds\":1.0}"
        ));

        assertEquals(8, snapshot.get("state_version").getAsLong());
        assertEquals(1, client.awaitStateChangeCalls);
        assertEquals(1, executor.calls);
    }

    @Test
    void waitStateChangeFailsWhenNoEventArrives() {
        NeverPlayClient client = new NeverPlayClient();
        CommandRegistry registry = ClientCommands.create(new ImmediateExecutor(), client);

        assertThrows(TimeoutException.class, () ->
            registry.find("wait_state_change").orElseThrow().execute(request(
                "wait_state_change",
                "{\"observed_version\":0,\"timeout_seconds\":1.0}"
            ))
        );
        assertEquals(1, client.awaitStateChangeCalls);
    }

    @Test
    void respawnUsesFacadeConfirmationWithBoundedTimeout() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);
        BridgeCommand respawn = registry.find("respawn").orElseThrow();

        assertEquals("respawned", respawn.execute(request(
            "respawn",
            "{\"timeout_seconds\":8.0}"
        )).get("status").getAsString());
        assertEquals(Duration.ofSeconds(8), client.respawnTimeout);
        assertEquals(0, executor.calls, "facade owns the push-driven respawn confirmation");

        respawn.execute(request(
            "respawn",
            "{\"keys\":[\"forward\",\"sprint\"],\"ticks\":20}"
        ));
        assertTrue(client.respawnWithInputsCalled);
        assertEquals(List.of("forward", "sprint"), client.pressedInputs);
        assertEquals(20, client.pressTicks);

        respawn.execute(request("respawn", "{}"));
        assertEquals(Duration.ofSeconds(10), client.respawnTimeout);
        assertThrows(IllegalArgumentException.class, () -> respawn.execute(request(
            "respawn",
            "{\"keys\":[\"forward\"]}"
        )));
        assertThrows(IllegalArgumentException.class, () -> respawn.execute(request(
            "respawn",
            "{\"ticks\":20}"
        )));
        assertThrows(IllegalArgumentException.class, () -> respawn.execute(request(
            "respawn",
            "{\"timeout_seconds\":0.01}"
        )));
        assertThrows(IllegalArgumentException.class, () -> respawn.execute(request(
            "respawn",
            "{\"timeout_seconds\":120.1}"
        )));
    }

    @Test
    void breakBlockForwardsExactTargetDropAndTimeout() throws Exception {
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(new ImmediateExecutor(), client);

        registry.find("break_block").orElseThrow().execute(request(
            "break_block",
            "{\"x\":2,\"y\":76,\"z\":3,\"face\":\"north\","
                + "\"expected_drop_item_id\":\"minecraft:jungle_log\","
                + "\"expected_drop_count\":1,\"timeout_seconds\":12.0}"
        ));

        assertEquals(List.of(2, 76, 3), client.breakTarget);
        assertEquals("north", client.breakFace);
        assertEquals("minecraft:jungle_log", client.breakDropItemId);
        assertEquals(1, client.breakDropCount);
        assertEquals(Duration.ofSeconds(12), client.breakTimeout);
    }

    @Test
    void structuredObservationsRunThroughClientThreadExecutor() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);

        JsonObject observed = registry.find("observe").orElseThrow().execute(request("observe", "{}"));
        JsonObject block = registry.find("read_block").orElseThrow().execute(
            request("read_block", "{\"x\":10,\"y\":64,\"z\":-3}")
        );

        assertEquals("minecraft:overworld", observed.get("dimension").getAsString());
        assertEquals(client.stateVersion, observed.get("state_version").getAsLong());
        assertEquals("minecraft:stone", block.get("block_id").getAsString());
        assertEquals(List.of(10, 64, -3), client.readBlockPosition);
        assertEquals(2, executor.calls);
    }

    @Test
    void recipeBookObservationIsBoundedAndRunsOnClientThread() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);
        BridgeCommand recipeBook = registry.find("recipe_book").orElseThrow();

        JsonObject result = recipeBook.execute(request("recipe_book", "{\"limit\":64}"));

        assertEquals(3, result.get("entry_count").getAsInt());
        assertEquals(64, client.recipeBookLimit);
        assertEquals(1, executor.calls);
        assertThrows(IllegalArgumentException.class, () -> recipeBook.execute(
            request("recipe_book", "{\"limit\":8193}")
        ));
        assertEquals(1, executor.calls);
    }

    @Test
    void loadedBlockWaitUsesFacadePacketEventsAndValidatesPayload() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);
        BridgeCommand waitLoaded = registry.find("wait_loaded_block").orElseThrow();

        JsonObject result = waitLoaded.execute(request(
            "wait_loaded_block",
            "{\"x\":10,\"y\":64,\"z\":-3,\"timeout_seconds\":8.0}"
        ));

        assertEquals("minecraft:stone", result.get("block_id").getAsString());
        assertEquals(List.of(10, 64, -3), client.waitedLoadedBlockPosition);
        assertEquals(Duration.ofSeconds(8), client.waitedLoadedBlockTimeout);
        assertEquals(0, executor.calls, "facade owns the packet-event wait");
        assertThrows(IllegalArgumentException.class, () -> waitLoaded.execute(request(
            "wait_loaded_block",
            "{\"x\":0,\"y\":0,\"z\":0,\"timeout_seconds\":120.1}"
        )));
    }

    @Test
    void navigateToBlockUsesFacadeArrivalAndValidatesTimeout() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);
        BridgeCommand navigate = registry.find("navigate_to_block").orElseThrow();

        JsonObject result = navigate.execute(request(
            "navigate_to_block",
            "{\"x\":10,\"y\":64,\"z\":-3,\"timeout_seconds\":8.0}"
        ));

        assertTrue(result.get("arrived").getAsBoolean());
        assertEquals(List.of(10, 64, -3), client.navigationTarget);
        assertEquals(Duration.ofSeconds(8), client.navigationTimeout);
        assertEquals(0, executor.calls, "facade owns tick-event navigation progress");
        assertThrows(IllegalArgumentException.class, () -> navigate.execute(request(
            "navigate_to_block",
            "{\"x\":10,\"y\":64,\"z\":-3,\"timeout_seconds\":120.1}"
        )));

        client.navigationTimesOut = true;
        assertThrows(TimeoutException.class, () -> navigate.execute(request(
            "navigate_to_block",
            "{\"x\":10,\"y\":64,\"z\":-3,\"timeout_seconds\":0.1}"
        )));
    }

    @Test
    void scanBlocksRejectsUnboundedOrInvertedBoxesBeforeClientThread() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);
        BridgeCommand scan = registry.find("scan_blocks").orElseThrow();

        JsonObject result = scan.execute(request(
            "scan_blocks",
            "{\"min_x\":0,\"min_y\":60,\"min_z\":0,"
                + "\"max_x\":3,\"max_y\":63,\"max_z\":3,\"max_blocks\":64}"
        ));

        assertEquals(64, result.get("count").getAsInt());
        assertEquals(List.of(0, 60, 0, 3, 63, 3, 64), client.scanArguments);
        assertEquals(1, executor.calls);

        assertThrows(IllegalArgumentException.class, () -> scan.execute(request(
            "scan_blocks",
            "{\"min_x\":1,\"min_y\":0,\"min_z\":0,"
                + "\"max_x\":0,\"max_y\":0,\"max_z\":0}"
        )));
        assertThrows(IllegalArgumentException.class, () -> scan.execute(request(
            "scan_blocks",
            "{\"min_x\":0,\"min_y\":0,\"min_z\":0,"
                + "\"max_x\":16,\"max_y\":16,\"max_z\":16}"
        )));
        assertThrows(IllegalArgumentException.class, () -> scan.execute(request(
            "scan_blocks",
            "{\"min_x\":-2147483648,\"min_y\":-2147483648,\"min_z\":-2147483648,"
                + "\"max_x\":2147483647,\"max_y\":2147483647,\"max_z\":2147483647}"
        )));
        assertEquals(1, executor.calls);
    }

    @Test
    void entityQueryAppliesDefaultsAndBoundsBeforeClientThread() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);
        BridgeCommand entities = registry.find("list_entities").orElseThrow();

        assertEquals(2, entities.execute(request("list_entities", "{}"))
            .get("count").getAsInt());
        assertEquals(32.0, client.entityRadius);
        assertEquals(128, client.entityLimit);
        assertEquals(1, executor.calls);

        assertThrows(IllegalArgumentException.class, () -> entities.execute(
            request("list_entities", "{\"radius\":128.1}")
        ));
        assertThrows(IllegalArgumentException.class, () -> entities.execute(
            request("list_entities", "{\"limit\":513}")
        ));
        assertEquals(1, executor.calls);
    }

    @Test
    void waitVisibleEntityUsesFacadeEventWaitAndValidatesPayload() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);
        BridgeCommand waitVisible = registry.find("wait_visible_entity").orElseThrow();

        JsonObject result = waitVisible.execute(request(
            "wait_visible_entity",
            "{\"entity_type\":\"minecraft:skeleton\",\"radius\":32.0,\"timeout_seconds\":8.0}"
        ));

        assertTrue(result.get("matched").getAsBoolean());
        assertEquals("minecraft:skeleton", client.waitedEntityType);
        assertEquals(32.0, client.waitedEntityRadius);
        assertEquals(Duration.ofSeconds(8), client.waitedEntityTimeout);
        assertEquals(0, executor.calls, "facade owns the event wait");
        assertThrows(IllegalArgumentException.class, () -> waitVisible.execute(request(
            "wait_visible_entity",
            "{\"entity_type\":\"minecraft:skeleton\",\"radius\":128.1}"
        )));
        assertThrows(IllegalArgumentException.class, () -> waitVisible.execute(request(
            "wait_visible_entity",
            "{\"entity_type\":\"\"}"
        )));
    }

    @Test
    void entityIdentityWaitsForwardCanonicalFenceAndBoundedMotionThresholds() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);
        String uuid = "01234567-89ab-cdef-0123-456789abcdef";

        JsonObject motion = registry.find("wait_entity_motion").orElseThrow().execute(request(
            "wait_entity_motion",
            "{\"entity_id\":42,\"entity_uuid\":\"" + uuid + "\","
                + "\"entity_type\":\"minecraft:cow\","
                + "\"minimum_horizontal_distance\":1.5,\"minimum_vertical_rise\":0.25,"
                + "\"timeout_seconds\":12.0}"
        ));
        JsonObject removed = registry.find("wait_entity_removed").orElseThrow().execute(request(
            "wait_entity_removed",
            "{\"entity_id\":42,\"entity_uuid\":\"" + uuid + "\","
                + "\"entity_type\":\"minecraft:cow\",\"timeout_seconds\":20.0}"
        ));

        assertTrue(motion.get("matched").getAsBoolean());
        assertTrue(removed.get("removed").getAsBoolean());
        assertEquals(42, client.waitedIdentityEntityId);
        assertEquals(UUID.fromString(uuid), client.waitedIdentityUuid);
        assertEquals("minecraft:cow", client.waitedIdentityType);
        assertEquals(1.5, client.minimumHorizontalDistance);
        assertEquals(0.25, client.minimumVerticalRise);
        assertEquals(Duration.ofSeconds(12), client.entityMotionTimeout);
        assertEquals(Duration.ofSeconds(20), client.entityRemovedTimeout);
        assertEquals(0, executor.calls, "facade owns both packet-event waits");

        assertThrows(IllegalArgumentException.class, () ->
            registry.find("wait_entity_motion").orElseThrow().execute(request(
                "wait_entity_motion",
                "{\"entity_id\":42,\"entity_uuid\":\"1-1-1-1-1\","
                    + "\"entity_type\":\"minecraft:cow\"}"
            ))
        );
        assertThrows(IllegalArgumentException.class, () ->
            registry.find("wait_entity_motion").orElseThrow().execute(request(
                "wait_entity_motion",
                "{\"entity_id\":42,\"entity_uuid\":\"" + uuid + "\","
                    + "\"entity_type\":\"minecraft:cow\",\"minimum_horizontal_distance\":128.1}"
            ))
        );
    }

    @Test
    void interactEntityForwardsCanonicalIdentityAndExplicitHand() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);
        String uuid = "01234567-89ab-cdef-0123-456789abcdef";

        JsonObject result = registry.find("interact_entity").orElseThrow().execute(request(
            "interact_entity",
            "{\"entity_id\":42,\"entity_uuid\":\"" + uuid + "\","
                + "\"entity_type\":\"minecraft:cow\",\"hand\":\"off_hand\"}"
        ));

        assertTrue(result.get("dispatched").getAsBoolean());
        assertEquals("pass", result.get("result").getAsString());
        assertEquals(42, client.interactedEntityId);
        assertEquals(UUID.fromString(uuid), client.interactedEntityUuid);
        assertEquals("minecraft:cow", client.interactedEntityType);
        assertEquals("off_hand", client.interactedHand);
        assertEquals(0, executor.calls, "facade owns the single client-thread dispatch");

        assertThrows(IllegalArgumentException.class, () -> registry.find("interact_entity")
            .orElseThrow()
            .execute(request(
                "interact_entity",
                "{\"entity_id\":42,\"entity_uuid\":\"" + uuid + "\","
                    + "\"entity_type\":\"minecraft:cow\",\"hand\":\"left\"}"
            ))
        );
        assertThrows(IllegalArgumentException.class, () -> registry.find("interact_entity")
            .orElseThrow()
            .execute(request(
                "interact_entity",
                "{\"entity_id\":42,\"entity_uuid\":\"1-1-1-1-1\","
                    + "\"entity_type\":\"minecraft:cow\"}"
            ))
        );
    }

    @Test
    void useItemOnForwardsExplicitHandAndReturnsTheVanillaResult() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);

        JsonObject result = registry.find("use_item_on").orElseThrow().execute(request(
            "use_item_on",
            "{\"x\":4,\"y\":65,\"z\":-2,\"face\":\"east\",\"hand\":\"off_hand\"}"
        ));

        assertTrue(result.get("dispatched").getAsBoolean());
        assertEquals("off_hand", result.get("hand").getAsString());
        assertEquals("SUCCESS", result.get("result").getAsString());
        assertEquals(List.of(4, 65, -2), client.useItemTarget);
        assertEquals("east", client.useItemFace);
        assertEquals("off_hand", client.useItemHand);
        assertEquals(1, executor.calls);

        registry.find("use_item_on").orElseThrow().execute(request(
            "use_item_on",
            "{\"x\":4,\"y\":65,\"z\":-2,\"face\":\"east\"}"
        ));
        assertEquals("main_hand", client.useItemHand);

        assertThrows(IllegalArgumentException.class, () -> registry.find("use_item_on")
            .orElseThrow()
            .execute(request(
                "use_item_on",
                "{\"x\":4,\"y\":65,\"z\":-2,\"face\":\"east\",\"hand\":\"left\"}"
            ))
        );
    }

    @Test
    void entityIdentityWaitsDoNotHoldSerializedExecutionLock() throws Exception {
        String uuid = "01234567-89ab-cdef-0123-456789abcdef";
        String payload = "{\"entity_id\":42,\"entity_uuid\":\"" + uuid + "\","
            + "\"entity_type\":\"minecraft:cow\"}";

        for (String commandName : List.of("wait_entity_motion", "wait_entity_removed")) {
            BlockingEntityWaitClient client = new BlockingEntityWaitClient();
            CommandRegistry registry = ClientCommands.create(new ImmediateExecutor(), client);
            BridgeCommand waitCommand = registry.find(commandName).orElseThrow();
            BridgeCommand control = registry.find("set_hotbar_slot").orElseThrow();
            try (var tasks = Executors.newVirtualThreadPerTaskExecutor()) {
                Future<JsonObject> wait = tasks.submit(() -> waitCommand.execute(
                    request(commandName, payload)
                ));
                assertTrue(client.waitEntered.await(2, TimeUnit.SECONDS));

                Future<JsonObject> controlResult = tasks.submit(() -> control.execute(
                    request("set_hotbar_slot", "{\"slot\":1}")
                ));
                assertEquals("ok", controlResult.get(2, TimeUnit.SECONDS).get("status").getAsString());

                client.releaseWait.countDown();
                wait.get(2, TimeUnit.SECONDS);
            }
        }
    }

    @Test
    void waitHealthBelowUsesFacadeEventWaitAndValidatesPayload() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);
        BridgeCommand waitHealth = registry.find("wait_health_below").orElseThrow();

        JsonObject result = waitHealth.execute(request(
            "wait_health_below",
            "{\"health\":20.0,\"timeout_seconds\":8.0}"
        ));

        assertEquals(18.0, result.get("health").getAsDouble());
        assertEquals(20.0, client.waitedHealthBelow);
        assertEquals(Duration.ofSeconds(8), client.waitedHealthTimeout);
        assertEquals(0, executor.calls, "facade owns the event wait");
        assertThrows(IllegalArgumentException.class, () -> waitHealth.execute(request(
            "wait_health_below",
            "{\"health\":2048.1}"
        )));
    }

    @Test
    void simultaneousInputsAreValidatedAndDoNotHoldClientThreadExecutor() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);
        BridgeCommand pressInputs = registry.find("press_inputs").orElseThrow();

        assertEquals("ok", pressInputs.execute(request(
            "press_inputs",
            "{\"keys\":[\"forward\",\"sprint\",\"jump\"],\"ticks\":18}"
        )).get("status").getAsString());
        assertEquals(List.of("forward", "sprint", "jump"), client.pressedInputs);
        assertEquals(18, client.pressTicks);
        assertEquals(0, executor.calls);

        assertEquals("ok", pressInputs.execute(request(
            "press_inputs",
            "{\"keys\":[\"swap_offhand\"],\"ticks\":1}"
        )).get("status").getAsString());
        assertEquals(List.of("swap_offhand"), client.pressedInputs);
        assertEquals(1, client.pressTicks);

        assertThrows(IllegalArgumentException.class, () -> pressInputs.execute(request(
            "press_inputs",
            "{\"keys\":[\"forward\",\"forward\"],\"ticks\":2}"
        )));
        assertThrows(IllegalArgumentException.class, () -> pressInputs.execute(request(
            "press_inputs",
            "{\"keys\":[\"inventory\"],\"ticks\":2}"
        )));
        assertThrows(IllegalArgumentException.class, () -> pressInputs.execute(request(
            "press_inputs",
            "{\"keys\":[\"forward\"],\"ticks\":256}"
        )));
        assertThrows(IllegalArgumentException.class, () -> pressInputs.execute(request(
            "press_inputs",
            "{\"keys\":[\"forward\"],\"duration_millis\":100}"
        )));
    }

    @Test
    void sendChatIsBoundedAndRunsOnClientThread() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);
        BridgeCommand sendChat = registry.find("send_chat").orElseThrow();

        sendChat.execute(request("send_chat", "{\"message\":\"time set day\",\"command\":true}"));

        assertEquals("time set day", client.chatMessage);
        assertTrue(client.chatCommand);
        assertEquals(1, executor.calls);
        assertThrows(IllegalArgumentException.class, () -> sendChat.execute(
            request("send_chat", "{\"message\":\"\"}")
        ));
        assertEquals(1, executor.calls);
    }

    @Test
    void dropSelectedItemUsesFacadeEventWaitAndValidatesPayload() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);
        BridgeCommand drop = registry.find("drop_selected_item").orElseThrow();

        JsonObject result = drop.execute(request(
            "drop_selected_item",
            "{\"item_id\":\"minecraft:birch_log\",\"count\":1,\"timeout_seconds\":8.0}"
        ));

        assertEquals("minecraft:birch_log", result.get("item_id").getAsString());
        assertEquals("minecraft:birch_log", client.droppedItemId);
        assertEquals(1, client.droppedCount);
        assertEquals(Duration.ofSeconds(8), client.dropTimeout);
        assertEquals(0, executor.calls, "facade owns the client-thread action and event wait");
        assertThrows(IllegalArgumentException.class, () -> drop.execute(request(
            "drop_selected_item",
            "{\"item_id\":\"\",\"count\":1}"
        )));
        assertThrows(IllegalArgumentException.class, () -> drop.execute(request(
            "drop_selected_item",
            "{\"item_id\":\"minecraft:birch_log\",\"count\":0}"
        )));
        assertThrows(IllegalArgumentException.class, () -> drop.execute(request(
            "drop_selected_item",
            "{\"item_id\":\"minecraft:birch_log\",\"count\":1,\"timeout_seconds\":0}"
        )));
    }

    @Test
    void selectHotbarItemUsesFacadeEventWaitAndValidatesPayload() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);
        BridgeCommand select = registry.find("select_hotbar_item").orElseThrow();

        JsonObject result = select.execute(request(
            "select_hotbar_item",
            "{\"item_id\":\"minecraft:birch_log\",\"count\":1,\"timeout_seconds\":8.0}"
        ));

        assertTrue(result.get("selected").getAsBoolean());
        assertEquals("minecraft:birch_log", client.selectedItemId);
        assertEquals(1, client.selectedItemCount);
        assertEquals(Duration.ofSeconds(8), client.selectItemTimeout);
        assertEquals(0, executor.calls, "facade owns the container action and event wait");
        assertThrows(IllegalArgumentException.class, () -> select.execute(request(
            "select_hotbar_item",
            "{\"item_id\":\"minecraft:birch_log\",\"count\":65}"
        )));
    }

    @Test
    void entityCombatCommandsUseFacadeEventLoopsAndValidatePayload() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);
        BridgeCommand approach = registry.find("approach_entity").orElseThrow();
        BridgeCommand attackOnce = registry.find("attack_entity_once").orElseThrow();
        BridgeCommand attack = registry.find("attack_entity_until_drop_collected").orElseThrow();

        JsonObject approachResult = approach.execute(request(
            "approach_entity",
            "{\"entity_id\":42,\"timeout_seconds\":12.0}"
        ));
        assertTrue(approachResult.get("in_reach").getAsBoolean());
        assertEquals(42, client.approachedEntityId);
        assertEquals(Duration.ofSeconds(12), client.approachEntityTimeout);

        UUID entityUuid = UUID.fromString("00000000-0000-0000-0000-000000000042");
        JsonObject attackOnceResult = attackOnce.execute(request(
            "attack_entity_once",
            "{\"entity_id\":42,\"entity_uuid\":\"" + entityUuid
                + "\",\"entity_type\":\"minecraft:zombie\",\"timeout_seconds\":5.0}"
        ));
        assertTrue(attackOnceResult.get("dispatched").getAsBoolean());
        assertEquals(42, client.attackedOnceEntityId);
        assertEquals(entityUuid, client.attackedOnceEntityUuid);
        assertEquals("minecraft:zombie", client.attackedOnceEntityType);
        assertEquals(Duration.ofSeconds(5), client.attackOnceTimeout);

        JsonObject attackResult = attack.execute(request(
            "attack_entity_until_drop_collected",
            "{\"entity_id\":42,\"expected_drop_item_id\":\"minecraft:rotten_flesh\","
                + "\"expected_drop_count\":1,\"timeout_seconds\":20.0}"
        ));
        assertTrue(attackResult.get("pickup_restored").getAsBoolean());
        assertEquals(42, client.attackedEntityId);
        assertEquals("minecraft:rotten_flesh", client.expectedDropItemId);
        assertEquals(1, client.expectedDropCount);
        assertEquals(Duration.ofSeconds(20), client.attackEntityTimeout);
        assertEquals(0, executor.calls, "facade owns movement, attack, and event waits");

        assertThrows(IllegalArgumentException.class, () -> approach.execute(request(
            "approach_entity",
            "{\"entity_id\":-1}"
        )));
        assertThrows(IllegalArgumentException.class, () -> attack.execute(request(
            "attack_entity_until_drop_collected",
            "{\"entity_id\":42,\"expected_drop_item_id\":\"\",\"expected_drop_count\":1}"
        )));
    }

    @Test
    void waitInventoryUsesFacadeEventWaitAndValidatesPayload() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);
        BridgeCommand waitInventory = registry.find("wait_inventory").orElseThrow();

        JsonObject result = waitInventory.execute(request(
            "wait_inventory",
            "{\"item_id\":\"minecraft:birch_log\",\"count\":1,\"timeout_seconds\":8.0}"
        ));

        assertTrue(result.get("matched").getAsBoolean());
        assertEquals("minecraft:birch_log", client.waitedInventoryItemId);
        assertEquals(1, client.waitedInventoryCount);
        assertEquals(Duration.ofSeconds(8), client.waitedInventoryTimeout);
        assertEquals(0, executor.calls, "facade owns the event wait");
        assertThrows(IllegalArgumentException.class, () -> waitInventory.execute(request(
            "wait_inventory",
            "{\"item_id\":\"minecraft:birch_log\",\"count\":4097}"
        )));
        assertThrows(IllegalArgumentException.class, () -> waitInventory.execute(request(
            "wait_inventory",
            "{\"item_id\":\"minecraft:birch_log\",\"count\":1,\"timeout_seconds\":0}"
        )));
    }

    @Test
    void waitVisibleItemUsesFacadeEventWaitAndValidatesPayload() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);
        BridgeCommand waitVisible = registry.find("wait_visible_item").orElseThrow();

        JsonObject result = waitVisible.execute(request(
            "wait_visible_item",
            "{\"item_id\":\"minecraft:birch_log\",\"x\":10,\"y\":80,\"z\":-2,\"timeout_seconds\":8.0}"
        ));

        assertTrue(result.get("visible").getAsBoolean());
        assertEquals("minecraft:birch_log", client.waitedVisibleItemId);
        assertEquals(List.of(10, 80, -2), client.waitedVisiblePosition);
        assertEquals(Duration.ofSeconds(8), client.waitedVisibleTimeout);
        assertEquals(0, executor.calls, "facade owns the event wait");
        assertThrows(IllegalArgumentException.class, () -> waitVisible.execute(request(
            "wait_visible_item",
            "{\"item_id\":\"minecraft:birch_log\",\"x\":10,\"y\":80,\"timeout_seconds\":8.0}"
        )));
    }

    @Test
    void waitNoVisibleItemUsesFacadeEventWait() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);
        BridgeCommand waitGone = registry.find("wait_no_visible_item").orElseThrow();

        JsonObject result = waitGone.execute(request(
            "wait_no_visible_item",
            "{\"item_id\":\"minecraft:birch_log\",\"x\":10,\"y\":80,\"z\":-2,\"timeout_seconds\":8.0}"
        ));

        assertFalse(result.get("visible").getAsBoolean());
        assertEquals("minecraft:birch_log", client.waitedGoneItemId);
        assertEquals(List.of(10, 80, -2), client.waitedGonePosition);
        assertEquals(Duration.ofSeconds(8), client.waitedGoneTimeout);
        assertEquals(0, executor.calls, "facade owns the event wait");
    }

    @Test
    void waitPlayResamplesOnlyAfterClientStateEvent() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        DelayedPlayClient client = new DelayedPlayClient();
        CommandRegistry registry = ClientCommands.create(executor, client);

        BridgeCommand waitPlay = registry.find("wait_play").orElseThrow();

        assertTrue(waitPlay.execute(
            request("wait_play", "{\"timeout_seconds\":1.0}")
        ).get("in_play").getAsBoolean());
        assertEquals(2, client.snapshotCalls);
        assertEquals(1, client.awaitStateChangeCalls);
        assertEquals(2, executor.calls);
    }

    @Test
    void waitPlaySurvivesTransientClientThreadUnavailability() throws Exception {
        TransientFailureExecutor executor = new TransientFailureExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);

        BridgeCommand waitPlay = registry.find("wait_play").orElseThrow();

        assertTrue(waitPlay.execute(
            request("wait_play", "{\"timeout_seconds\":1.0}")
        ).get("in_play").getAsBoolean());
        assertEquals(1, client.awaitStateChangeCalls);
        assertEquals(2, executor.calls);
    }

    @Test
    void waitPlayAcceptsRealClientGateTimeout() throws Exception {
        CommandRegistry registry = ClientCommands.create(new ImmediateExecutor(), new FakeClient());

        JsonObject snapshot = registry.find("wait_play").orElseThrow().execute(
            request("wait_play", "{\"timeout_seconds\":1800.0}")
        );

        assertTrue(snapshot.get("in_play").getAsBoolean());
    }

    @Test
    void waitPlayTimesOutWithLastSnapshot() throws Exception {
        NeverPlayClient client = new NeverPlayClient();
        CommandRegistry registry = ClientCommands.create(new ImmediateExecutor(), client);

        BridgeCommand waitPlay = registry.find("wait_play").orElseThrow();

        assertEquals("", waitPlay.execute(
            request("wait_play", "{\"timeout_seconds\":0.01}")
        ).get("dimension").getAsString());
        assertEquals(1, client.snapshotCalls);
        assertEquals(1, client.awaitStateChangeCalls);
    }

    @Test
    void connectParsesDriverServerAddressOnClientThread() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);

        BridgeCommand connect = registry.find("connect").orElseThrow();

        connect.execute(request("connect", "{\"server_addr\":\"127.0.0.1:25565\"}"));

        assertEquals("127.0.0.1", client.host);
        assertEquals(25565, client.port);
        assertEquals(1, executor.calls);
    }

    @Test
    void exposedControlsRejectInvalidArgumentsBeforeClientThread() {
        ImmediateExecutor executor = new ImmediateExecutor();
        CommandRegistry registry = ClientCommands.create(executor, new FakeClient());

        assertThrows(IllegalArgumentException.class, () -> registry.find("connect").orElseThrow().execute(
            request("connect", "{\"server_addr\":\"127.0.0.1:65536\"}")
        ));
        assertThrows(IllegalArgumentException.class, () -> registry.find("set_hotbar_slot").orElseThrow().execute(
            request("set_hotbar_slot", "{\"slot\":9}")
        ));
        assertThrows(IllegalArgumentException.class, () -> registry.find("look_at_block").orElseThrow().execute(
            request("look_at_block", "{\"x\":0,\"y\":64,\"z\":0,\"face\":\"inside\"}")
        ));
        assertThrows(IllegalArgumentException.class, () -> registry.find("wait_play").orElseThrow().execute(
            request("wait_play", "{\"timeout_seconds\":0}")
        ));
        assertThrows(IllegalArgumentException.class, () -> registry.find("screenshot").orElseThrow().execute(
            request("screenshot", "{\"path\":\"\"}")
        ));
        assertEquals(0, executor.calls);
    }

    @Test
    void screenshotUsesExactDriverPath() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);
        Path path = Path.of("run/screenshots/m94-02b-rejected-block-resync.png");

        BridgeCommand screenshot = registry.find("screenshot").orElseThrow();

        assertEquals(path.toString(), screenshot.execute(
            request("screenshot", "{\"path\":\"" + path + "\"}")
        ).get("path").getAsString());
        assertEquals(path, client.screenshotPath);
        assertEquals(0, executor.calls, "screenshot completion must wait outside the client thread executor");
    }

    @Test
    void moveForwardDoesNotHoldTheClientThreadExecutor() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);

        BridgeCommand moveForward = registry.find("move_forward").orElseThrow();

        assertEquals("ok", moveForward.execute(
            request("move_forward", "{\"ticks\":15}")
        ).get("status").getAsString());
        assertEquals(15, client.moveForwardTicks);
        assertEquals(0, executor.calls, "movement tick wait must not block the client thread executor");
        assertThrows(IllegalArgumentException.class, () -> moveForward.execute(
            request("move_forward", "{\"duration_millis\":750}")
        ));
    }

    @Test
    void moveBackwardDoesNotHoldTheClientThreadExecutor() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);

        BridgeCommand moveBackward = registry.find("move_backward").orElseThrow();

        assertEquals("ok", moveBackward.execute(
            request("move_backward", "{\"ticks\":15}")
        ).get("status").getAsString());
        assertEquals(15, client.moveBackwardTicks);
        assertEquals(0, executor.calls, "movement tick wait must not block the client thread executor");
    }

    @Test
    void replayWaitTicksIsBoundedAndDoesNotHoldTheClientThreadExecutor() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);

        BridgeCommand waitTicks = registry.find("wait_ticks").orElseThrow();

        assertEquals("ok", waitTicks.execute(
            request("wait_ticks", "{\"ticks\":4}")
        ).get("status").getAsString());
        assertEquals(4, client.waitedTicks);
        assertEquals(0, executor.calls, "tick waits must poll without holding the client thread executor");
        assertThrows(
            IllegalArgumentException.class,
            () -> waitTicks.execute(request("wait_ticks", "{\"ticks\":0}"))
        );
        assertThrows(
            IllegalArgumentException.class,
            () -> waitTicks.execute(request("wait_ticks", "{\"ticks\":256}"))
        );
    }

    @Test
    void replayMoveByAndLookRunOnTheClientThreadWithBoundedPayloads() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);

        BridgeCommand moveBy = registry.find("move_by").orElseThrow();
        BridgeCommand look = registry.find("look").orElseThrow();

        assertEquals("ok", moveBy.execute(
            request("move_by", "{\"dx_cm\":100,\"dz_cm\":-50}")
        ).get("status").getAsString());
        assertEquals(100, client.moveDxCm);
        assertEquals(-50, client.moveDzCm);
        assertEquals("ok", look.execute(
            request("look", "{\"yaw_deg\":90,\"pitch_deg\":0}")
        ).get("status").getAsString());
        assertEquals(90, client.lookYawDeg);
        assertEquals(0, client.lookPitchDeg);
        assertEquals(2, executor.calls);

        assertThrows(
            IllegalArgumentException.class,
            () -> moveBy.execute(request("move_by", "{\"dx_cm\":32768,\"dz_cm\":0}"))
        );
        assertThrows(
            IllegalArgumentException.class,
            () -> look.execute(request("look", "{\"yaw_deg\":181,\"pitch_deg\":0}"))
        );
        assertThrows(
            IllegalArgumentException.class,
            () -> look.execute(request("look", "{\"yaw_deg\":0,\"pitch_deg\":-91}"))
        );
        assertEquals(2, executor.calls, "invalid replay actions must fail before entering the client thread");
    }

    @Test
    void closeScreenRunsOnClientThread() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);

        BridgeCommand closeScreen = registry.find("close_screen").orElseThrow();

        assertEquals("ok", closeScreen.execute(request("close_screen", "{}")).get("status").getAsString());
        assertEquals(1, client.closeScreenCalls);
        assertEquals(1, executor.calls);
    }

    @Test
    void confirmationButtonRequiresExactBoundedIdentityAndRunsOnClientThread() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        BridgeCommand command = ClientCommands.create(executor, client)
            .find("click_confirmation_button")
            .orElseThrow();

        assertEquals(
            "ok",
            command.execute(request(
                "click_confirmation_button",
                """
                {
                  "expected_title": "Allow Solaris content from 127.0.0.1:25567?",
                  "button_label": "Allow"
                }
                """
            )).get("status").getAsString()
        );
        assertEquals(
            "Allow Solaris content from 127.0.0.1:25567?",
            client.confirmationTitle
        );
        assertEquals("Allow", client.confirmationButtonLabel);
        assertEquals(1, executor.calls);

        assertThrows(
            IllegalArgumentException.class,
            () -> command.execute(request(
                "click_confirmation_button",
                "{\"expected_title\":\"title\"}"
            ))
        );
        assertEquals(1, executor.calls);
    }

    @Test
    void screenButtonRequiresExactBoundedIdentityAndRunsOnClientThread() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        BridgeCommand command = ClientCommands.create(executor, client)
            .find("click_screen_button")
            .orElseThrow();

        assertEquals(
            "ok",
            command.execute(request(
                "click_screen_button",
                """
                {
                  "expected_screen_class": "dev.solaris.loader.fabric.LoaderTextScreen",
                  "expected_title": "Ruby Loader Fixture",
                  "button_label": "Confirm Ruby"
                }
                """
            )).get("status").getAsString()
        );
        assertEquals(
            "dev.solaris.loader.fabric.LoaderTextScreen",
            client.expectedScreenClass
        );
        assertEquals("Ruby Loader Fixture", client.expectedScreenTitle);
        assertEquals("Confirm Ruby", client.screenButtonLabel);
        assertEquals(1, executor.calls);

        assertThrows(
            IllegalArgumentException.class,
            () -> command.execute(request(
                "click_screen_button",
                "{\"expected_screen_class\":\"screen\",\"expected_title\":\"title\"}"
            ))
        );
        assertEquals(1, executor.calls);
    }

    @Test
    void openInventoryRunsOnClientThread() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);

        BridgeCommand openInventory = registry.find("open_inventory").orElseThrow();

        assertEquals("ok", openInventory.execute(request("open_inventory", "{}")).get("status").getAsString());
        assertEquals(1, client.openInventoryCalls);
        assertEquals(1, executor.calls);
    }

    @Test
    void runScenarioReturnsStructuredScenarioReport() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);

        BridgeCommand runScenario = registry.find("run_scenario").orElseThrow();

        assertEquals("passed", runScenario.execute(request(
            "run_scenario",
            "{\"id\":\"m94-02b-rejected-block-resync\",\"screenshots_dir\":\"run/screenshots\"}"
        )).get("result").getAsString());
        assertEquals("m94-02b-rejected-block-resync", client.scenarioId);
        assertEquals(Path.of("run/screenshots"), client.scenarioScreenshotsDir);
        assertEquals(0, executor.calls, "long-running scenarios must not block the client thread");

        assertEquals("passed", runScenario.execute(request(
            "run_scenario",
            "{\"id\":\"m94-02b-rejected-block-resync\"}"
        )).get("result").getAsString());
        assertEquals(Path.of("run/mcp-artifacts"), client.scenarioScreenshotsDir);

        assertThrows(IllegalArgumentException.class, () -> runScenario.execute(request(
            "run_scenario",
            "{\"id\":\"\",\"screenshots_dir\":\"run/screenshots\"}"
        )));
        assertThrows(IllegalArgumentException.class, () -> runScenario.execute(request(
            "run_scenario",
            "{\"id\":\"m94-02b-rejected-block-resync\",\"artifacts_dir\":\"\"}"
        )));
        assertEquals("m94-02b-rejected-block-resync", client.scenarioId);
    }

    @Test
    void containerControlsUseFacadeEventWaitAndValidatePayload() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);

        JsonObject quickMove = registry.find("quick_move_container_slot").orElseThrow().execute(request(
            "quick_move_container_slot",
            "{\"slot\":37,\"timeout_seconds\":8.0}"
        ));
        JsonObject click = registry.find("click_container_slot").orElseThrow().execute(request(
            "click_container_slot",
            "{\"slot\":3,\"button\":\"secondary\",\"timeout_seconds\":7.0}"
        ));
        JsonObject button = registry.find("click_container_button").orElseThrow().execute(request(
            "click_container_button",
            "{\"button_id\":2,\"timeout_seconds\":9.0}"
        ));
        JsonObject waited = registry.find("wait_for_container_slot").orElseThrow().execute(request(
            "wait_for_container_slot",
            "{\"slot\":2,\"item_id\":\"minecraft:cooked_porkchop\",\"count\":1,\"timeout_seconds\":12.0}"
        ));

        assertTrue(quickMove.get("confirmed").getAsBoolean());
        assertTrue(click.get("confirmed").getAsBoolean());
        assertTrue(button.get("confirmed").getAsBoolean());
        assertTrue(waited.get("matched").getAsBoolean());
        assertEquals(37, client.quickMovedContainerSlot);
        assertEquals(Duration.ofSeconds(8), client.quickMoveContainerTimeout);
        assertEquals(3, client.clickedContainerSlot);
        assertEquals("secondary", client.containerSlotButton);
        assertEquals(Duration.ofSeconds(7), client.containerSlotTimeout);
        assertEquals(2, client.clickedContainerButton);
        assertEquals(Duration.ofSeconds(9), client.containerButtonTimeout);
        assertEquals(2, client.waitedContainerSlot);
        assertEquals("minecraft:cooked_porkchop", client.waitedContainerItemId);
        assertEquals(1, client.waitedContainerCount);
        assertEquals(Duration.ofSeconds(12), client.waitedContainerTimeout);
        assertEquals(0, executor.calls, "facade owns the packet-event wait");

        assertThrows(IllegalArgumentException.class, () ->
            registry.find("quick_move_container_slot").orElseThrow().execute(request(
                "quick_move_container_slot",
                "{\"slot\":32768}"
            ))
        );
        assertThrows(IllegalArgumentException.class, () ->
            registry.find("click_container_slot").orElseThrow().execute(request(
                "click_container_slot",
                "{\"slot\":0,\"button\":\"middle\"}"
            ))
        );
        assertThrows(IllegalArgumentException.class, () ->
            registry.find("click_container_slot").orElseThrow().execute(request(
                "click_container_slot",
                "{\"slot\":0}"
            ))
        );
        assertThrows(IllegalArgumentException.class, () ->
            registry.find("click_container_slot").orElseThrow().execute(request(
                "click_container_slot",
                "{\"slot\":32768}"
            ))
        );
        assertThrows(IllegalArgumentException.class, () ->
            registry.find("click_container_button").orElseThrow().execute(request(
                "click_container_button",
                "{\"button_id\":-1}"
            ))
        );
        assertThrows(IllegalArgumentException.class, () ->
            registry.find("wait_for_container_slot").orElseThrow().execute(request(
                "wait_for_container_slot",
                "{\"slot\":0,\"item_id\":\"minecraft:stone\",\"count\":0}"
            ))
        );
    }

    private static BridgeRequest request(String command, String payload) {
        return BridgeCodec.decodeRequest(
            "{\"id\":1,\"secret\":\"s\",\"command\":\"" + command + "\",\"payload\":" + payload + "}"
        );
    }

    private static final class ImmediateExecutor implements ClientTaskExecutor {
        int calls;

        @Override
        public <T> T callOnClientThread(Callable<T> callable) throws Exception {
            calls += 1;
            return callable.call();
        }
    }

    private static final class TransientFailureExecutor implements ClientTaskExecutor {
        int calls;

        @Override
        public <T> T callOnClientThread(Callable<T> callable) throws Exception {
            calls += 1;
            if (calls == 1) {
                throw new IllegalStateException("minecraft singleton is not initialized");
            }
            return callable.call();
        }
    }

    private static class FakeClient implements ClientFacade {
        String host;
        int port;
        Path screenshotPath;
        String scenarioId;
        Path scenarioScreenshotsDir;
        int moveForwardTicks;
        int moveBackwardTicks;
        int waitedTicks;
        int moveDxCm;
        int moveDzCm;
        int lookYawDeg;
        int lookPitchDeg;
        int closeScreenCalls;
        String confirmationTitle;
        String confirmationButtonLabel;
        String expectedScreenClass;
        String expectedScreenTitle;
        String screenButtonLabel;
        int openInventoryCalls;
        List<Integer> readBlockPosition;
        List<Integer> scanArguments;
        double entityRadius;
        int entityLimit;
        int recipeBookLimit;
        List<Integer> waitedLoadedBlockPosition;
        Duration waitedLoadedBlockTimeout;
        String waitedEntityType;
        double waitedEntityRadius;
        Duration waitedEntityTimeout;
        int waitedIdentityEntityId;
        UUID waitedIdentityUuid;
        String waitedIdentityType;
        double minimumHorizontalDistance;
        double minimumVerticalRise;
        Duration entityMotionTimeout;
        Duration entityRemovedTimeout;
        double waitedHealthBelow;
        Duration waitedHealthTimeout;
        List<String> pressedInputs;
        int pressTicks;
        String chatMessage;
        boolean chatCommand;
        String droppedItemId;
        int droppedCount;
        Duration dropTimeout;
        int quickMovedContainerSlot = -1;
        int waitedContainerSlot = -1;
        String waitedContainerItemId;
        int waitedContainerCount;
        Duration waitedContainerTimeout;
        Duration quickMoveContainerTimeout;
        int clickedContainerSlot = -1;
        String containerSlotButton;
        Duration containerSlotTimeout;
        int clickedContainerButton = -1;
        Duration containerButtonTimeout;
        String selectedItemId;
        int selectedItemCount;
        Duration selectItemTimeout;
        String waitedInventoryItemId;
        int waitedInventoryCount;
        Duration waitedInventoryTimeout;
        String waitedVisibleItemId;
        List<Integer> waitedVisiblePosition;
        Duration waitedVisibleTimeout;
        String waitedGoneItemId;
        List<Integer> waitedGonePosition;
        Duration waitedGoneTimeout;
        int approachedEntityId;
        Duration approachEntityTimeout;
        List<Integer> navigationTarget;
        Duration navigationTimeout;
        boolean navigationTimesOut;
        int attackedEntityId;
        int attackedOnceEntityId;
        UUID attackedOnceEntityUuid;
        String attackedOnceEntityType;
        Duration attackOnceTimeout;
        String expectedDropItemId;
        int expectedDropCount;
        Duration attackEntityTimeout;
        int interactedEntityId;
        UUID interactedEntityUuid;
        String interactedEntityType;
        String interactedHand;
        List<Integer> useItemTarget;
        String useItemFace;
        String useItemHand;
        Duration respawnTimeout;
        boolean respawnWithInputsCalled;
        List<Integer> breakTarget;
        String breakFace;
        String breakDropItemId;
        int breakDropCount;
        Duration breakTimeout;
        long stateVersion;
        int awaitStateChangeCalls;

        @Override
        public ClientSnapshot snapshot() {
            return new ClientSnapshot(
                true,
                "minecraft:overworld",
                10.0,
                64.0,
                -3.0,
                2,
                "none",
                ""
            );
        }

        @Override
        public long stateVersion() {
            return stateVersion;
        }

        @Override
        public boolean awaitStateChange(long observedVersion, Duration timeout) {
            awaitStateChangeCalls += 1;
            stateVersion += 1;
            return true;
        }

        public JsonObject observe() {
            JsonObject observation = new JsonObject();
            observation.addProperty("dimension", "minecraft:overworld");
            return observation;
        }

        public JsonObject readBlock(int x, int y, int z) {
            readBlockPosition = List.of(x, y, z);
            JsonObject block = new JsonObject();
            block.addProperty("block_id", "minecraft:stone");
            return block;
        }

        public JsonObject scanBlocks(
            int minX,
            int minY,
            int minZ,
            int maxX,
            int maxY,
            int maxZ,
            int maxBlocks
        ) {
            scanArguments = List.of(minX, minY, minZ, maxX, maxY, maxZ, maxBlocks);
            JsonObject blocks = new JsonObject();
            blocks.addProperty("count", 64);
            return blocks;
        }

        public JsonObject listEntities(double radius, int limit) {
            entityRadius = radius;
            entityLimit = limit;
            JsonObject entities = new JsonObject();
            entities.addProperty("count", 2);
            return entities;
        }

        @Override
        public JsonObject readRecipeBook(int limit) {
            recipeBookLimit = limit;
            JsonObject recipeBook = new JsonObject();
            recipeBook.addProperty("entry_count", 3);
            return recipeBook;
        }

        @Override
        public JsonObject waitForLoadedBlock(int x, int y, int z, Duration timeout) {
            waitedLoadedBlockPosition = List.of(x, y, z);
            waitedLoadedBlockTimeout = timeout;
            JsonObject block = new JsonObject();
            block.addProperty("block_id", "minecraft:stone");
            return block;
        }

        public JsonObject waitForVisibleEntity(String entityType, double radius, Duration timeout) {
            waitedEntityType = entityType;
            waitedEntityRadius = radius;
            waitedEntityTimeout = timeout;
            JsonObject entity = new JsonObject();
            entity.addProperty("matched", true);
            entity.addProperty("type", entityType);
            return entity;
        }

        public JsonObject waitForEntityMotion(
            int entityId,
            UUID uuid,
            String entityType,
            double minimumHorizontalDistance,
            double minimumVerticalRise,
            Duration timeout
        ) throws Exception {
            recordIdentity(entityId, uuid, entityType);
            this.minimumHorizontalDistance = minimumHorizontalDistance;
            this.minimumVerticalRise = minimumVerticalRise;
            entityMotionTimeout = timeout;
            JsonObject result = new JsonObject();
            result.addProperty("matched", true);
            return result;
        }

        public JsonObject waitForEntityRemoved(
            int entityId,
            UUID uuid,
            String entityType,
            Duration timeout
        ) throws Exception {
            recordIdentity(entityId, uuid, entityType);
            entityRemovedTimeout = timeout;
            JsonObject result = new JsonObject();
            result.addProperty("removed", true);
            return result;
        }

        private void recordIdentity(int entityId, UUID uuid, String entityType) {
            waitedIdentityEntityId = entityId;
            waitedIdentityUuid = uuid;
            waitedIdentityType = entityType;
        }

        public JsonObject waitForHealthBelow(double health, Duration timeout) {
            waitedHealthBelow = health;
            waitedHealthTimeout = timeout;
            JsonObject result = new JsonObject();
            result.addProperty("matched", true);
            result.addProperty("health", 18.0);
            return result;
        }

        public JsonObject approachEntity(int entityId, Duration timeout) {
            approachedEntityId = entityId;
            approachEntityTimeout = timeout;
            JsonObject result = new JsonObject();
            result.addProperty("entity_id", entityId);
            result.addProperty("in_reach", true);
            return result;
        }

        public JsonObject navigateToBlock(int x, int y, int z, Duration timeout) throws Exception {
            navigationTarget = List.of(x, y, z);
            navigationTimeout = timeout;
            if (navigationTimesOut) {
                throw new TimeoutException("navigation timed out before arrival");
            }
            JsonObject result = new JsonObject();
            result.addProperty("arrived", true);
            return result;
        }

        public JsonObject attackEntityUntilDropCollected(
            int entityId,
            String expectedDropItemId,
            int expectedDropCount,
            Duration timeout
        ) {
            attackedEntityId = entityId;
            this.expectedDropItemId = expectedDropItemId;
            this.expectedDropCount = expectedDropCount;
            attackEntityTimeout = timeout;
            JsonObject result = new JsonObject();
            result.addProperty("entity_id", entityId);
            result.addProperty("pickup_restored", true);
            return result;
        }

        @Override
        public JsonObject attackEntityOnce(
            int entityId,
            UUID entityUuid,
            String entityType,
            Duration timeout
        ) {
            attackedOnceEntityId = entityId;
            attackedOnceEntityUuid = entityUuid;
            attackedOnceEntityType = entityType;
            attackOnceTimeout = timeout;
            JsonObject result = new JsonObject();
            result.addProperty("dispatched", true);
            return result;
        }

        @Override
        public JsonObject interactEntity(int entityId, UUID entityUuid, String entityType, String hand) {
            interactedEntityId = entityId;
            interactedEntityUuid = entityUuid;
            interactedEntityType = entityType;
            interactedHand = hand;
            JsonObject result = new JsonObject();
            result.addProperty("dispatched", true);
            result.addProperty("result", "pass");
            return result;
        }

        public void respawn(Duration timeout) {
            respawnTimeout = timeout;
        }

        @Override
        public void respawnWithInputs(List<String> inputs, int ticks, Duration timeout) {
            respawnWithInputsCalled = true;
            pressedInputs = List.copyOf(inputs);
            pressTicks = ticks;
            respawnTimeout = timeout;
        }

        public void pressInputs(List<String> inputs, int ticks) {
            pressedInputs = List.copyOf(inputs);
            pressTicks = ticks;
        }

        @Override
        public JsonObject breakBlock(
            int x,
            int y,
            int z,
            String face,
            String expectedDropItemId,
            int expectedDropCount,
            Duration timeout
        ) {
            breakTarget = List.of(x, y, z);
            breakFace = face;
            breakDropItemId = expectedDropItemId;
            breakDropCount = expectedDropCount;
            breakTimeout = timeout;
            return new JsonObject();
        }

        public void sendChat(String message, boolean command) {
            chatMessage = message;
            chatCommand = command;
        }

        @Override
        public JsonObject dropSelectedItem(String itemId, int count, Duration timeout) {
            droppedItemId = itemId;
            droppedCount = count;
            dropTimeout = timeout;
            JsonObject result = new JsonObject();
            result.addProperty("status", "confirmed");
            result.addProperty("item_id", itemId);
            result.addProperty("count", count);
            result.addProperty("visible", true);
            return result;
        }

        @Override
        public JsonObject quickMoveContainerSlot(int slot, Duration timeout) {
            quickMovedContainerSlot = slot;
            quickMoveContainerTimeout = timeout;
            JsonObject result = new JsonObject();
            result.addProperty("confirmed", true);
            return result;
        }

        @Override
        public JsonObject waitForContainerSlot(
            int slot,
            String itemId,
            int count,
            Duration timeout
        ) {
            waitedContainerSlot = slot;
            waitedContainerItemId = itemId;
            waitedContainerCount = count;
            waitedContainerTimeout = timeout;
            JsonObject result = new JsonObject();
            result.addProperty("matched", true);
            return result;
        }

        @Override
        public JsonObject clickContainerSlot(int slot, String button, Duration timeout) {
            clickedContainerSlot = slot;
            containerSlotButton = button;
            containerSlotTimeout = timeout;
            JsonObject result = new JsonObject();
            result.addProperty("confirmed", true);
            return result;
        }

        @Override
        public JsonObject clickContainerButton(int buttonId, Duration timeout) {
            clickedContainerButton = buttonId;
            containerButtonTimeout = timeout;
            JsonObject result = new JsonObject();
            result.addProperty("confirmed", true);
            return result;
        }

        public JsonObject selectHotbarItem(String itemId, int count, Duration timeout) {
            selectedItemId = itemId;
            selectedItemCount = count;
            selectItemTimeout = timeout;
            JsonObject result = new JsonObject();
            result.addProperty("selected", true);
            return result;
        }

        public JsonObject waitForInventoryCount(String itemId, int count, Duration timeout) {
            waitedInventoryItemId = itemId;
            waitedInventoryCount = count;
            waitedInventoryTimeout = timeout;
            JsonObject result = new JsonObject();
            result.addProperty("matched", true);
            return result;
        }

        public JsonObject waitForVisibleItem(String itemId, int x, int y, int z, Duration timeout) {
            waitedVisibleItemId = itemId;
            waitedVisiblePosition = List.of(x, y, z);
            waitedVisibleTimeout = timeout;
            JsonObject result = new JsonObject();
            result.addProperty("visible", true);
            return result;
        }

        public JsonObject waitForNoVisibleItem(String itemId, int x, int y, int z, Duration timeout) {
            waitedGoneItemId = itemId;
            waitedGonePosition = List.of(x, y, z);
            waitedGoneTimeout = timeout;
            JsonObject result = new JsonObject();
            result.addProperty("visible", false);
            return result;
        }

        @Override
        public void connect(String host, int port) {
            this.host = host;
            this.port = port;
        }

        @Override
        public void selectHotbarSlot(int slot) {
        }

        @Override
        public void lookAtBlock(int x, int y, int z, String face) {
        }

        @Override
        public JsonObject useItemOn(int x, int y, int z, String face, String hand) {
            useItemTarget = List.of(x, y, z);
            useItemFace = face;
            useItemHand = hand;
            JsonObject result = new JsonObject();
            result.addProperty("dispatched", true);
            result.addProperty("hand", hand);
            result.addProperty("result", "SUCCESS");
            return result;
        }

        @Override
        public void moveForward(int ticks) {
            moveForwardTicks = ticks;
        }

        @Override
        public void moveBackward(int ticks) {
            moveBackwardTicks = ticks;
        }

        @Override
        public void waitTicks(int ticks) {
            waitedTicks = ticks;
        }

        @Override
        public void moveByCentimeters(int dxCm, int dzCm) {
            moveDxCm = dxCm;
            moveDzCm = dzCm;
        }

        @Override
        public void look(int yawDeg, int pitchDeg) {
            lookYawDeg = yawDeg;
            lookPitchDeg = pitchDeg;
        }

        @Override
        public void closeCurrentScreen() {
            closeScreenCalls += 1;
        }

        @Override
        public void clickConfirmationButton(String expectedTitle, String buttonLabel) {
            confirmationTitle = expectedTitle;
            confirmationButtonLabel = buttonLabel;
        }

        @Override
        public void clickScreenButton(
            String expectedScreenClass,
            String expectedTitle,
            String buttonLabel
        ) {
            this.expectedScreenClass = expectedScreenClass;
            expectedScreenTitle = expectedTitle;
            screenButtonLabel = buttonLabel;
        }

        @Override
        public void openInventory() {
            openInventoryCalls += 1;
        }

        @Override
        public Path screenshot(Path path) {
            screenshotPath = path;
            return path;
        }

        @Override
        public ClientScenarioReport runScenario(String id, Path screenshotsDir) {
            scenarioId = id;
            scenarioScreenshotsDir = screenshotsDir;
            return new ClientScenarioReport("passed", id, List.of("fake client executed scenario"));
        }

        @Override
        public void disconnect() {
        }
    }

    private static final class DelayedPlayClient extends FakeClient {
        int snapshotCalls;

        @Override
        public ClientSnapshot snapshot() {
            snapshotCalls += 1;
            if (snapshotCalls == 1) {
                return new ClientSnapshot(false, "", 0.0, 0.0, 0.0, -1, "joining", "");
            }
            return super.snapshot();
        }
    }

    private static final class NeverPlayClient extends FakeClient {
        int snapshotCalls;

        @Override
        public ClientSnapshot snapshot() {
            snapshotCalls += 1;
            return new ClientSnapshot(false, "", 0.0, 0.0, 0.0, -1, "joining", "");
        }

        @Override
        public boolean awaitStateChange(long observedVersion, Duration timeout) {
            awaitStateChangeCalls += 1;
            return false;
        }
    }

    private static final class BlockingEntityWaitClient extends FakeClient {
        private final CountDownLatch waitEntered = new CountDownLatch(1);
        private final CountDownLatch releaseWait = new CountDownLatch(1);

        @Override
        public JsonObject waitForEntityMotion(
            int entityId,
            UUID uuid,
            String entityType,
            double minimumHorizontalDistance,
            double minimumVerticalRise,
            Duration timeout
        ) throws Exception {
            awaitRelease();
            return super.waitForEntityMotion(
                entityId,
                uuid,
                entityType,
                minimumHorizontalDistance,
                minimumVerticalRise,
                timeout
            );
        }

        @Override
        public JsonObject waitForEntityRemoved(
            int entityId,
            UUID uuid,
            String entityType,
            Duration timeout
        ) throws Exception {
            awaitRelease();
            return super.waitForEntityRemoved(entityId, uuid, entityType, timeout);
        }

        private void awaitRelease() throws Exception {
            waitEntered.countDown();
            if (!releaseWait.await(2, TimeUnit.SECONDS)) {
                throw new TimeoutException("test did not release entity wait");
            }
        }
    }
}
