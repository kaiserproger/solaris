package dev.solaris.agent.javaagent;

record ScenarioFoodUseResult(
    boolean started,
    int foodBefore,
    int foodAfter,
    int itemCountBefore,
    int itemCountAfter
) {}
