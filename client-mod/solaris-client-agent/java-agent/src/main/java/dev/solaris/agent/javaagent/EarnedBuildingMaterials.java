package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;

record EarnedBuildingMaterials(
    ClientScenarioReport report,
    String planksItemId,
    ScenarioBlockTarget tableTarget
) {
}
