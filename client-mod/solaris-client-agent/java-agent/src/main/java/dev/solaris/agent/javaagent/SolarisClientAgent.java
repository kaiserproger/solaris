package dev.solaris.agent.javaagent;

import dev.solaris.agent.bridge.AgentHttpBridge;
import dev.solaris.agent.client.ClientCommands;

import java.lang.instrument.Instrumentation;
import java.util.Properties;

public final class SolarisClientAgent {
    private static AgentHttpBridge bridge;

    private SolarisClientAgent() {
    }

    public static void premain(String agentArgs, Instrumentation instrumentation) {
        start(agentArgs);
    }

    public static void agentmain(String agentArgs, Instrumentation instrumentation) {
        start(agentArgs);
    }

    static synchronized boolean start(String agentArgs) {
        AgentConfig config = AgentConfig.from(System.getProperties(), agentArgs);
        if (!config.enabled()) {
            return false;
        }
        if (bridge != null) {
            return true;
        }
        try {
            bridge = AgentHttpBridge.start(
                config.secret(),
                config.port(),
                ClientCommands.create(new MinecraftClientExecutor(), new MinecraftClientFacade())
            );
            return true;
        } catch (Exception error) {
            throw new IllegalStateException("failed to start Solaris client-agent bridge", error);
        }
    }

    static synchronized void stopForTest() {
        if (bridge != null) {
            bridge.close();
            bridge = null;
        }
    }

    static AgentConfig configForTest(Properties properties, String agentArgs) {
        return AgentConfig.from(properties, agentArgs);
    }
}
