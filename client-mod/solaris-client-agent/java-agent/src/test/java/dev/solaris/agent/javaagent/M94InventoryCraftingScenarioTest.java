package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class M94InventoryCraftingScenarioTest {
    @Test
    void runsFocusedInventoryRecipeScenarioThroughClientActions() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new M94InventoryCraftingScenario().run(
            "m94-03a-inventory-oak-log-to-planks",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "give:minecraft:oak_log:1:0",
            "count:minecraft:oak_log",
            "count:minecraft:oak_planks",
            "recipe:0:697:false",
            "inventory:minecraft:oak_log:0",
            "inventory:minecraft:oak_planks:8"
        ), client.operations);
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("inventory recipe: passed")),
            "scenario report must name the recipe execution result"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("degraded: crafting table")),
            "scenario report must keep broad inventory/container paths degraded"
        );
    }

    @Test
    void broadInventoryScenarioRunsRecipeProbeAndBlocksRemainingSubrows() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new M94InventoryCraftingScenario().run(
            "m94-03-inventory-crafting-containers-stations",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("blocked", report.result());
        assertEquals(List.of(
            "give:minecraft:oak_log:1:0",
            "count:minecraft:oak_log",
            "count:minecraft:oak_planks",
            "recipe:0:697:false",
            "inventory:minecraft:oak_log:0",
            "inventory:minecraft:oak_planks:8",
            "findDry:WITHIN_SURVIVAL_REACH",
            "give:minecraft:chest:1:1",
            "use:within-reach-place-clicked:minecraft:chest",
            "block:within-reach-place-target:minecraft:chest",
            "give:minecraft:air:0:1",
            "use:within-reach-place-target:minecraft:air",
            "screen:net.minecraft.client.gui.screens.inventory.ContainerScreen",
            "closeScreen"
        ), client.operations);
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("inventory recipe: passed")),
            "broad scenario must still record the real recipe probe"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("simple chest open: passed")),
            "broad scenario must record the real chest open probe"
        );
        for (String phase : List.of(
            M94InventoryCraftingScenario.TABLE_CRAFT_ID,
            M94InventoryCraftingScenario.FURNACE_UI_ID,
            M94InventoryCraftingScenario.MALFORMED_REJECTION_ID
        )) {
            assertTrue(
                report.observations().stream().anyMatch(entry -> entry.contains("focused phase blocked: " + phase)),
                "broad scenario must name blocked phase " + phase
            );
        }
        assertTrue(
            report.observations().stream().anyMatch(
                entry -> entry.contains("focused phase available: " + M94InventoryCraftingScenario.REOPEN_CONSERVATION_ID)
            ),
            "broad scenario must name the executable reopen phase"
        );
    }

    @Test
    void twoClientSharedChestDepositWritesMarkerAndTransfersSelectedItem(@TempDir Path runDir)
        throws Exception {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new M94InventoryCraftingScenario().run(
            "m94-03b-two-client-shared-chest-deposit",
            runDir.resolve("screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "teleport:2.5:81.0:0.5",
            "findDry:WITHIN_SURVIVAL_REACH",
            "give:minecraft:chest:1:0",
            "use:within-reach-place-clicked:minecraft:chest",
            "block:within-reach-place-target:minecraft:chest",
            "give:minecraft:dirt:1:0",
            "use:within-reach-place-target:minecraft:dirt",
            "screen:net.minecraft.client.gui.screens.inventory.ContainerScreen",
            "moveContainerSlot:0:minecraft:dirt:1",
            "containerSlot:0:minecraft:dirt:1",
            "closeScreen"
        ), client.operations);
        String marker = Files.readString(runDir.resolve("m94-03b-shared-chest-marker.properties"));
        assertTrue(marker.contains("x=1"));
        assertTrue(marker.contains("block_id=minecraft:chest"));
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("shared chest deposit: passed")),
            "deposit scenario must record the real container transfer result"
        );
    }

    @Test
    void twoClientSharedChestObserverOpensMarkedChestAndReadsSlot(@TempDir Path runDir)
        throws Exception {
        Files.writeString(
            runDir.resolve("m94-03b-shared-chest-marker.properties"),
            "x=9\n"
                + "y=64\n"
                + "z=9\n"
                + "face=up\n"
                + "block_id=minecraft:chest\n"
        );
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new M94InventoryCraftingScenario().run(
            "m94-03b-two-client-shared-chest-observe",
            runDir.resolve("screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "block:shared-chest-marker:minecraft:chest",
            "selected",
            "use:shared-chest-marker:minecraft:dirt",
            "screen:net.minecraft.client.gui.screens.inventory.ContainerScreen",
            "containerSlot:0:minecraft:dirt:1",
            "closeScreen"
        ), client.operations);
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("shared chest observe: passed")),
            "observer scenario must record the secondary container slot read"
        );
    }

    @Test
    void twoClientSharedChestLiveOpenDepositsAndKeepsPrimaryScreenOpen(@TempDir Path runDir)
        throws Exception {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new M94InventoryCraftingScenario().run(
            "m94-03c-two-client-shared-chest-open-with-dirt",
            runDir.resolve("screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "teleport:2.5:81.0:0.5",
            "findDry:WITHIN_SURVIVAL_REACH",
            "give:minecraft:chest:1:0",
            "use:within-reach-place-clicked:minecraft:chest",
            "block:within-reach-place-target:minecraft:chest",
            "give:minecraft:dirt:1:0",
            "use:within-reach-place-target:minecraft:dirt",
            "screen:net.minecraft.client.gui.screens.inventory.ContainerScreen",
            "moveContainerSlot:0:minecraft:dirt:1",
            "containerSlot:0:minecraft:dirt:1"
        ), client.operations);
        String marker = Files.readString(runDir.resolve("m94-03c-live-shared-chest-marker.properties"));
        assertTrue(marker.contains("x=1"));
        assertTrue(marker.contains("block_id=minecraft:chest"));
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("shared chest live open: passed")),
            "live-open scenario must record the primary deposit result"
        );
    }

    @Test
    void twoClientSharedChestLiveWithdrawMovesSecondarySlotToInventory(@TempDir Path runDir)
        throws Exception {
        Files.writeString(
            runDir.resolve("m94-03c-live-shared-chest-marker.properties"),
            "x=9\n"
                + "y=64\n"
                + "z=9\n"
                + "face=up\n"
                + "block_id=minecraft:chest\n"
        );
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new M94InventoryCraftingScenario().run(
            "m94-03c-two-client-shared-chest-withdraw",
            runDir.resolve("screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "block:shared-chest-marker:minecraft:chest",
            "selected",
            "use:shared-chest-marker:minecraft:dirt",
            "screen:net.minecraft.client.gui.screens.inventory.ContainerScreen",
            "containerSlot:0:minecraft:dirt:1",
            "moveContainerToInventory:0:minecraft:dirt:1",
            "containerSlotEmpty:0",
            "closeScreen"
        ), client.operations);
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("shared chest live withdraw: passed")),
            "withdraw scenario must record the secondary mutation result"
        );
    }

    @Test
    void twoClientSharedChestLivePrimaryObservesPeerRemovalBeforeClose() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new M94InventoryCraftingScenario().run(
            "m94-03c-two-client-shared-chest-observe-empty",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "containerSlotEmpty:0",
            "closeScreen"
        ), client.operations);
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("shared chest live update: passed")),
            "primary observer must record the peer mutation arriving on the open screen"
        );
    }

    @Test
    void chestReopenPhaseOpensClosesAndReopensTheSameEmptyContainer() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new M94InventoryCraftingScenario().run(
            M94InventoryCraftingScenario.REOPEN_CONSERVATION_ID,
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertEquals(
            2,
            client.operations.stream().filter(operation -> operation.startsWith("screen:")).count(),
            "reopen phase must observe two distinct screen opens"
        );
        assertEquals(
            2,
            client.operations.stream().filter("closeScreen"::equals).count(),
            "reopen phase must close both views"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("chest reopen conservation: passed"))
        );
    }

    @Test
    void unsupportedFocusedContainerPhasesFailClosedByExactId() {
        for (String id : List.of(
            M94InventoryCraftingScenario.TABLE_CRAFT_ID,
            M94InventoryCraftingScenario.FURNACE_UI_ID,
            M94InventoryCraftingScenario.MALFORMED_REJECTION_ID
        )) {
            ClientScenarioReport report = new M94InventoryCraftingScenario().run(
                id,
                Path.of("run/screenshots"),
                new FakeScenarioClient()
            );
            assertEquals("blocked", report.result(), id);
            assertEquals(id, report.id());
            assertTrue(report.observations().get(0).startsWith("blocked:"), id);
        }
    }

    @Test
    void blocksWhenOakLogSetupDoesNotConverge() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.setupItem = new ScenarioHeldItem("minecraft:air", 0);

        ClientScenarioReport report = new M94InventoryCraftingScenario().run(
            "m94-03a-inventory-oak-log-to-planks",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("blocked", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("expected oak log")),
            "blocked report must explain that setup failed"
        );
    }

    @Test
    void blocksUnknownScenarioIds() {
        ClientScenarioReport report = new M94InventoryCraftingScenario().run(
            "m94-unknown",
            Path.of("run/screenshots"),
            new FakeScenarioClient()
        );

        assertEquals("blocked", report.result());
        assertTrue(report.observations().get(0).contains("unsupported scenario"));
    }

    private static final class FakeScenarioClient implements ScenarioClient {
        static final ScenarioBlockPair CHEST_PAIR = new ScenarioBlockPair(
            new ScenarioBlockTarget(1, 64, 1, "up", "within-reach-place-clicked", "minecraft:dirt"),
            new ScenarioBlockTarget(1, 65, 1, "down", "within-reach-place-target", "minecraft:air")
        );

        final List<String> operations = new ArrayList<>();
        ScenarioHeldItem setupItem;
        ScenarioHeldItem selectedItem = new ScenarioHeldItem("minecraft:dirt", 1);
        int oakLogCount = 1;
        int oakPlanksCount = 4;
        boolean teleportSucceeds = true;

        @Override
        public ScenarioBlockPair findOccupiedPair(ScenarioReach reach) {
            throw new UnsupportedOperationException("not used by inventory crafting scenario");
        }

        @Override
        public ScenarioBlockPair findPlaceablePair(ScenarioReach reach) {
            throw new UnsupportedOperationException("not used by inventory crafting scenario");
        }

        @Override
        public ScenarioBlockPair findDryPlaceablePair(ScenarioReach reach) {
            operations.add("findDry:" + reach);
            return CHEST_PAIR;
        }

        @Override
        public ScenarioHeldItem giveAndSelect(String itemId, int count, int hotbarSlot, Duration timeout) {
            operations.add("give:" + itemId + ":" + count + ":" + hotbarSlot);
            if (setupItem != null) {
                return setupItem;
            }
            if (count == 0) {
                return new ScenarioHeldItem("minecraft:air", 0);
            }
            return new ScenarioHeldItem(itemId, count);
        }

        @Override
        public ScenarioUseResult useItemOn(ScenarioBlockTarget clicked, ScenarioHeldItem heldItem) {
            operations.add("use:" + clicked.label() + ":" + heldItem.itemId());
            return new ScenarioUseResult("SUCCESS");
        }

        @Override
        public boolean waitForBlock(ScenarioBlockTarget target, String blockId, Duration duration) {
            operations.add("block:" + target.label() + ":" + blockId);
            return true;
        }

        @Override
        public boolean waitForStableBlocks(ScenarioBlockPair pair, Duration duration) {
            throw new UnsupportedOperationException("not used by inventory crafting scenario");
        }

        @Override
        public boolean waitForNoFluid(ScenarioBlockTarget target, Duration duration) {
            throw new UnsupportedOperationException("not used by inventory crafting scenario");
        }

        @Override
        public ScenarioBreakResult breakBlock(
            ScenarioBlockTarget target,
            String expectedDropItemId,
            int expectedSelectedCount,
            Duration timeout
        ) {
            throw new UnsupportedOperationException("not used by inventory crafting scenario");
        }

        @Override
        public ScenarioHeldItem selectedItem() {
            operations.add("selected");
            return selectedItem;
        }

        @Override
        public void placeRecipe(int containerId, int recipeDisplayId, boolean useMaxItems) {
            operations.add("recipe:" + containerId + ":" + recipeDisplayId + ":" + useMaxItems);
        }

        @Override
        public boolean teleportTo(double x, double y, double z, Duration timeout) {
            operations.add(String.format(Locale.ROOT, "teleport:%.1f:%.1f:%.1f", x, y, z));
            return teleportSucceeds;
        }

        @Override
        public int inventoryCount(String itemId) {
            operations.add("count:" + itemId);
            return switch (itemId) {
                case "minecraft:oak_log" -> oakLogCount;
                case "minecraft:oak_planks" -> oakPlanksCount;
                default -> 0;
            };
        }

        @Override
        public boolean waitForInventoryCount(String itemId, int count, Duration duration) {
            operations.add("inventory:" + itemId + ":" + count);
            return true;
        }

        @Override
        public boolean waitForScreenClassName(String className, Duration duration) {
            operations.add("screen:" + className);
            return true;
        }

        @Override
        public boolean closeCurrentScreen(Duration duration) {
            operations.add("closeScreen");
            return true;
        }

        @Override
        public boolean moveSelectedItemToContainerSlot(
            int containerSlot,
            String itemId,
            int count,
            Duration duration
        ) {
            operations.add("moveContainerSlot:" + containerSlot + ":" + itemId + ":" + count);
            return true;
        }

        @Override
        public boolean waitForContainerSlot(
            int containerSlot,
            String itemId,
            int count,
            Duration duration
        ) {
            operations.add("containerSlot:" + containerSlot + ":" + itemId + ":" + count);
            return true;
        }

        @Override
        public boolean moveContainerSlotToInventory(
            int containerSlot,
            String itemId,
            int count,
            Duration duration
        ) {
            operations.add("moveContainerToInventory:" + containerSlot + ":" + itemId + ":" + count);
            return true;
        }

        @Override
        public boolean waitForContainerSlotEmpty(int containerSlot, Duration duration) {
            operations.add("containerSlotEmpty:" + containerSlot);
            return true;
        }
    }
}
