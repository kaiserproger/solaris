package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

final class M94InventoryCraftingScenario {
    static final String ID = "m94-03a-inventory-oak-log-to-planks";
    static final String BROAD_ID = "m94-03-inventory-crafting-containers-stations";
    static final String SHARED_CHEST_ID = "m94-03b-two-client-shared-chest";
    static final String SHARED_CHEST_DEPOSIT_ID = "m94-03b-two-client-shared-chest-deposit";
    static final String SHARED_CHEST_OBSERVE_ID = "m94-03b-two-client-shared-chest-observe";
    static final String SHARED_CHEST_LIVE_UPDATE_ID = "m94-03c-two-client-shared-chest-live-update";
    static final String SHARED_CHEST_LIVE_OPEN_ID = "m94-03c-two-client-shared-chest-open-with-dirt";
    static final String SHARED_CHEST_LIVE_WITHDRAW_ID = "m94-03c-two-client-shared-chest-withdraw";
    static final String SHARED_CHEST_LIVE_OBSERVE_EMPTY_ID = "m94-03c-two-client-shared-chest-observe-empty";
    static final String TABLE_CRAFT_ID = "m94-03d-crafting-table-max-craft";
    static final String FURNACE_UI_ID = "m94-03e-furnace-family-ui";
    static final String MALFORMED_REJECTION_ID = "m94-03f-malformed-container-rejection";
    static final String REOPEN_CONSERVATION_ID = "m94-03g-chest-reopen-conservation";
    static final int OAK_PLANKS_RECIPE_DISPLAY_ID = 697;
    private static final String CONTAINER_SCREEN = "net.minecraft.client.gui.screens.inventory.ContainerScreen";
    private static final String SHARED_CHEST_MARKER_FILE = "m94-03b-shared-chest-marker.properties";
    private static final String SHARED_CHEST_LIVE_MARKER_FILE = "m94-03c-live-shared-chest-marker.properties";
    private static final Duration SETUP_TIMEOUT = Duration.ofSeconds(8);
    private static final Duration BLOCK_TIMEOUT = Duration.ofSeconds(2);
    private static final Duration INVENTORY_TIMEOUT = Duration.ofSeconds(5);
    private static final int HOTBAR_SLOT = 0;
    private static final int SHARED_CHEST_SLOT = 0;
    private static final double SHARED_CHEST_SETUP_X = 2.5;
    private static final double SHARED_CHEST_SETUP_Y = 81.0;
    private static final double SHARED_CHEST_SETUP_Z = 0.5;

    static boolean supports(String id) {
        return ID.equals(id)
            || BROAD_ID.equals(id)
            || SHARED_CHEST_ID.equals(id)
            || SHARED_CHEST_DEPOSIT_ID.equals(id)
            || SHARED_CHEST_OBSERVE_ID.equals(id)
            || SHARED_CHEST_LIVE_UPDATE_ID.equals(id)
            || SHARED_CHEST_LIVE_OPEN_ID.equals(id)
            || SHARED_CHEST_LIVE_WITHDRAW_ID.equals(id)
            || SHARED_CHEST_LIVE_OBSERVE_EMPTY_ID.equals(id)
            || TABLE_CRAFT_ID.equals(id)
            || FURNACE_UI_ID.equals(id)
            || MALFORMED_REJECTION_ID.equals(id)
            || REOPEN_CONSERVATION_ID.equals(id);
    }

    ClientScenarioReport run(String id, Path screenshotsDir, ScenarioClient client) {
        if (SHARED_CHEST_ID.equals(id)) {
            return new ClientScenarioReport(
                "blocked",
                id,
                List.of("blocked: " + id + " requires primary/secondary orchestration by the real-client driver")
            );
        }
        if (SHARED_CHEST_LIVE_UPDATE_ID.equals(id)) {
            return new ClientScenarioReport(
                "blocked",
                id,
                List.of("blocked: " + id + " requires primary/secondary/primary orchestration by the real-client driver")
            );
        }
        if (SHARED_CHEST_DEPOSIT_ID.equals(id)) {
            return runTwoClientSharedChestDeposit(id, screenshotsDir, client);
        }
        if (SHARED_CHEST_OBSERVE_ID.equals(id)) {
            return runTwoClientSharedChestObserve(id, screenshotsDir, client);
        }
        if (SHARED_CHEST_LIVE_OPEN_ID.equals(id)) {
            return runTwoClientSharedChestLiveOpen(id, screenshotsDir, client);
        }
        if (SHARED_CHEST_LIVE_WITHDRAW_ID.equals(id)) {
            return runTwoClientSharedChestLiveWithdraw(id, screenshotsDir, client);
        }
        if (SHARED_CHEST_LIVE_OBSERVE_EMPTY_ID.equals(id)) {
            return runTwoClientSharedChestLiveObserveEmpty(id, screenshotsDir, client);
        }
        if (REOPEN_CONSERVATION_ID.equals(id)) {
            return runChestReopenConservation(id, screenshotsDir, client);
        }
        if (TABLE_CRAFT_ID.equals(id)) {
            return blockedPhase(
                id,
                "crafting-table max-craft requires a dedicated table-grid/cursor client primitive"
            );
        }
        if (FURNACE_UI_ID.equals(id)) {
            return blockedPhase(
                id,
                "furnace-family UI requires dedicated input, fuel, output, and reopen observations"
            );
        }
        if (MALFORMED_REJECTION_ID.equals(id)) {
            return blockedPhase(
                id,
                "malformed container rejection requires a bounded raw-click injection primitive"
            );
        }
        if (!supports(id)) {
            return new ClientScenarioReport("blocked", id, List.of("unsupported scenario id: " + id));
        }
        boolean broadScenario = BROAD_ID.equals(id);

        List<String> observations = new ArrayList<>();
        try {
            ScenarioHeldItem oakLog = client.giveAndSelect("minecraft:oak_log", 1, 0, SETUP_TIMEOUT);
            if (!oakLog.matches("minecraft:oak_log", 1)) {
                observations.add(
                    "blocked: expected oak log setup, saw " + oakLog.itemId() + " x" + oakLog.count()
                );
                return new ClientScenarioReport("blocked", id, observations);
            }
            observations.add("held setup: minecraft:oak_log x1 in hotbar slot 0");
            int initialOakLogCount = client.inventoryCount("minecraft:oak_log");
            int initialOakPlanksCount = client.inventoryCount("minecraft:oak_planks");
            int expectedOakLogCount = Math.max(0, initialOakLogCount - 1);
            int expectedOakPlanksCount = initialOakPlanksCount + 4;
            observations.add(
                "inventory baseline: oak_log_count=" + initialOakLogCount
                    + " oak_planks_count=" + initialOakPlanksCount
            );

            client.placeRecipe(0, OAK_PLANKS_RECIPE_DISPLAY_ID, false);
            boolean logConsumed = client.waitForInventoryCount(
                "minecraft:oak_log",
                expectedOakLogCount,
                INVENTORY_TIMEOUT
            );
            boolean planksCreated = client.waitForInventoryCount(
                "minecraft:oak_planks",
                expectedOakPlanksCount,
                INVENTORY_TIMEOUT
            );
            observations.add(
                "inventory recipe: " + (logConsumed && planksCreated ? "passed" : "failed")
                    + " recipe_display_id=" + OAK_PLANKS_RECIPE_DISPLAY_ID
                    + " oak_log_expected_count=" + expectedOakLogCount
                    + " oak_log_count_matched=" + logConsumed
                    + " oak_planks_expected_count=" + expectedOakPlanksCount
                    + " oak_planks_count_matched=" + planksCreated
            );
            observations.add(
                "degraded: crafting table UI, cursor recovery, recipe-book discovery UI, furnace-family UI, "
                    + "stations, malformed clicks, and broad recipe execution are not fully exercised by " + id
            );
            observations.add(
                "degraded: recipe_display_id="
                    + OAK_PLANKS_RECIPE_DISPLAY_ID
                    + " is tied to the current configured vanilla sidecar recipe layout"
            );
            ChestProbeResult chestProbe = broadScenario
                ? runSimpleChestOpenProbe(client, observations)
                : ChestProbeResult.passed();
            if (broadScenario) {
                observations.add("focused phase available: " + ID);
                observations.add("focused phase available: " + SHARED_CHEST_ID);
                observations.add("focused phase available: " + SHARED_CHEST_LIVE_UPDATE_ID);
                observations.add("focused phase available: " + REOPEN_CONSERVATION_ID);
                observations.add("focused phase blocked: " + TABLE_CRAFT_ID);
                observations.add("focused phase blocked: " + FURNACE_UI_ID);
                observations.add("focused phase blocked: " + MALFORMED_REJECTION_ID);
                observations.add(
                    "blocked: broad inventory/container coverage is the conjunction of the named focused phases; "
                        + "unavailable phases cannot be counted through " + BROAD_ID
                );
            }
            observations.add("screenshots directory available to driver: " + screenshotsDir);

            if (broadScenario) {
                if (!logConsumed || !planksCreated || chestProbe.isFailed()) {
                    return new ClientScenarioReport("failed", id, observations);
                }
                return new ClientScenarioReport("blocked", id, observations);
            }
            return new ClientScenarioReport(
                logConsumed && planksCreated ? "passed" : "failed",
                id,
                observations
            );
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private ClientScenarioReport runTwoClientSharedChestDeposit(
        String id,
        Path screenshotsDir,
        ScenarioClient client
    ) {
        List<String> observations = new ArrayList<>();
        try {
            if (!movePrimaryToSharedChestSetup(client, observations)) {
                return new ClientScenarioReport("blocked", id, observations);
            }
            ScenarioBlockPair pair = client.findDryPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
            if (pair == null) {
                observations.add("blocked: no loaded dry chest placement target found within survival reach");
                return new ClientScenarioReport("blocked", id, observations);
            }
            observations.add(
                "shared chest target: clicked=" + pair.clicked().blockId()
                    + " at " + coordinates(pair.clicked())
                    + ", target=" + pair.target().blockId()
                    + " at " + coordinates(pair.target())
            );

            ScenarioHeldItem chest = client.giveAndSelect(
                "minecraft:chest",
                1,
                HOTBAR_SLOT,
                SETUP_TIMEOUT
            );
            if (!chest.matches("minecraft:chest", 1)) {
                observations.add(
                    "blocked: expected chest setup, saw " + chest.itemId() + " x" + chest.count()
                );
                return new ClientScenarioReport("blocked", id, observations);
            }
            ScenarioUseResult placeUse = client.useItemOn(pair.clicked(), chest);
            boolean placed = client.waitForBlock(pair.target(), "minecraft:chest", BLOCK_TIMEOUT);
            ScenarioBlockTarget chestTarget = new ScenarioBlockTarget(
                pair.target().x(),
                pair.target().y(),
                pair.target().z(),
                "up",
                pair.target().label(),
                "minecraft:chest"
            );
            observations.add(
                "shared chest placement: " + (placed ? "passed" : "failed")
                    + " place_use_result=" + placeUse.result()
                    + " target=" + coordinates(chestTarget)
            );
            if (!placed) {
                return new ClientScenarioReport("failed", id, observations);
            }
            writeMarker(sharedChestMarkerPath(screenshotsDir), chestTarget);
            observations.add("shared chest marker written for secondary real-client observer");

            ScenarioHeldItem dirt = client.giveAndSelect("minecraft:dirt", 1, HOTBAR_SLOT, SETUP_TIMEOUT);
            if (!dirt.matches("minecraft:dirt", 1)) {
                observations.add(
                    "blocked: expected dirt setup, saw " + dirt.itemId() + " x" + dirt.count()
                );
                return new ClientScenarioReport("blocked", id, observations);
            }
            ScenarioUseResult openUse = client.useItemOn(chestTarget, dirt);
            boolean opened = client.waitForScreenClassName(CONTAINER_SCREEN, INVENTORY_TIMEOUT);
            boolean moved = opened
                && client.moveSelectedItemToContainerSlot(
                    SHARED_CHEST_SLOT,
                    "minecraft:dirt",
                    1,
                    INVENTORY_TIMEOUT
                );
            boolean slotMatched = moved
                && client.waitForContainerSlot(
                    SHARED_CHEST_SLOT,
                    "minecraft:dirt",
                    1,
                    INVENTORY_TIMEOUT
                );
            boolean closed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
            boolean passed = opened && moved && slotMatched && closed;
            observations.add(
                "shared chest deposit: " + (passed ? "passed" : "failed")
                    + " open_use_result=" + openUse.result()
                    + " screen_matched=" + opened
                    + " moved_slot=" + moved
                    + " slot_matched=" + slotMatched
                    + " closed=" + closed
            );
            observations.add("screenshots directory available to driver: " + screenshotsDir);
            return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private ClientScenarioReport runTwoClientSharedChestObserve(
        String id,
        Path screenshotsDir,
        ScenarioClient client
    ) {
        List<String> observations = new ArrayList<>();
        Path markerPath = sharedChestMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing shared chest marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        try {
            ScenarioBlockTarget chestTarget = readMarker(markerPath);
            boolean visible = client.waitForBlock(chestTarget, "minecraft:chest", BLOCK_TIMEOUT);
            ScenarioHeldItem heldItem = client.selectedItem();
            ScenarioUseResult openUse = client.useItemOn(chestTarget, heldItem);
            boolean opened = client.waitForScreenClassName(CONTAINER_SCREEN, INVENTORY_TIMEOUT);
            boolean slotMatched = opened
                && client.waitForContainerSlot(
                    SHARED_CHEST_SLOT,
                    "minecraft:dirt",
                    1,
                    INVENTORY_TIMEOUT
                );
            boolean closed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
            boolean passed = visible && opened && slotMatched && closed;
            observations.add(
                "shared chest observe: " + (passed ? "passed" : "failed")
                    + " target=" + coordinates(chestTarget)
                    + " visible=" + visible
                    + " held_item=" + heldItem.itemId() + " x" + heldItem.count()
                    + " open_use_result=" + openUse.result()
                    + " screen_matched=" + opened
                    + " slot_matched=" + slotMatched
                    + " closed=" + closed
            );
            observations.add("screenshots directory available to driver: " + screenshotsDir);
            return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private ClientScenarioReport runTwoClientSharedChestLiveOpen(
        String id,
        Path screenshotsDir,
        ScenarioClient client
    ) {
        List<String> observations = new ArrayList<>();
        try {
            if (!movePrimaryToSharedChestSetup(client, observations)) {
                return new ClientScenarioReport("blocked", id, observations);
            }
            ScenarioBlockPair pair = client.findDryPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
            if (pair == null) {
                observations.add("blocked: no loaded dry chest placement target found within survival reach");
                return new ClientScenarioReport("blocked", id, observations);
            }
            observations.add(
                "shared chest live target: clicked=" + pair.clicked().blockId()
                    + " at " + coordinates(pair.clicked())
                    + ", target=" + pair.target().blockId()
                    + " at " + coordinates(pair.target())
            );

            ScenarioHeldItem chest = client.giveAndSelect(
                "minecraft:chest",
                1,
                HOTBAR_SLOT,
                SETUP_TIMEOUT
            );
            if (!chest.matches("minecraft:chest", 1)) {
                observations.add(
                    "blocked: expected chest setup, saw " + chest.itemId() + " x" + chest.count()
                );
                return new ClientScenarioReport("blocked", id, observations);
            }
            ScenarioUseResult placeUse = client.useItemOn(pair.clicked(), chest);
            boolean placed = client.waitForBlock(pair.target(), "minecraft:chest", BLOCK_TIMEOUT);
            ScenarioBlockTarget chestTarget = new ScenarioBlockTarget(
                pair.target().x(),
                pair.target().y(),
                pair.target().z(),
                "up",
                pair.target().label(),
                "minecraft:chest"
            );
            observations.add(
                "shared chest live placement: " + (placed ? "passed" : "failed")
                    + " place_use_result=" + placeUse.result()
                    + " target=" + coordinates(chestTarget)
            );
            if (!placed) {
                return new ClientScenarioReport("failed", id, observations);
            }
            writeMarker(sharedChestLiveMarkerPath(screenshotsDir), chestTarget);
            observations.add("shared chest live marker written for secondary real-client mutator");

            ScenarioHeldItem dirt = client.giveAndSelect("minecraft:dirt", 1, HOTBAR_SLOT, SETUP_TIMEOUT);
            if (!dirt.matches("minecraft:dirt", 1)) {
                observations.add(
                    "blocked: expected dirt setup, saw " + dirt.itemId() + " x" + dirt.count()
                );
                return new ClientScenarioReport("blocked", id, observations);
            }
            ScenarioUseResult openUse = client.useItemOn(chestTarget, dirt);
            boolean opened = client.waitForScreenClassName(CONTAINER_SCREEN, INVENTORY_TIMEOUT);
            boolean moved = opened
                && client.moveSelectedItemToContainerSlot(
                    SHARED_CHEST_SLOT,
                    "minecraft:dirt",
                    1,
                    INVENTORY_TIMEOUT
                );
            boolean slotMatched = moved
                && client.waitForContainerSlot(
                    SHARED_CHEST_SLOT,
                    "minecraft:dirt",
                    1,
                    INVENTORY_TIMEOUT
                );
            boolean passed = opened && moved && slotMatched;
            observations.add(
                "shared chest live open: " + (passed ? "passed" : "failed")
                    + " open_use_result=" + openUse.result()
                    + " screen_matched=" + opened
                    + " moved_slot=" + moved
                    + " slot_matched=" + slotMatched
                    + " primary_screen_left_open=" + passed
            );
            observations.add("screenshots directory available to driver: " + screenshotsDir);
            return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private ClientScenarioReport runTwoClientSharedChestLiveWithdraw(
        String id,
        Path screenshotsDir,
        ScenarioClient client
    ) {
        List<String> observations = new ArrayList<>();
        Path markerPath = sharedChestLiveMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing shared chest live marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        try {
            ScenarioBlockTarget chestTarget = readMarker(markerPath);
            boolean visible = client.waitForBlock(chestTarget, "minecraft:chest", BLOCK_TIMEOUT);
            ScenarioHeldItem heldItem = client.selectedItem();
            ScenarioUseResult openUse = client.useItemOn(chestTarget, heldItem);
            boolean opened = client.waitForScreenClassName(CONTAINER_SCREEN, INVENTORY_TIMEOUT);
            boolean slotMatched = opened
                && client.waitForContainerSlot(
                    SHARED_CHEST_SLOT,
                    "minecraft:dirt",
                    1,
                    INVENTORY_TIMEOUT
                );
            boolean moved = slotMatched
                && client.moveContainerSlotToInventory(
                    SHARED_CHEST_SLOT,
                    "minecraft:dirt",
                    1,
                    INVENTORY_TIMEOUT
                );
            boolean slotEmpty = moved
                && client.waitForContainerSlotEmpty(SHARED_CHEST_SLOT, INVENTORY_TIMEOUT);
            boolean closed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
            boolean passed = visible && opened && slotMatched && moved && slotEmpty && closed;
            observations.add(
                "shared chest live withdraw: " + (passed ? "passed" : "failed")
                    + " target=" + coordinates(chestTarget)
                    + " visible=" + visible
                    + " held_item=" + heldItem.itemId() + " x" + heldItem.count()
                    + " open_use_result=" + openUse.result()
                    + " screen_matched=" + opened
                    + " slot_matched=" + slotMatched
                    + " moved_to_inventory=" + moved
                    + " slot_empty=" + slotEmpty
                    + " closed=" + closed
            );
            observations.add("screenshots directory available to driver: " + screenshotsDir);
            return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private ClientScenarioReport runTwoClientSharedChestLiveObserveEmpty(
        String id,
        Path screenshotsDir,
        ScenarioClient client
    ) {
        List<String> observations = new ArrayList<>();
        try {
            boolean slotEmpty = client.waitForContainerSlotEmpty(SHARED_CHEST_SLOT, INVENTORY_TIMEOUT);
            boolean closed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
            boolean passed = slotEmpty && closed;
            observations.add(
                "shared chest live update: " + (passed ? "passed" : "failed")
                    + " slot_empty=" + slotEmpty
                    + " closed=" + closed
            );
            observations.add("screenshots directory available to driver: " + screenshotsDir);
            return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private ClientScenarioReport runChestReopenConservation(
        String id,
        Path screenshotsDir,
        ScenarioClient client
    ) {
        List<String> observations = new ArrayList<>();
        try {
            ScenarioBlockPair pair = client.findDryPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
            if (pair == null) {
                return blockedPhase(id, "no loaded dry chest target exists within survival reach");
            }
            ScenarioHeldItem chest = client.giveAndSelect(
                "minecraft:chest",
                1,
                HOTBAR_SLOT,
                SETUP_TIMEOUT
            );
            if (!chest.matches("minecraft:chest", 1)) {
                return blockedPhase(id, "chest setup did not converge");
            }
            ScenarioUseResult placeUse = client.useItemOn(pair.clicked(), chest);
            boolean placed = client.waitForBlock(pair.target(), "minecraft:chest", BLOCK_TIMEOUT);
            if (!placed) {
                observations.add("chest placement failed: use_result=" + placeUse.result());
                return new ClientScenarioReport("failed", id, observations);
            }
            ScenarioBlockTarget chestTarget = new ScenarioBlockTarget(
                pair.target().x(),
                pair.target().y(),
                pair.target().z(),
                "up",
                pair.target().label(),
                "minecraft:chest"
            );
            ScenarioHeldItem emptyHand = client.giveAndSelect(
                "minecraft:air",
                0,
                HOTBAR_SLOT,
                SETUP_TIMEOUT
            );
            if (!emptyHand.matches("minecraft:air", 0)) {
                return blockedPhase(id, "empty-hand setup did not converge before reopen probe");
            }

            ScenarioUseResult firstUse = client.useItemOn(chestTarget, emptyHand);
            boolean firstOpened = client.waitForScreenClassName(CONTAINER_SCREEN, INVENTORY_TIMEOUT);
            boolean firstEmpty = firstOpened
                && client.waitForContainerSlotEmpty(SHARED_CHEST_SLOT, INVENTORY_TIMEOUT);
            boolean firstClosed = firstOpened && client.closeCurrentScreen(INVENTORY_TIMEOUT);

            ScenarioUseResult secondUse = client.useItemOn(chestTarget, emptyHand);
            boolean secondOpened = firstClosed
                && client.waitForScreenClassName(CONTAINER_SCREEN, INVENTORY_TIMEOUT);
            boolean secondEmpty = secondOpened
                && client.waitForContainerSlotEmpty(SHARED_CHEST_SLOT, INVENTORY_TIMEOUT);
            boolean secondClosed = secondOpened && client.closeCurrentScreen(INVENTORY_TIMEOUT);
            boolean passed = firstOpened
                && firstEmpty
                && firstClosed
                && secondOpened
                && secondEmpty
                && secondClosed;
            observations.add(
                "chest reopen conservation: " + (passed ? "passed" : "failed")
                    + " first_use=" + firstUse.result()
                    + " first_opened=" + firstOpened
                    + " first_empty=" + firstEmpty
                    + " first_closed=" + firstClosed
                    + " second_use=" + secondUse.result()
                    + " second_opened=" + secondOpened
                    + " second_empty=" + secondEmpty
                    + " second_closed=" + secondClosed
            );
            observations.add("screenshots directory available to driver: " + screenshotsDir);
            return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private static ClientScenarioReport blockedPhase(String id, String reason) {
        return new ClientScenarioReport("blocked", id, List.of("blocked: " + reason));
    }

    private static boolean movePrimaryToSharedChestSetup(
        ScenarioClient client,
        List<String> observations
    ) throws Exception {
        boolean teleported = client.teleportTo(
            SHARED_CHEST_SETUP_X,
            SHARED_CHEST_SETUP_Y,
            SHARED_CHEST_SETUP_Z,
            SETUP_TIMEOUT
        );
        observations.add(
            "shared chest primary setup position: " + (teleported ? "passed" : "blocked")
                + " x=" + SHARED_CHEST_SETUP_X
                + " y=" + SHARED_CHEST_SETUP_Y
                + " z=" + SHARED_CHEST_SETUP_Z
        );
        return teleported;
    }

    private static ChestProbeResult runSimpleChestOpenProbe(
        ScenarioClient client,
        List<String> observations
    ) throws Exception {
        ScenarioBlockPair pair = client.findDryPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
        if (pair == null) {
            observations.add("blocked: no loaded dry placeable chest target found within survival reach");
            return ChestProbeResult.blocked();
        }

        ScenarioHeldItem chest = client.giveAndSelect("minecraft:chest", 1, 1, SETUP_TIMEOUT);
        if (!chest.matches("minecraft:chest", 1)) {
            observations.add(
                "blocked: expected chest setup, saw " + chest.itemId() + " x" + chest.count()
            );
            return ChestProbeResult.blocked();
        }
        ScenarioUseResult placeUse = client.useItemOn(pair.clicked(), chest);
        boolean placed = client.waitForBlock(pair.target(), "minecraft:chest", BLOCK_TIMEOUT);

        ScenarioHeldItem emptyHand = client.giveAndSelect("minecraft:air", 0, 1, SETUP_TIMEOUT);
        if (!emptyHand.matches("minecraft:air", 0)) {
            observations.add(
                "blocked: expected empty-hand setup before chest open, saw "
                    + emptyHand.itemId() + " x" + emptyHand.count()
            );
            return ChestProbeResult.blocked();
        }
        ScenarioBlockTarget chestTarget = new ScenarioBlockTarget(
            pair.target().x(),
            pair.target().y(),
            pair.target().z(),
            "up",
            pair.target().label(),
            "minecraft:chest"
        );
        ScenarioUseResult openUse = client.useItemOn(chestTarget, emptyHand);
        boolean opened = client.waitForScreenClassName(CONTAINER_SCREEN, INVENTORY_TIMEOUT);
        boolean closed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
        boolean passed = placed && opened && closed;
        observations.add(
            "simple chest open: " + (passed ? "passed" : "failed")
                + " place_use_result=" + placeUse.result()
                + " placed=" + placed
                + " open_use_result=" + openUse.result()
                + " screen=" + CONTAINER_SCREEN
                + " screen_matched=" + opened
                + " closed=" + closed
        );
        return passed ? ChestProbeResult.passed() : ChestProbeResult.failed();
    }

    private static Path sharedChestMarkerPath(Path screenshotsDir) {
        Path runDir = screenshotsDir.getParent();
        return (runDir == null ? Path.of(".") : runDir).resolve(SHARED_CHEST_MARKER_FILE);
    }

    private static Path sharedChestLiveMarkerPath(Path screenshotsDir) {
        Path runDir = screenshotsDir.getParent();
        return (runDir == null ? Path.of(".") : runDir).resolve(SHARED_CHEST_LIVE_MARKER_FILE);
    }

    private static void writeMarker(Path path, ScenarioBlockTarget target) throws IOException {
        Path parent = path.getParent();
        if (parent != null) {
            Files.createDirectories(parent);
        }
        Files.writeString(
            path,
            "x=" + target.x() + "\n"
                + "y=" + target.y() + "\n"
                + "z=" + target.z() + "\n"
                + "face=" + target.face() + "\n"
                + "block_id=" + target.blockId() + "\n"
        );
    }

    private static ScenarioBlockTarget readMarker(Path path) throws IOException {
        Integer x = null;
        Integer y = null;
        Integer z = null;
        String face = null;
        String blockId = null;
        for (String line : Files.readAllLines(path)) {
            String[] parts = line.split("=", 2);
            if (parts.length != 2) {
                continue;
            }
            switch (parts[0]) {
                case "x" -> x = parseMarkerInt(path, "x", parts[1]);
                case "y" -> y = parseMarkerInt(path, "y", parts[1]);
                case "z" -> z = parseMarkerInt(path, "z", parts[1]);
                case "face" -> face = parts[1];
                case "block_id" -> blockId = parts[1];
                default -> {
                }
            }
        }
        if (
            x == null
                || y == null
                || z == null
                || face == null
                || face.isBlank()
                || blockId == null
                || blockId.isBlank()
        ) {
            throw new IOException("invalid shared chest marker: missing x, y, z, face, or block_id in " + path);
        }
        return new ScenarioBlockTarget(x, y, z, face, "shared-chest-marker", blockId);
    }

    private static int parseMarkerInt(Path path, String key, String value) throws IOException {
        try {
            return Integer.parseInt(value);
        } catch (NumberFormatException error) {
            throw new IOException("invalid shared chest marker: " + key + "=" + value + " in " + path, error);
        }
    }

    private static String coordinates(ScenarioBlockTarget target) {
        return target.x() + "," + target.y() + "," + target.z() + "/" + target.face();
    }

    private record ChestProbeResult(String result) {
        static ChestProbeResult passed() {
            return new ChestProbeResult("passed");
        }

        static ChestProbeResult failed() {
            return new ChestProbeResult("failed");
        }

        static ChestProbeResult blocked() {
            return new ChestProbeResult("blocked");
        }

        boolean isFailed() {
            return "failed".equals(result);
        }
    }
}
