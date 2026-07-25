package dev.solaris.agent.javaagent;

record ScenarioDeepWaterTarget(
    int x,
    int bottomY,
    int topY,
    int z,
    float yaw,
    String direction
) {
    double centerX() {
        return x + 0.5;
    }

    double centerZ() {
        return z + 0.5;
    }
}
