package dev.solaris.agent.javaagent;

import dev.solaris.agent.bridge.AgentHttpBridge;
import dev.solaris.agent.bridge.CommandRegistry;
import dev.solaris.agent.client.ClientCommands;
import dev.solaris.agent.mcp.ClientMcpTools;
import dev.solaris.agent.mcp.McpHttpServer;

import java.lang.instrument.Instrumentation;
import java.util.Properties;

public final class SolarisClientAgent {
    private static AgentHttpBridge bridge;
    private static McpHttpServer mcp;
    private static boolean shutdownHookInstalled;

    private SolarisClientAgent() {
    }

    public static void premain(String agentArgs, Instrumentation instrumentation) {
        start(agentArgs);
    }

    public static void agentmain(String agentArgs, Instrumentation instrumentation) {
        start(agentArgs);
    }

    public static boolean startFromRuntime() {
        return start(null);
    }

    static synchronized boolean start(String agentArgs) {
        AgentConfig bridgeConfig = AgentConfig.from(System.getProperties(), agentArgs);
        ClientMcpConfig mcpConfig = ClientMcpConfig.from(System.getProperties(), agentArgs);
        if (!bridgeConfig.enabled() && !mcpConfig.enabled()) {
            return false;
        }
        if ((!bridgeConfig.enabled() || bridge != null) && (!mcpConfig.enabled() || mcp != null)) {
            return true;
        }

        AgentHttpBridge startedBridge = null;
        McpHttpServer startedMcp = null;
        try {
            CommandRegistry commands = ClientCommands.create(
                new MinecraftClientExecutor(),
                new MinecraftClientFacade()
            );
            if (bridgeConfig.enabled() && bridge == null) {
                startedBridge = AgentHttpBridge.start(
                    bridgeConfig.secret(),
                    bridgeConfig.port(),
                    commands
                );
                bridge = startedBridge;
                System.out.println(
                    "Solaris client agent bridge listening on http://127.0.0.1:"
                        + bridge.port() + "/rpc"
                );
            }
            if (mcpConfig.enabled() && mcp == null) {
                startedMcp = McpHttpServer.start(
                    mcpConfig.token(),
                    mcpConfig.port(),
                    commands,
                    ClientMcpTools.definitions()
                );
                mcp = startedMcp;
                System.out.println(
                    "Solaris Minecraft MCP listening on http://127.0.0.1:" + mcp.port() + "/mcp"
                );
            }
            installShutdownHook();
            return true;
        } catch (Exception error) {
            if (startedMcp != null) {
                startedMcp.close();
                mcp = null;
            }
            if (startedBridge != null) {
                startedBridge.close();
                bridge = null;
            }
            throw new IllegalStateException("failed to start Solaris client-agent endpoints", error);
        }
    }

    public static synchronized void stopFromRuntime() {
        if (mcp != null) {
            mcp.close();
            mcp = null;
        }
        if (bridge != null) {
            bridge.close();
            bridge = null;
        }
    }

    static void stopForTest() {
        stopFromRuntime();
    }

    static AgentConfig configForTest(Properties properties, String agentArgs) {
        return AgentConfig.from(properties, agentArgs);
    }

    private static void installShutdownHook() {
        if (shutdownHookInstalled) {
            return;
        }
        Runtime.getRuntime().addShutdownHook(
            new Thread(SolarisClientAgent::stopFromRuntime, "solaris-client-agent-shutdown")
        );
        shutdownHookInstalled = true;
    }
}
