package dev.solaris.agent.javaagent;

import org.junit.jupiter.api.Test;

import java.util.Map;
import java.util.Properties;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class ClientMcpConfigTest {
    @Test
    void disabledWhenTokenIsMissing() {
        ClientMcpConfig config = ClientMcpConfig.from(new Properties(), "", Map.of());

        assertFalse(config.enabled());
        assertEquals(39095, config.port());
    }

    @Test
    void agentArgumentsOverrideProperties() {
        Properties properties = new Properties();
        properties.setProperty("solaris.clientMcp.token", "from-property");
        properties.setProperty("solaris.clientMcp.port", "39095");

        ClientMcpConfig config = ClientMcpConfig.from(
            properties,
            "mcpToken=from-args,mcpPort=39096",
            Map.of()
        );

        assertTrue(config.enabled());
        assertEquals("from-args", config.token());
        assertEquals(39096, config.port());
    }

    @Test
    void readsTokenAndPortFromEnvironment() {
        ClientMcpConfig config = ClientMcpConfig.from(
            new Properties(),
            "",
            Map.of(
                "SOLARIS_CLIENT_MCP_TOKEN", "from-env",
                "SOLARIS_CLIENT_MCP_PORT", "39097"
            )
        );

        assertTrue(config.enabled());
        assertEquals("from-env", config.token());
        assertEquals(39097, config.port());
    }

    @Test
    void explicitRunEnvironmentOverridesStaleJvmProperties() {
        Properties properties = new Properties();
        properties.setProperty("solaris.clientMcp.token", "stale-token");
        properties.setProperty("solaris.clientMcp.port", "39094");

        ClientMcpConfig config = ClientMcpConfig.from(
            properties,
            "",
            Map.of(
                "SOLARIS_CLIENT_MCP_TOKEN", "current-token",
                "SOLARIS_CLIENT_MCP_PORT", "39097"
            )
        );

        assertEquals("current-token", config.token());
        assertEquals(39097, config.port());
    }

    @Test
    void rejectsInvalidPortBeforeBinding() {
        Properties properties = new Properties();
        properties.setProperty("solaris.clientMcp.token", "token");
        properties.setProperty("solaris.clientMcp.port", "65536");

        assertThrows(
            IllegalArgumentException.class,
            () -> ClientMcpConfig.from(properties, "", Map.of())
        );
    }

    @Test
    void ignoresStaleInvalidPortWhenMcpIsDisabled() {
        Properties properties = new Properties();
        properties.setProperty("solaris.clientMcp.port", "not-a-port");

        ClientMcpConfig config = ClientMcpConfig.from(properties, "", Map.of());

        assertFalse(config.enabled());
        assertEquals(39095, config.port());
    }
}
