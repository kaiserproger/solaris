package dev.solaris.agent.javaagent;

import java.nio.file.Path;
import java.util.HashMap;
import java.util.Map;
import java.util.Properties;

record AgentConfig(boolean enabled, String secret, int port, Path runDirectory) {
    private static final int DEFAULT_PORT = 39094;

    static AgentConfig from(Properties properties, String agentArgs) {
        return from(properties, agentArgs, System.getenv());
    }

    static AgentConfig from(Properties properties, String agentArgs, Map<String, String> environment) {
        Map<String, String> args = parseAgentArgs(agentArgs);
        String secret = firstNonBlank(
            args.get("secret"),
            properties.getProperty("solaris.clientAgent.secret"),
            environment.get("SOLARIS_CLIENT_AGENT_SECRET")
        );
        int port = Integer.parseInt(firstNonBlank(
            args.get("port"),
            properties.getProperty("solaris.clientAgent.port"),
            environment.get("SOLARIS_CLIENT_AGENT_PORT"),
            Integer.toString(DEFAULT_PORT)
        ));
        Path runDirectory = Path.of(firstNonBlank(
            args.get("runDir"),
            properties.getProperty("solaris.clientAgent.runDir"),
            environment.get("SOLARIS_CLIENT_AGENT_RUN_DIR"),
            "."
        ));
        return new AgentConfig(secret != null, secret == null ? "" : secret, port, runDirectory);
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
