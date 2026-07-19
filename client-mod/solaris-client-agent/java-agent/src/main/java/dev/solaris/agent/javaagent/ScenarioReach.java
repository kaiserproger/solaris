package dev.solaris.agent.javaagent;

enum ScenarioReach {
    WITHIN_SURVIVAL_REACH("within-survival-reach"),
    OUTSIDE_SURVIVAL_REACH("outside-survival-reach");

    private static final double SURVIVAL_REACH_SQUARED = 20.25;
    private final String label;

    ScenarioReach(String label) {
        this.label = label;
    }

    String label() {
        return label;
    }

    boolean includes(double distanceSquared) {
        return this == WITHIN_SURVIVAL_REACH
            ? distanceSquared <= SURVIVAL_REACH_SQUARED
            : distanceSquared > SURVIVAL_REACH_SQUARED;
    }
}
