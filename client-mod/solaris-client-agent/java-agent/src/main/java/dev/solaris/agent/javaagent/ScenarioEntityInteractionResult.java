package dev.solaris.agent.javaagent;

import java.util.Objects;

record ScenarioEntityInteractionResult(
    String result,
    boolean consumesAction,
    double hitX,
    double hitY,
    double hitZ
) {
    ScenarioEntityInteractionResult {
        Objects.requireNonNull(result, "interaction result");
        if (result.isBlank()) {
            throw new IllegalArgumentException("interaction result must not be blank");
        }
        if (!Double.isFinite(hitX) || !Double.isFinite(hitY) || !Double.isFinite(hitZ)) {
            throw new IllegalArgumentException("interaction hit location must be finite");
        }
    }
}
