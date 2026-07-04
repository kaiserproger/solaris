package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;

import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

final class M94SignsBedsCampfiresScenario {
    static final String ID = "m94-04-signs-beds-campfires-and-block-entities";

    ClientScenarioReport run(String id, Path screenshotsDir, ScenarioClient client) {
        if (!ID.equals(id)) {
            return new ClientScenarioReport("blocked", id, List.of("unsupported scenario id: " + id));
        }

        List<String> observations = new ArrayList<>();
        ClientScenarioReport sign = new M94SignScenario().run(
            M94SignScenario.ID,
            screenshotsDir,
            client
        );
        appendSubprobe("sign", sign, observations);
        if ("failed".equals(sign.result())) {
            return new ClientScenarioReport("failed", id, observations);
        }
        if ("blocked".equals(sign.result())) {
            return new ClientScenarioReport("blocked", id, observations);
        }

        observations.add(
            "blocked: beds, campfires, restart persistence, hanging signs, waxed signs, "
                + "styled/filtered/clickable text, sounds/statistics/events, and after-close "
                + "visual assertions need dedicated in-client primitives before " + ID + " can be green"
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
