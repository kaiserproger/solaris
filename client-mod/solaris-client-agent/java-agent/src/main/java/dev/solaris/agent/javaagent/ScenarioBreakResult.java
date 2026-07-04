package dev.solaris.agent.javaagent;

record ScenarioBreakResult(
    boolean started,
    boolean becameAir,
    boolean sawDrop,
    boolean pickupRestored,
    ScenarioHeldItem selectedItem
) {
    boolean passed() {
        return started && becameAir && pickupRestored;
    }
}
