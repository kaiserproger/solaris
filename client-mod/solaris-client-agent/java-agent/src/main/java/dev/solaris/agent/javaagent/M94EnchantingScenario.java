package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;

import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

final class M94EnchantingScenario {
    static final String ID = "m94-08-enchanting-efficiency";
    private static final String ENCHANTMENT_SCREEN =
        "net.minecraft.client.gui.screens.inventory.EnchantmentScreen";
    private static final Duration SETUP_TIMEOUT = Duration.ofSeconds(8);
    private static final Duration BLOCK_TIMEOUT = Duration.ofSeconds(3);
    private static final Duration CONTAINER_TIMEOUT = Duration.ofSeconds(8);

    ClientScenarioReport run(String id, Path screenshotsDir, ScenarioClient client) {
        if (!ID.equals(id)) {
            return new ClientScenarioReport("blocked", id, List.of("unsupported scenario id: " + id));
        }

        List<String> observations = new ArrayList<>();
        try {
            ScenarioBlockPair pair = client.findDryPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
            if (pair == null) {
                observations.add("blocked: no loaded enchanting-table target within survival reach");
                return new ClientScenarioReport("blocked", id, observations);
            }

            ScenarioHeldItem table = client.giveAndSelect(
                "minecraft:enchanting_table",
                1,
                0,
                SETUP_TIMEOUT
            );
            ScenarioUseResult placement = client.useItemOn(pair.clicked(), table);
            boolean tablePlaced = client.waitForBlock(
                pair.target(),
                "minecraft:enchanting_table",
                BLOCK_TIMEOUT
            );
            observations.add(
                "enchanting table placement: " + (tablePlaced ? "passed" : "failed")
                    + " use_result=" + placement.result()
            );
            if (!tablePlaced) {
                return new ClientScenarioReport("failed", id, observations);
            }

            ScenarioHeldItem pickaxe = client.giveAndSelect(
                "minecraft:stone_pickaxe",
                1,
                1,
                SETUP_TIMEOUT
            );
            ScenarioHeldItem lapis = client.giveAndSelect(
                "minecraft:lapis_lazuli",
                1,
                2,
                SETUP_TIMEOUT
            );
            if (!pickaxe.matches("minecraft:stone_pickaxe", 1)
                || !lapis.matches("minecraft:lapis_lazuli", 1)) {
                observations.add("blocked: targeted enchanting inventory setup did not converge");
                return new ClientScenarioReport("blocked", id, observations);
            }

            client.sendCommand("debug survival xp 7");
            boolean xpPrepared = client.waitForExperience(7, 1, SETUP_TIMEOUT);
            observations.add("enchanting XP setup: " + (xpPrepared ? "passed" : "failed"));
            if (!xpPrepared) {
                return new ClientScenarioReport("failed", id, observations);
            }

            ScenarioUseResult openUse = client.useItemOn(pair.target(), lapis);
            boolean opened = client.waitForScreenClassName(ENCHANTMENT_SCREEN, CONTAINER_TIMEOUT);
            observations.add(
                "enchanting screen open: " + (opened ? "passed" : "failed")
                    + " use_result=" + openUse.result()
            );
            if (!opened) {
                return new ClientScenarioReport("failed", id, observations);
            }

            boolean toolMoved = client.moveSelectedItemToContainerSlot(
                0,
                "minecraft:stone_pickaxe",
                1,
                CONTAINER_TIMEOUT
            );
            boolean lapisMoved = client.moveSelectedItemToContainerSlot(
                1,
                "minecraft:lapis_lazuli",
                1,
                CONTAINER_TIMEOUT
            );
            boolean buttonConfirmed = toolMoved
                && lapisMoved
                && client.clickContainerButton(0, CONTAINER_TIMEOUT);
            boolean enchanted = buttonConfirmed
                && client.containerSlotHasEnchantment(0, "minecraft:efficiency", 1);
            int totalExperience = client.totalExperience();
            int experienceLevel = client.experienceLevel();
            boolean xpSpent = totalExperience == 7 && experienceLevel == 0;
            observations.add(
                "efficiency enchantment: " + (enchanted && xpSpent ? "passed" : "failed")
                    + " tool_moved=" + toolMoved
                    + " lapis_moved=" + lapisMoved
                    + " button_confirmed=" + buttonConfirmed
                    + " component_matched=" + enchanted
                    + " total_xp_preserved=" + (totalExperience == 7)
                    + " level_spent=" + (experienceLevel == 0)
            );

            boolean returned = enchanted && client.moveContainerSlotToInventory(
                0,
                "minecraft:stone_pickaxe",
                1,
                CONTAINER_TIMEOUT
            );
            boolean closed = client.closeCurrentScreen(CONTAINER_TIMEOUT);
            observations.add("enchanted tool return: " + (returned && closed ? "passed" : "failed"));
            observations.add("targeted setup: debug give and XP commands; natural acquisition is not covered");
            observations.add("screenshots directory available to driver: " + screenshotsDir);

            return new ClientScenarioReport(
                enchanted && xpSpent && returned && closed ? "passed" : "failed",
                id,
                observations
            );
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }
}
