package dev.solaris.agent.javaagent;

record ScenarioEntityObservation(
    String entityTypeId,
    int entityId,
    double x,
    double y,
    double z,
    double distanceSquared
) {
}
