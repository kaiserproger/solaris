package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;

import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

final class M94M40M41RouteScenario {
    static final String ID = "m94-07-m40-m41-route-with-metrics";
    private static final Duration ENTITY_TIMEOUT = Duration.ofSeconds(8);
    private static final double MAX_SUMMONED_ENTITY_DISTANCE_SQUARED = 256.0;

    ClientScenarioReport run(String id, Path screenshotsDir, ScenarioClient client) {
        if (!ID.equals(id)) {
            return new ClientScenarioReport("blocked", id, List.of("unsupported scenario id: " + id));
        }

        List<String> observations = new ArrayList<>();
        ClientScenarioReport water = new M94WaterBucketScenario().run(
            M94WaterBucketScenario.ID,
            screenshotsDir,
            client
        );
        appendSubprobe("water", water, observations);
        if ("failed".equals(water.result())) {
            return new ClientScenarioReport("failed", id, observations);
        }
        if ("blocked".equals(water.result())) {
            return new ClientScenarioReport("blocked", id, observations);
        }

        ClientScenarioReport solid = new M94SolidBlockScenario().run(
            M94SolidBlockScenario.ID,
            screenshotsDir,
            client
        );
        appendSubprobe("solid/drop", solid, observations);
        if ("failed".equals(solid.result())) {
            return new ClientScenarioReport("failed", id, observations);
        }
        if ("blocked".equals(solid.result())) {
            return new ClientScenarioReport("blocked", id, observations);
        }

        try {
            ScenarioEntityObservation cow = client.summonEntityNearPlayer(
                "minecraft:cow",
                0.0,
                0.0,
                4.0,
                ENTITY_TIMEOUT
            );
            boolean entityVisible = cow != null
                && cow.distanceSquared() <= MAX_SUMMONED_ENTITY_DISTANCE_SQUARED;
            observations.add(
                "visible entity: " + (entityVisible ? "passed" : "failed")
                    + " type=minecraft:cow"
                    + (entityVisible
                        ? " entity_id=" + cow.entityId()
                            + " position=" + coordinates(cow)
                            + " distance_squared=" + cow.distanceSquared()
                        : "")
            );
            observations.add(
                "blocked: swim feel, sugar cane support/cascade/drop, TPS/lock log analysis, "
                    + "owner M40/M41 frozen-world route, and broad performance evidence need "
                    + "dedicated real-client/manual gates before " + ID + " can be green"
            );
            observations.add("screenshots directory available to driver: " + screenshotsDir);

            return new ClientScenarioReport(entityVisible ? "blocked" : "failed", id, observations);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private static void appendSubprobe(
        String label,
        ClientScenarioReport subprobe,
        List<String> observations
    ) {
        observations.add(label + " subprobe result: " + subprobe.result());
        for (String observation : subprobe.observations()) {
            observations.add(label + " subprobe: " + observation);
        }
    }

    private static String coordinates(ScenarioEntityObservation entity) {
        return entity.x() + "," + entity.y() + "," + entity.z();
    }
}
