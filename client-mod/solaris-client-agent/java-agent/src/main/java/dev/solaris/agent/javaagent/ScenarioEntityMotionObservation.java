package dev.solaris.agent.javaagent;

record ScenarioEntityMotionObservation(
    String entityTypeId,
    int entityId,
    double endX,
    double endY,
    double endZ,
    double horizontalDistance,
    double verticalRise,
    double maxHorizontalSpeed,
    double minimumYawDelta
) {
}
