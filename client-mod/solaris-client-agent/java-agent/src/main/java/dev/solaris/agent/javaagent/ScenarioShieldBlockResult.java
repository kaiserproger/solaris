package dev.solaris.agent.javaagent;

record ScenarioShieldBlockResult(
    boolean useStarted,
    boolean blockedAttackObserved,
    float healthBefore,
    float healthAfter,
    int shieldDamageBefore,
    int shieldDamageAfter
) {
}
