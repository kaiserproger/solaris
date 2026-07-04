package dev.solaris.agent.javaagent;

enum ScenarioReach {
    WITHIN_SURVIVAL_REACH("within-survival-reach"),
    OUTSIDE_SURVIVAL_REACH("outside-survival-reach");

    private final String label;

    ScenarioReach(String label) {
        this.label = label;
    }

    String label() {
        return label;
    }
}
