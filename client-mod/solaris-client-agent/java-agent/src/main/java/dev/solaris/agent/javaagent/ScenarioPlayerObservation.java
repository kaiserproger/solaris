package dev.solaris.agent.javaagent;

record ScenarioPlayerObservation(
    String playerName,
    int entityId,
    double x,
    double y,
    double z,
    double distanceSquared
) {
}
