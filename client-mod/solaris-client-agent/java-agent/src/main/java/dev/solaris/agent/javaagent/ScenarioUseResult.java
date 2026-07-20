package dev.solaris.agent.javaagent;

record ScenarioUseResult(String result, long blockChangeAckVersionBeforeUse) {
    ScenarioUseResult(String result) {
        this(result, -1L);
    }
}
