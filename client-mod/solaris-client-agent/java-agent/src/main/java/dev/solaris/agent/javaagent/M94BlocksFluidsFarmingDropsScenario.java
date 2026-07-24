package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;

import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

final class M94BlocksFluidsFarmingDropsScenario {
    static final String ID = "m94-02-blocks-fluids-farming-drops";

    static final String SOLID_PHASE_ID = M94SolidBlockScenario.ID;
    static final String WATER_PHASE_ID = M94WaterBucketScenario.ID;

    ClientScenarioReport run(String id, Path screenshotsDir, ScenarioClient client) {
        if (SOLID_PHASE_ID.equals(id)) {
            return new M94SolidBlockScenario().run(id, screenshotsDir, client);
        }

        if (WATER_PHASE_ID.equals(id)) {
            return new M94WaterBucketScenario().run(id, screenshotsDir, client);
        }

        if (!ID.equals(id)) {
            return new ClientScenarioReport("blocked", id, List.of("unsupported scenario id: " + id));
        }

        List<String> observations = new ArrayList<>();
        ClientScenarioReport solid = new M94SolidBlockScenario().run(
            M94SolidBlockScenario.ID,
            screenshotsDir,
            client
        );
        appendSubprobe("solid", solid, observations);
        if ("failed".equals(solid.result())) {
            return new ClientScenarioReport("failed", id, observations);
        }
        if ("blocked".equals(solid.result())) {
            return new ClientScenarioReport("blocked", id, observations);
        }

        ClientScenarioReport water = new M94WaterBucketScenario().run(
            M94WaterBucketScenario.ID,
            screenshotsDir,
            client
        );
        appendSubprobe("water", water, observations);
        if ("failed".equals(water.result())) {
            return new ClientScenarioReport("failed", id, observations);
        }

        observations.add(
            "blocked: door/trapdoor, crop/bonemeal, sugar cane support/cascade/drop, "
                + "broad fluid spread, water-lava interaction, and swim feel need dedicated "
                + "in-client primitives before " + ID + " can be green"
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);

        return new ClientScenarioReport("blocked", id, observations);
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
}
