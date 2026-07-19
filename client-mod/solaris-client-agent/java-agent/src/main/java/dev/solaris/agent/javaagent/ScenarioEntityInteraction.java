package dev.solaris.agent.javaagent;

import java.util.Objects;

record ScenarioEntityInteraction(ScenarioEntityIdentity identity, String hand) {
    ScenarioEntityInteraction {
        Objects.requireNonNull(identity, "entity identity");
        if (!"main_hand".equals(hand) && !"off_hand".equals(hand)) {
            throw new IllegalArgumentException("interaction hand must be main_hand or off_hand");
        }
    }
}
