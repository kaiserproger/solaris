package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;

import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

final class M94SolidBlockScenario {
    static final String ID = "m94-02a-solid-place-break-drop";
    private static final Duration SETUP_TIMEOUT = Duration.ofSeconds(8);
    private static final Duration BLOCK_TIMEOUT = Duration.ofSeconds(2);
    private static final Duration BREAK_TIMEOUT = Duration.ofSeconds(6);

    ClientScenarioReport run(String id, Path screenshotsDir, ScenarioClient client) {
        if (!ID.equals(id)) {
            return new ClientScenarioReport("blocked", id, List.of("unsupported scenario id: " + id));
        }

        List<String> observations = new ArrayList<>();
        try {
            ScenarioBlockPair pair = client.findPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
            if (pair == null) {
                observations.add("blocked: no loaded placeable target found within survival reach");
                return new ClientScenarioReport("blocked", id, observations);
            }
            observations.add(
                "break/place target: clicked=" + pair.clicked().blockId()
                    + " at " + coordinates(pair.clicked())
                    + ", target=" + pair.target().blockId()
                    + " at " + coordinates(pair.target())
            );

            ScenarioHeldItem cleared = client.giveAndSelect("minecraft:dirt", 0, 0, SETUP_TIMEOUT);
            if (!cleared.matches("minecraft:air", 0)) {
                observations.add(
                    "blocked: expected cleared selected slot, saw "
                        + cleared.itemId() + " x" + cleared.count()
                );
                return new ClientScenarioReport("blocked", id, observations);
            }
            observations.add("held setup: cleared hotbar slot 0 before survival break");

            ScenarioBreakResult broke = client.breakBlock(pair.clicked(), "minecraft:dirt", 1, BREAK_TIMEOUT);
            observations.add(
                "break/drop/pickup: " + (broke.passed() ? "passed" : "failed")
                    + " started=" + broke.started()
                    + " became_air=" + broke.becameAir()
                    + " saw_drop=" + broke.sawDrop()
                    + " pickup_restored=" + broke.pickupRestored()
                    + " held=" + broke.selectedItem().itemId() + " x" + broke.selectedItem().count()
            );
            if (!broke.sawDrop()) {
                observations.add(
                    "degraded: visible item-entity window was not observed before pickup; "
                        + "pickup convergence is still recorded"
                );
            }

            ScenarioBlockPair placePair = client.findPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
            if (placePair == null) {
                observations.add("blocked: no loaded placeable target found after pickup");
                return new ClientScenarioReport("blocked", id, observations);
            }
            ScenarioUseResult use = client.useItemOn(placePair.clicked(), broke.selectedItem());
            boolean placed = client.waitForBlock(placePair.target(), "minecraft:dirt", BLOCK_TIMEOUT);
            ScenarioHeldItem afterPlace = client.selectedItem();
            boolean decremented = afterPlace.matches("minecraft:air", 0);
            observations.add(
                "solid placement: " + (placed && decremented ? "passed" : "failed")
                    + " use_result=" + use.result()
                    + " target_is_dirt=" + placed
                    + " held=" + afterPlace.itemId() + " x" + afterPlace.count()
            );
            observations.add(
                "degraded: door/trapdoor, crop/bonemeal, sugar cane, and broad fluid paths "
                    + "are not exercised by " + ID
            );
            observations.add("screenshots directory available to driver: " + screenshotsDir);

            boolean passed = broke.passed() && placed && decremented;
            return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private static String coordinates(ScenarioBlockTarget target) {
        return target.x() + "," + target.y() + "," + target.z() + "/" + target.face();
    }
}
