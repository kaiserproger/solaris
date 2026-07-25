package dev.solaris.agent.javaagent;

record ScenarioWaterObservation(
    double x,
    double y,
    double z,
    double eyeY,
    double eyeHeight,
    double bodyHeight,
    boolean inWater,
    boolean underWater,
    boolean swimming,
    double waterFluidHeight,
    String feetBlockId,
    String feetFluidId,
    boolean feetFluidSource,
    double feetCellFluidHeight,
    String eyeBlockId,
    String eyeFluidId,
    boolean eyeFluidSource,
    double eyeCellFluidHeight,
    int air,
    int maxAir,
    float health,
    String pose,
    boolean connected
) {
    double horizontalDistance(ScenarioWaterObservation other) {
        double dx = x - other.x;
        double dz = z - other.z;
        return Math.sqrt(dx * dx + dz * dz);
    }
}
