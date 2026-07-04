package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;

import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

final class M94WaterBucketScenario {
    static final String ID = "m94-02c-water-bucket-place-pickup";
    private static final Duration SETUP_TIMEOUT = Duration.ofSeconds(8);
    private static final Duration FLUID_TIMEOUT = Duration.ofSeconds(3);

    ClientScenarioReport run(String id, Path screenshotsDir, ScenarioClient client) {
        if (!ID.equals(id)) {
            return new ClientScenarioReport("blocked", id, List.of("unsupported scenario id: " + id));
        }

        List<String> observations = new ArrayList<>();
        try {
            ScenarioBlockPair pair = client.findDryPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
            if (pair == null) {
                observations.add("blocked: no loaded placeable fluid target found within survival reach");
                return new ClientScenarioReport("blocked", id, observations);
            }
            observations.add(
                "water target: clicked=" + pair.clicked().blockId()
                    + " at " + coordinates(pair.clicked())
                    + ", target=" + pair.target().blockId()
                    + " at " + coordinates(pair.target())
            );

            ScenarioHeldItem waterBucket = client.giveAndSelect("minecraft:water_bucket", 1, 0, SETUP_TIMEOUT);
            if (!waterBucket.matches("minecraft:water_bucket", 1)) {
                observations.add(
                    "blocked: expected held water bucket, saw "
                        + waterBucket.itemId() + " x" + waterBucket.count()
                );
                return new ClientScenarioReport("blocked", id, observations);
            }
            observations.add("held setup: minecraft:water_bucket x1 in hotbar slot 0");

            ScenarioUseResult placeUse = client.useItemOn(pair.clicked(), waterBucket);
            boolean waterVisible = client.waitForBlock(pair.target(), "minecraft:water", FLUID_TIMEOUT);
            ScenarioHeldItem afterPlace = client.selectedItem();
            boolean emptied = afterPlace.matches("minecraft:bucket", 1);
            observations.add(
                "water placement: " + (waterVisible && emptied ? "passed" : "failed")
                    + " use_result=" + placeUse.result()
                    + " target_is_water=" + waterVisible
                    + " held=" + afterPlace.itemId() + " x" + afterPlace.count()
            );

            ScenarioUseResult pickupUse = client.useItemOn(pair.target(), afterPlace);
            boolean sourceCleared = client.waitForNoFluid(pair.target(), FLUID_TIMEOUT);
            ScenarioHeldItem afterPickup = client.selectedItem();
            boolean refilled = afterPickup.matches("minecraft:water_bucket", 1);
            observations.add(
                "water pickup: " + (sourceCleared && refilled ? "passed" : "failed")
                    + " use_result=" + pickupUse.result()
                    + " source_cleared=" + sourceCleared
                    + " held=" + afterPickup.itemId() + " x" + afterPickup.count()
            );
            observations.add(
                "degraded: lava, broad fluid spread, water-lava interaction, and swim feel are not exercised by "
                    + ID
            );
            observations.add("screenshots directory available to driver: " + screenshotsDir);

            boolean passed = waterVisible && emptied && sourceCleared && refilled;
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
