package dev.solaris.agent.javaagent;

record ScenarioHeldItem(String itemId, int count) {
    boolean matches(String expectedItemId, int minimumCount) {
        return itemId.equals(expectedItemId) && count >= minimumCount;
    }
}
