package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;

import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

final class PlayableBuildingPlacementScenario {
    static final String ID = "playable-48-wall-torch-stairs-slabs";

    private static final String CRAFTING_SCREEN =
        "net.minecraft.client.gui.screens.inventory.CraftingScreen";
    private static final Duration ACK_TIMEOUT = Duration.ofSeconds(5);
    private static final Duration INVENTORY_TIMEOUT = Duration.ofSeconds(5);
    private static final Duration SCREEN_TIMEOUT = Duration.ofSeconds(5);
    private static final Duration APPROACH_TIMEOUT = Duration.ofSeconds(10);
    private static final Duration HOTBAR_TIMEOUT = Duration.ofSeconds(5);

    private final EarnedBuildingPreparation preparation;

    PlayableBuildingPlacementScenario() {
        PlayableRealClientLoopScenario earned = new PlayableRealClientLoopScenario();
        this.preparation = earned::prepareEarnedBuildingMaterials;
    }

    PlayableBuildingPlacementScenario(EarnedBuildingPreparation preparation) {
        this.preparation = preparation;
    }

    static boolean supports(String id) {
        return ID.equals(id);
    }

    ClientScenarioReport run(String id, Path screenshotsDir, ScenarioClient client) {
        if (!supports(id)) {
            return new ClientScenarioReport("blocked", id, List.of("unsupported scenario id: " + id));
        }

        List<String> observations = new ArrayList<>();
        observations.add("artifact directory available to driver: " + screenshotsDir);
        try {
            EarnedBuildingMaterials materials = preparation.prepare(id, observations, client);
            if (!"passed".equals(materials.report().result())) {
                return materials.report();
            }

            String planksItemId = materials.planksItemId();
            if (planksItemId == null || !planksItemId.endsWith("_planks")) {
                observations.add("blocked: earned planks family cannot derive stairs and slab items");
                return new ClientScenarioReport("blocked", id, observations);
            }
            String woodPrefix = planksItemId.substring(0, planksItemId.length() - "_planks".length());
            String stairsItemId = woodPrefix + "_stairs";
            String slabItemId = woodPrefix + "_slab";

            ClientScenarioReport crafted = craftBuildingItems(
                id,
                observations,
                client,
                materials.tableTarget(),
                planksItemId,
                stairsItemId,
                slabItemId
            );
            if (!"passed".equals(crafted.result())) {
                return crafted;
            }

            ClientScenarioReport wallTorch = placeWallTorch(id, observations, client);
            if (!"passed".equals(wallTorch.result())) {
                return wallTorch;
            }

            ClientScenarioReport stairs = placeBottomStairs(
                id,
                observations,
                client,
                stairsItemId
            );
            if (!"passed".equals(stairs.result())) {
                return stairs;
            }

            ScenarioBlockTarget bottomSlab = placeBottomSlab(
                id,
                observations,
                client,
                slabItemId
            );
            if (bottomSlab == null) {
                return new ClientScenarioReport("failed", id, observations);
            }

            ClientScenarioReport rejected = rejectWallTorchOnSlab(
                id,
                observations,
                client,
                bottomSlab
            );
            if (!"passed".equals(rejected.result())) {
                return rejected;
            }

            ClientScenarioReport topSlab = placeTopSlab(
                id,
                observations,
                client,
                slabItemId
            );
            if (!"passed".equals(topSlab.result())) {
                return topSlab;
            }

            return mergeBottomSlab(id, observations, client, bottomSlab, slabItemId);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private static ClientScenarioReport craftBuildingItems(
        String id,
        List<String> observations,
        ScenarioClient client,
        ScenarioBlockTarget table,
        String planksItemId,
        String stairsItemId,
        String slabItemId
    ) throws Exception {
        if (table == null || !client.approachBlock(table, APPROACH_TIMEOUT)) {
            observations.add("blocked: earned crafting table is not reachable for building recipes");
            return new ClientScenarioReport("blocked", id, observations);
        }

        ScenarioHeldItem hand = client.selectHotbarItem("minecraft:torch", 1, HOTBAR_TIMEOUT);
        ScenarioUseResult openUse = client.useItemOn(table, hand);
        boolean openAcknowledged = client.waitForUseAcknowledgement(openUse, ACK_TIMEOUT);
        boolean opened = openAcknowledged && client.waitForScreenClassName(CRAFTING_SCREEN, SCREEN_TIMEOUT);
        observations.add(
            "building crafting table open: " + (opened ? "passed" : "failed")
                + " acknowledged=" + openAcknowledged
                + " use_result=" + openUse.result()
        );
        if (!opened) {
            return new ClientScenarioReport("failed", id, observations);
        }

        int stairsRecipe = client.recipeDisplayIdForResult(stairsItemId);
        int slabRecipe = client.recipeDisplayIdForResult(slabItemId);
        if (stairsRecipe < 0 || slabRecipe < 0) {
            observations.add(
                "blocked: client recipe book is missing building recipes"
                    + " stairs_item=" + stairsItemId
                    + " stairs_recipe=" + stairsRecipe
                    + " slab_item=" + slabItemId
                    + " slab_recipe=" + slabRecipe
            );
            client.closeCurrentScreen(SCREEN_TIMEOUT);
            return new ClientScenarioReport("blocked", id, observations);
        }

        int containerId = client.activeContainerId();
        int planksBeforeStairs = client.inventoryCount(planksItemId);
        int stairsBefore = client.inventoryCount(stairsItemId);
        if (planksBeforeStairs < 6) {
            observations.add("failed: fewer than six earned planks available for stairs");
            client.closeCurrentScreen(SCREEN_TIMEOUT);
            return new ClientScenarioReport("failed", id, observations);
        }
        client.placeRecipe(containerId, stairsRecipe, false);
        boolean stairsPlanksConsumed = client.waitForInventoryCount(
            planksItemId,
            planksBeforeStairs - 6,
            INVENTORY_TIMEOUT
        );
        boolean stairsCreated = client.waitForInventoryCount(
            stairsItemId,
            stairsBefore + 4,
            INVENTORY_TIMEOUT
        );

        int planksBeforeSlabs = client.inventoryCount(planksItemId);
        int slabsBefore = client.inventoryCount(slabItemId);
        if (!stairsPlanksConsumed || !stairsCreated || planksBeforeSlabs < 3) {
            observations.add("building stairs recipe: failed");
            client.closeCurrentScreen(SCREEN_TIMEOUT);
            return new ClientScenarioReport("failed", id, observations);
        }
        client.placeRecipe(containerId, slabRecipe, false);
        boolean slabPlanksConsumed = client.waitForInventoryCount(
            planksItemId,
            planksBeforeSlabs - 3,
            INVENTORY_TIMEOUT
        );
        boolean slabsCreated = client.waitForInventoryCount(
            slabItemId,
            slabsBefore + 6,
            INVENTORY_TIMEOUT
        );
        boolean closed = client.closeCurrentScreen(SCREEN_TIMEOUT);
        boolean passed = slabPlanksConsumed && slabsCreated && closed;
        observations.add(
            "building recipes: " + (passed ? "passed" : "failed")
                + " planks_item=" + planksItemId
                + " stairs_item=" + stairsItemId
                + " stairs_recipe=" + stairsRecipe
                + " slab_item=" + slabItemId
                + " slab_recipe=" + slabRecipe
                + " screen_closed=" + closed
        );
        return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
    }

    private static ClientScenarioReport placeWallTorch(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        ScenarioBlockPair pair = client.findHorizontalPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
        if (pair == null) {
            observations.add("blocked: no dry horizontal support found for wall torch");
            return new ClientScenarioReport("blocked", id, observations);
        }
        int before = client.inventoryCount("minecraft:torch");
        ScenarioHeldItem torch = client.selectHotbarItem("minecraft:torch", 1, HOTBAR_TIMEOUT);
        ScenarioUseResult use = client.useItemOn(pair.clicked(), torch);
        boolean acknowledged = client.waitForUseAcknowledgement(use, ACK_TIMEOUT);
        boolean blockMatched = acknowledged
            && client.waitForBlock(pair.target(), "minecraft:wall_torch", Duration.ZERO);
        boolean facingMatched = acknowledged
            && client.waitForBlockProperty(
                pair.target(),
                "facing",
                pair.clicked().face(),
                Duration.ZERO
            );
        boolean inventoryDebited = acknowledged
            && client.waitForInventoryCount("minecraft:torch", before - 1, INVENTORY_TIMEOUT);
        int after = client.inventoryCount("minecraft:torch");
        boolean passed = acknowledged && blockMatched && facingMatched && inventoryDebited;
        observations.add(
            "wall torch placement: " + (passed ? "passed" : "failed")
                + " acknowledged=" + acknowledged
                + " expected_facing=" + pair.clicked().face()
                + " facing_matched=" + facingMatched
                + " inventory_debited=" + inventoryDebited
                + " inventory_before=" + before
                + " inventory_after=" + after
        );
        return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
    }

    private static ClientScenarioReport placeBottomStairs(
        String id,
        List<String> observations,
        ScenarioClient client,
        String stairsItemId
    ) throws Exception {
        ScenarioBlockPair pair = client.findHorizontalPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
        if (pair == null) {
            observations.add("blocked: no dry horizontal target found for stairs");
            return new ClientScenarioReport("blocked", id, observations);
        }
        String expectedFacing = pair.clicked().face();
        int before = client.inventoryCount(stairsItemId);
        ScenarioHeldItem stairs = client.selectHotbarItem(stairsItemId, 1, HOTBAR_TIMEOUT);
        ScenarioUseResult use = client.useItemOn(pair.clicked(), stairs);
        boolean acknowledged = client.waitForUseAcknowledgement(use, ACK_TIMEOUT);
        boolean blockMatched = acknowledged
            && client.waitForBlock(pair.target(), stairsItemId, Duration.ZERO);
        boolean facingMatched = acknowledged
            && client.waitForBlockProperty(pair.target(), "facing", expectedFacing, Duration.ZERO);
        boolean halfMatched = acknowledged
            && client.waitForBlockProperty(pair.target(), "half", "bottom", Duration.ZERO);
        boolean inventoryDebited = acknowledged
            && client.waitForInventoryCount(stairsItemId, before - 1, INVENTORY_TIMEOUT);
        int after = client.inventoryCount(stairsItemId);
        boolean passed = acknowledged
            && blockMatched
            && facingMatched
            && halfMatched
            && inventoryDebited;
        observations.add(
            "stairs placement: " + (passed ? "passed" : "failed")
                + " acknowledged=" + acknowledged
                + " expected_facing=" + expectedFacing
                + " facing_matched=" + facingMatched
                + " expected_half=bottom"
                + " half_matched=" + halfMatched
                + " inventory_debited=" + inventoryDebited
                + " inventory_before=" + before
                + " inventory_after=" + after
        );
        return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
    }

    private static ScenarioBlockTarget placeBottomSlab(
        String id,
        List<String> observations,
        ScenarioClient client,
        String slabItemId
    ) throws Exception {
        ScenarioBlockPair pair = client.findVerticalPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
        if (pair == null) {
            observations.add("blocked: no dry upper target found for bottom slab");
            return null;
        }
        int before = client.inventoryCount(slabItemId);
        ScenarioHeldItem slab = client.selectHotbarItem(slabItemId, 1, HOTBAR_TIMEOUT);
        ScenarioUseResult use = client.useItemOn(pair.clicked(), slab);
        boolean acknowledged = client.waitForUseAcknowledgement(use, ACK_TIMEOUT);
        boolean blockMatched = acknowledged
            && client.waitForBlock(pair.target(), slabItemId, Duration.ZERO);
        boolean typeMatched = acknowledged
            && client.waitForBlockProperty(pair.target(), "type", "bottom", Duration.ZERO);
        boolean inventoryDebited = acknowledged
            && client.waitForInventoryCount(slabItemId, before - 1, INVENTORY_TIMEOUT);
        int after = client.inventoryCount(slabItemId);
        boolean passed = acknowledged && blockMatched && typeMatched && inventoryDebited;
        observations.add(
            "bottom slab placement: " + (passed ? "passed" : "failed")
                + " acknowledged=" + acknowledged
                + " type_matched=" + typeMatched
                + " inventory_debited=" + inventoryDebited
                + " inventory_before=" + before
                + " inventory_after=" + after
        );
        return passed ? pair.target() : null;
    }

    private static ClientScenarioReport rejectWallTorchOnSlab(
        String id,
        List<String> observations,
        ScenarioClient client,
        ScenarioBlockTarget bottomSlab
    ) throws Exception {
        ScenarioBlockPair pair = client.findHorizontalAttachmentPair(
            bottomSlab,
            ScenarioReach.WITHIN_SURVIVAL_REACH
        );
        if (pair == null) {
            observations.add("blocked: no dry side target found on the bottom slab");
            return new ClientScenarioReport("blocked", id, observations);
        }
        int before = client.inventoryCount("minecraft:torch");
        ScenarioHeldItem torch = client.selectHotbarItem("minecraft:torch", 1, HOTBAR_TIMEOUT);
        ScenarioUseResult use = client.useItemOn(pair.clicked(), torch);
        boolean acknowledged = client.waitForUseAcknowledgement(use, ACK_TIMEOUT);
        boolean targetStayedAir = acknowledged
            && client.waitForBlock(pair.target(), "minecraft:air", Duration.ZERO);
        boolean supportStayedBottom = acknowledged
            && client.waitForBlockProperty(bottomSlab, "type", "bottom", Duration.ZERO);
        int after = client.inventoryCount("minecraft:torch");
        boolean inventoryUnchanged = after == before;
        boolean passed = acknowledged && targetStayedAir && supportStayedBottom && inventoryUnchanged;
        observations.add(
            "rejected wall torch support: " + (passed ? "passed" : "failed")
                + " acknowledged=" + acknowledged
                + " target_stayed_air=" + targetStayedAir
                + " support_stayed_bottom=" + supportStayedBottom
                + " inventory_unchanged=" + inventoryUnchanged
                + " inventory_before=" + before
                + " inventory_after=" + after
        );
        return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
    }

    private static ClientScenarioReport placeTopSlab(
        String id,
        List<String> observations,
        ScenarioClient client,
        String slabItemId
    ) throws Exception {
        ScenarioBlockPair pair = client.findHorizontalPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
        if (pair == null) {
            observations.add("blocked: no dry horizontal target found for top slab");
            return new ClientScenarioReport("blocked", id, observations);
        }
        int before = client.inventoryCount(slabItemId);
        ScenarioHeldItem slab = client.selectHotbarItem(slabItemId, 1, HOTBAR_TIMEOUT);
        ScenarioUseResult use = client.useItemOnAtHeight(pair.clicked(), slab, 0.75);
        boolean acknowledged = client.waitForUseAcknowledgement(use, ACK_TIMEOUT);
        boolean blockMatched = acknowledged
            && client.waitForBlock(pair.target(), slabItemId, Duration.ZERO);
        boolean typeMatched = acknowledged
            && client.waitForBlockProperty(pair.target(), "type", "top", Duration.ZERO);
        boolean inventoryDebited = acknowledged
            && client.waitForInventoryCount(slabItemId, before - 1, INVENTORY_TIMEOUT);
        int after = client.inventoryCount(slabItemId);
        boolean passed = acknowledged && blockMatched && typeMatched && inventoryDebited;
        observations.add(
            "top slab placement: " + (passed ? "passed" : "failed")
                + " acknowledged=" + acknowledged
                + " type_matched=" + typeMatched
                + " inventory_debited=" + inventoryDebited
                + " inventory_before=" + before
                + " inventory_after=" + after
        );
        return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
    }

    private static ClientScenarioReport mergeBottomSlab(
        String id,
        List<String> observations,
        ScenarioClient client,
        ScenarioBlockTarget bottomSlab,
        String slabItemId
    ) throws Exception {
        int before = client.inventoryCount(slabItemId);
        ScenarioHeldItem slab = client.selectHotbarItem(slabItemId, 1, HOTBAR_TIMEOUT);
        ScenarioBlockTarget mergeTarget = new ScenarioBlockTarget(
            bottomSlab.x(),
            bottomSlab.y(),
            bottomSlab.z(),
            "up",
            "bottom-slab-merge",
            slabItemId
        );
        ScenarioUseResult use = client.useItemOn(mergeTarget, slab);
        boolean acknowledged = client.waitForUseAcknowledgement(use, ACK_TIMEOUT);
        boolean blockMatched = acknowledged
            && client.waitForBlock(bottomSlab, slabItemId, Duration.ZERO);
        boolean typeMatched = acknowledged
            && client.waitForBlockProperty(bottomSlab, "type", "double", Duration.ZERO);
        boolean inventoryDebited = acknowledged
            && client.waitForInventoryCount(slabItemId, before - 1, INVENTORY_TIMEOUT);
        int after = client.inventoryCount(slabItemId);
        boolean passed = acknowledged && blockMatched && typeMatched && inventoryDebited;
        observations.add(
            "matching slab merge: " + (passed ? "passed" : "failed")
                + " acknowledged=" + acknowledged
                + " expected_type=double"
                + " type_matched=" + typeMatched
                + " inventory_debited=" + inventoryDebited
                + " inventory_before=" + before
                + " inventory_after=" + after
        );
        return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
    }

}
