package dev.solaris.agent.javaagent;

import java.util.List;

@FunctionalInterface
interface EarnedBuildingPreparation {
    EarnedBuildingMaterials prepare(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception;
}
