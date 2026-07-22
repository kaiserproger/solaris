package dev.solaris.agent.javaagent;

import java.util.HashMap;
import java.util.Map;
import java.util.Properties;

record ClientMcpConfig(boolean enabled, String token, int port) {
    private static final int DEFAULT_PORT = 39095;

    static ClientMcpConfig from(Properties properties, String agentArgs) {
        return from(properties, agentArgs, System.getenv());
    }

    static ClientMcpConfig from(
        Properties properties,
        String agentArgs,
        Map<String, String> environment
    ) {
        Map<String, String> args = parseAgentArgs(agentArgs);
        String token = firstNonBlank(
            args.get("mcpToken"),
            environment.get("SOLARIS_CLIENT_MCP_TOKEN"),
            properties.getProperty("solaris.clientMcp.token")
        );
        if (token == null) {
            return new ClientMcpConfig(false, "", DEFAULT_PORT);
        }
        String configuredPort = firstNonBlank(
            args.get("mcpPort"),
            environment.get("SOLARIS_CLIENT_MCP_PORT"),
            properties.getProperty("solaris.clientMcp.port"),
            Integer.toString(DEFAULT_PORT)
        );
        final int port;
        try {
            port = Integer.parseInt(configuredPort);
        } catch (NumberFormatException error) {
            throw new IllegalArgumentException("MCP port must be an integer: " + configuredPort, error);
        }
        if (port < 1 || port > 65_535) {
            throw new IllegalArgumentException("MCP port must be between 1 and 65535: " + port);
        }
        return new ClientMcpConfig(true, token, port);
    }

    private static Map<String, String> parseAgentArgs(String agentArgs) {
        Map<String, String> values = new HashMap<>();
        if (agentArgs == null || agentArgs.isBlank()) {
            return values;
        }
        for (String entry : agentArgs.split(",")) {
            int separator = entry.indexOf('=');
            if (separator <= 0) {
                continue;
            }
            values.put(entry.substring(0, separator).trim(), entry.substring(separator + 1).trim());
        }
        return values;
    }

    private static String firstNonBlank(String... values) {
        for (String value : values) {
            if (value != null && !value.isBlank()) {
                return value;
            }
        }
        return null;
    }
}
