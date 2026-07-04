package dev.solaris.agent.client;

public record ClientSnapshot(
    boolean inPlay,
    String dimension,
    double x,
    double y,
    double z,
    int selectedHotbarSlot,
    String currentScreen,
    String disconnectReason
) {
}
