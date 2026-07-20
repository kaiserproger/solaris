package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;
import org.junit.jupiter.api.Test;

import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class PlayableBuildingPlacementScenarioTest {
    private static final ScenarioBlockTarget TABLE =
        target(0, 64, 0, "up", "earned-table", "minecraft:crafting_table");

    @Test
    void placesWallTorchStairsAndBothSlabHalvesThenMergesMatchingSlabs() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = scenario().run(
            PlayableBuildingPlacementScenario.ID,
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertEquals(3, client.count("minecraft:torch"), "one of four earned torches must be debited");
        assertEquals(3, client.count("minecraft:oak_stairs"), "one of four crafted stairs must be debited");
        assertEquals(3, client.count("minecraft:oak_slab"), "three of six crafted slabs must be debited");
        assertEquals("east", client.property(client.wallTarget, "facing"));
        assertEquals("south", client.property(client.stairsTarget, "facing"));
        assertEquals("bottom", client.property(client.stairsTarget, "half"));
        assertEquals("double", client.property(client.bottomSlabTarget, "type"));
        assertEquals("top", client.property(client.topSlabTarget, "type"));
        assertEquals("minecraft:air", client.blockId(client.rejectedTorchTarget));
        assertTrue(client.operations.contains("recipe:minecraft:oak_stairs"));
        assertTrue(client.operations.contains("recipe:minecraft:oak_slab"));
        assertTrue(client.operations.contains("use-height:0.75:minecraft:oak_slab:top-slab-support"));
        assertEquals(
            7,
            client.operations.stream().filter(operation -> operation.startsWith("ack:")).count(),
            "every accepted placement and the rejected wall torch must cross an applied block ack"
        );
        assertFalse(
            client.operations.stream().anyMatch(operation -> operation.startsWith("debug:")),
            "building scenario must not use debug setup"
        );
    }

    @Test
    void failsWhenRejectedWallTorchConsumesInventoryAfterAcknowledgement() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.debitRejectedTorch = true;

        ClientScenarioReport report = scenario().run(
            PlayableBuildingPlacementScenario.ID,
            Path.of("run/screenshots"),
            client
        );

        assertEquals("failed", report.result());
        assertTrue(report.observations().stream().anyMatch(observation ->
            observation.contains("rejected wall torch support: failed")
                && observation.contains("inventory_unchanged=false")
        ));
        int rejectedUse = client.operations.indexOf("use:minecraft:torch:unsupported-slab-side");
        int rejectedAck = client.operations.indexOf("ack:unsupported-slab-side");
        int rejectedCount = client.operations.lastIndexOf("count:minecraft:torch");
        assertTrue(rejectedUse < rejectedAck && rejectedAck < rejectedCount);
    }

    @Test
    void failsWhenAcknowledgedStairFacingDoesNotMatchThePlacementDirection() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.wrongStairFacing = true;

        ClientScenarioReport report = scenario().run(
            PlayableBuildingPlacementScenario.ID,
            Path.of("run/screenshots"),
            client
        );

        assertEquals("failed", report.result());
        assertTrue(report.observations().stream().anyMatch(observation ->
            observation.contains("stairs placement: failed")
                && observation.contains("expected_facing=south")
        ));
    }

    private static PlayableBuildingPlacementScenario scenario() {
        return new PlayableBuildingPlacementScenario((id, observations, client) -> {
            FakeScenarioClient fake = (FakeScenarioClient) client;
            fake.inventory.put("minecraft:torch", 4);
            fake.inventory.put("minecraft:oak_planks", 9);
            observations.add("earned building materials: passed");
            return new EarnedBuildingMaterials(
                new ClientScenarioReport("passed", id, observations),
                "minecraft:oak_planks",
                TABLE
            );
        });
    }

    private static ScenarioBlockTarget target(
        int x,
        int y,
        int z,
        String face,
        String label,
        String blockId
    ) {
        return new ScenarioBlockTarget(x, y, z, face, label, blockId);
    }

    private static final class FakeScenarioClient implements ScenarioClient {
        final List<String> operations = new ArrayList<>();
        final Map<String, Integer> inventory = new HashMap<>();
        final Map<String, String> blocks = new HashMap<>();
        final Map<String, Map<String, String>> properties = new HashMap<>();
        final ScenarioBlockTarget wallTarget = target(2, 64, 0, "west", "wall-target", "minecraft:air");
        final ScenarioBlockTarget stairsTarget = target(4, 64, 0, "north", "stairs-target", "minecraft:air");
        final ScenarioBlockTarget bottomSlabTarget =
            target(6, 65, 0, "down", "bottom-slab-target", "minecraft:air");
        final ScenarioBlockTarget rejectedTorchTarget =
            target(7, 65, 0, "west", "unsupported-wall-torch-target", "minecraft:air");
        final ScenarioBlockTarget topSlabTarget =
            target(9, 64, 0, "east", "top-slab-target", "minecraft:air");
        boolean debitRejectedTorch;
        boolean wrongStairFacing;
        int horizontalPairIndex;

        FakeScenarioClient() {
            blocks.put(key(TABLE), "minecraft:crafting_table");
        }

        int count(String itemId) {
            return inventory.getOrDefault(itemId, 0);
        }

        String blockId(ScenarioBlockTarget target) {
            return blocks.getOrDefault(key(target), "minecraft:air");
        }

        String property(ScenarioBlockTarget target, String property) {
            return properties.getOrDefault(key(target), Map.of()).get(property);
        }

        @Override
        public ScenarioHeldItem selectedItem() {
            return new ScenarioHeldItem("minecraft:air", 0);
        }

        @Override
        public ScenarioBlockPair findOccupiedPair(ScenarioReach reach) {
            throw new UnsupportedOperationException();
        }

        @Override
        public ScenarioBlockPair findPlaceablePair(ScenarioReach reach) {
            throw new UnsupportedOperationException();
        }

        @Override
        public ScenarioBlockPair findDryPlaceablePair(ScenarioReach reach) {
            throw new UnsupportedOperationException();
        }

        @Override
        public ScenarioBlockPair findHorizontalPlaceablePair(ScenarioReach reach) {
            ScenarioBlockPair pair = switch (horizontalPairIndex++) {
                case 0 -> new ScenarioBlockPair(
                    target(1, 64, 0, "east", "wall-support", "minecraft:dirt"),
                    wallTarget
                );
                case 1 -> new ScenarioBlockPair(
                    target(4, 64, -1, "south", "stairs-support", "minecraft:dirt"),
                    stairsTarget
                );
                case 2 -> new ScenarioBlockPair(
                    target(10, 64, 0, "west", "top-slab-support", "minecraft:dirt"),
                    topSlabTarget
                );
                default -> throw new IllegalStateException("unexpected horizontal placement request");
            };
            operations.add("find-horizontal:" + pair.clicked().label());
            return pair;
        }

        @Override
        public ScenarioBlockPair findVerticalPlaceablePair(ScenarioReach reach) {
            operations.add("find-vertical:bottom-slab-support");
            return new ScenarioBlockPair(
                target(6, 64, 0, "up", "bottom-slab-support", "minecraft:dirt"),
                bottomSlabTarget
            );
        }

        @Override
        public ScenarioBlockPair findHorizontalAttachmentPair(
            ScenarioBlockTarget support,
            ScenarioReach reach
        ) {
            operations.add("find-attachment:" + support.label());
            return new ScenarioBlockPair(
                target(
                    support.x(),
                    support.y(),
                    support.z(),
                    "east",
                    "unsupported-slab-side",
                    "minecraft:oak_slab"
                ),
                rejectedTorchTarget
            );
        }

        @Override
        public int recipeDisplayIdForResult(String itemId) {
            operations.add("recipe:" + itemId);
            return switch (itemId) {
                case "minecraft:oak_stairs" -> 80;
                case "minecraft:oak_slab" -> 81;
                default -> -1;
            };
        }

        @Override
        public void placeRecipe(int containerId, int recipeDisplayId, boolean useMaxItems) {
            operations.add("place-recipe:" + containerId + ":" + recipeDisplayId);
            if (recipeDisplayId == 80) {
                debit("minecraft:oak_planks", 6);
                credit("minecraft:oak_stairs", 4);
            } else if (recipeDisplayId == 81) {
                debit("minecraft:oak_planks", 3);
                credit("minecraft:oak_slab", 6);
            }
        }

        @Override
        public int inventoryCount(String itemId) {
            operations.add("count:" + itemId);
            return count(itemId);
        }

        @Override
        public boolean waitForInventoryCount(String itemId, int count, Duration duration) {
            operations.add("inventory:" + itemId + ":" + count);
            return count(itemId) == count;
        }

        @Override
        public boolean approachBlock(ScenarioBlockTarget target, Duration timeout) {
            operations.add("approach:" + target.label());
            return true;
        }

        @Override
        public ScenarioHeldItem selectHotbarItem(String itemId, int count, Duration timeout) {
            operations.add("select:" + itemId + ":" + count);
            return new ScenarioHeldItem(itemId, count(itemId));
        }

        @Override
        public ScenarioUseResult useItemOn(ScenarioBlockTarget clicked, ScenarioHeldItem heldItem) {
            operations.add("use:" + heldItem.itemId() + ":" + clicked.label());
            applyUse(clicked, heldItem, 0.5);
            return new ScenarioUseResult("success", operations.size());
        }

        @Override
        public ScenarioUseResult useItemOnAtHeight(
            ScenarioBlockTarget clicked,
            ScenarioHeldItem heldItem,
            double cursorHeight
        ) {
            operations.add("use-height:" + cursorHeight + ":" + heldItem.itemId() + ":" + clicked.label());
            applyUse(clicked, heldItem, cursorHeight);
            return new ScenarioUseResult("success", operations.size());
        }

        private void applyUse(ScenarioBlockTarget clicked, ScenarioHeldItem heldItem, double cursorHeight) {
            String itemId = heldItem.itemId();
            if ("minecraft:crafting_table".equals(clicked.blockId())) {
                return;
            }
            if ("wall-support".equals(clicked.label())) {
                place(wallTarget, "minecraft:wall_torch", Map.of("facing", "east"));
                debit(itemId, 1);
            } else if ("stairs-support".equals(clicked.label())) {
                place(
                    stairsTarget,
                    "minecraft:oak_stairs",
                    Map.of("facing", wrongStairFacing ? "north" : "south", "half", "bottom")
                );
                debit(itemId, 1);
            } else if ("bottom-slab-support".equals(clicked.label())) {
                place(bottomSlabTarget, "minecraft:oak_slab", Map.of("type", "bottom"));
                debit(itemId, 1);
            } else if ("unsupported-slab-side".equals(clicked.label())) {
                if (debitRejectedTorch) {
                    debit(itemId, 1);
                }
            } else if ("top-slab-support".equals(clicked.label()) && cursorHeight > 0.5) {
                place(topSlabTarget, "minecraft:oak_slab", Map.of("type", "top"));
                debit(itemId, 1);
            } else if ("bottom-slab-merge".equals(clicked.label())) {
                place(bottomSlabTarget, "minecraft:oak_slab", Map.of("type", "double"));
                debit(itemId, 1);
            }
        }

        @Override
        public boolean waitForUseAcknowledgement(ScenarioUseResult use, Duration timeout) {
            String lastUse = operations.stream()
                .filter(operation -> operation.startsWith("use:" ) || operation.startsWith("use-height:"))
                .reduce((first, second) -> second)
                .orElseThrow();
            String label = lastUse.substring(lastUse.lastIndexOf(':') + 1);
            operations.add("ack:" + label);
            return true;
        }

        @Override
        public boolean waitForBlock(ScenarioBlockTarget target, String blockId, Duration duration) {
            operations.add("block:" + target.label() + ":" + blockId);
            return blockId.equals(blockId(target));
        }

        @Override
        public boolean waitForBlockProperty(
            ScenarioBlockTarget target,
            String property,
            String value,
            Duration duration
        ) {
            operations.add("property:" + target.label() + ":" + property + ":" + value);
            return value.equals(property(target, property));
        }

        @Override
        public boolean waitForScreenClassName(String className, Duration duration) {
            operations.add("screen:" + className);
            return true;
        }

        @Override
        public boolean closeCurrentScreen(Duration duration) {
            operations.add("close-screen");
            return true;
        }

        @Override
        public int activeContainerId() {
            return 7;
        }

        @Override
        public ScenarioHeldItem giveAndSelect(String itemId, int count, int hotbarSlot, Duration timeout) {
            operations.add("debug:give");
            return new ScenarioHeldItem(itemId, count);
        }

        @Override
        public boolean waitForStableBlocks(ScenarioBlockPair pair, Duration duration) {
            throw new UnsupportedOperationException();
        }

        @Override
        public boolean waitForNoFluid(ScenarioBlockTarget target, Duration duration) {
            throw new UnsupportedOperationException();
        }

        @Override
        public ScenarioBreakResult breakBlock(
            ScenarioBlockTarget target,
            String expectedDropItemId,
            int expectedSelectedCount,
            Duration timeout
        ) {
            throw new UnsupportedOperationException();
        }

        private void place(ScenarioBlockTarget target, String blockId, Map<String, String> state) {
            blocks.put(key(target), blockId);
            properties.put(key(target), new HashMap<>(state));
        }

        private void debit(String itemId, int count) {
            inventory.put(itemId, count(itemId) - count);
        }

        private void credit(String itemId, int count) {
            inventory.put(itemId, count(itemId) + count);
        }

        private static String key(ScenarioBlockTarget target) {
            return target.x() + ":" + target.y() + ":" + target.z();
        }
    }
}
