package dev.solaris.agent.javaagent;

record ScenarioBreakResult(
    boolean started,
    boolean becameAir,
    boolean sawDrop,
    boolean pickupRestored,
    ScenarioHeldItem selectedItem,
    String pickupDetail
) {
    ScenarioBreakResult(
        boolean started,
        boolean becameAir,
        boolean sawDrop,
        boolean pickupRestored,
        ScenarioHeldItem selectedItem
    ) {
        this(started, becameAir, sawDrop, pickupRestored, selectedItem, "");
    }

    boolean passed() {
        return started && becameAir && pickupRestored;
    }
}
