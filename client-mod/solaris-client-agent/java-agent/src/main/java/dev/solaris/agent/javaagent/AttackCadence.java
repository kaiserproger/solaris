package dev.solaris.agent.javaagent;

final class AttackCadence {
    private AttackCadence() {
    }

    static boolean ready(float attackStrength) {
        return attackStrength >= 0.90F;
    }
}
