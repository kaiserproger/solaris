package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;
import org.junit.jupiter.api.Test;

import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class M94EnchantingScenarioTest {
    @Test
    void enchantsThroughTheRealContainerContract() {
        FakeClient client = new FakeClient();

        ClientScenarioReport report = new M94EnchantingScenario().run(
            M94EnchantingScenario.ID,
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result(), () -> String.join("\n", report.observations()));
        assertEquals(List.of(
            "give:minecraft:enchanting_table:1:0",
            "use:minecraft:enchanting_table:ground",
            "waitBlock:table:minecraft:enchanting_table",
            "give:minecraft:stone_pickaxe:1:1",
            "give:minecraft:lapis_lazuli:1:2",
            "command:debug survival xp 7",
            "waitExperience:7:1",
            "use:minecraft:lapis_lazuli:table",
            "screen:net.minecraft.client.gui.screens.inventory.EnchantmentScreen",
            "moveToContainer:0:minecraft:stone_pickaxe:1",
            "moveToContainer:1:minecraft:lapis_lazuli:1",
            "button:0",
            "enchantment:0:minecraft:efficiency:1",
            "totalExperience",
            "experienceLevel",
            "moveFromContainer:0:minecraft:stone_pickaxe:1",
            "closeScreen"
        ), client.operations);
        assertTrue(report.observations().stream().anyMatch(line ->
            line.contains("efficiency enchantment: passed")
        ));
    }

    private static final class FakeClient implements ScenarioClient {
        final List<String> operations = new ArrayList<>();
        private final ScenarioBlockTarget ground = new ScenarioBlockTarget(
            0, 63, 0, "up", "ground", "minecraft:grass_block"
        );
        private final ScenarioBlockTarget table = new ScenarioBlockTarget(
            0, 64, 0, "up", "table", "minecraft:air"
        );

        @Override
        public ScenarioBlockPair findDryPlaceablePair(ScenarioReach reach) {
            return new ScenarioBlockPair(ground, table);
        }

        @Override
        public ScenarioHeldItem giveAndSelect(String itemId, int count, int hotbarSlot, Duration timeout) {
            operations.add("give:" + itemId + ":" + count + ":" + hotbarSlot);
            return new ScenarioHeldItem(itemId, count);
        }

        @Override
        public ScenarioUseResult useItemOn(ScenarioBlockTarget clicked, ScenarioHeldItem heldItem) {
            operations.add("use:" + heldItem.itemId() + ":" + clicked.label());
            return new ScenarioUseResult("success");
        }

        @Override
        public boolean waitForBlock(ScenarioBlockTarget target, String blockId, Duration duration) {
            operations.add("waitBlock:" + target.label() + ":" + blockId);
            return true;
        }

        @Override
        public void sendCommand(String command) {
            operations.add("command:" + command);
        }

        @Override
        public boolean waitForExperience(int total, int level, Duration duration) {
            operations.add("waitExperience:" + total + ":" + level);
            return true;
        }

        @Override
        public boolean waitForScreenClassName(String className, Duration duration) {
            operations.add("screen:" + className);
            return true;
        }

        @Override
        public boolean moveSelectedItemToContainerSlot(
            int containerSlot,
            String itemId,
            int count,
            Duration duration
        ) {
            operations.add("moveToContainer:" + containerSlot + ":" + itemId + ":" + count);
            return true;
        }

        @Override
        public boolean clickContainerButton(int buttonId, Duration duration) {
            operations.add("button:" + buttonId);
            return true;
        }

        @Override
        public boolean containerSlotHasEnchantment(int slot, String enchantmentId, int level) {
            operations.add("enchantment:" + slot + ":" + enchantmentId + ":" + level);
            return true;
        }

        @Override
        public int totalExperience() {
            operations.add("totalExperience");
            return 7;
        }

        @Override
        public int experienceLevel() {
            operations.add("experienceLevel");
            return 0;
        }

        @Override
        public boolean moveContainerSlotToInventory(
            int containerSlot,
            String itemId,
            int count,
            Duration duration
        ) {
            operations.add("moveFromContainer:" + containerSlot + ":" + itemId + ":" + count);
            return true;
        }

        @Override
        public boolean closeCurrentScreen(Duration duration) {
            operations.add("closeScreen");
            return true;
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

        @Override
        public ScenarioHeldItem selectedItem() {
            return new ScenarioHeldItem("minecraft:air", 0);
        }
    }
}
