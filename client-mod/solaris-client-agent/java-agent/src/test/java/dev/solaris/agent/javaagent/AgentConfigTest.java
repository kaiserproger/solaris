package dev.solaris.agent.javaagent;

import org.junit.jupiter.api.Test;

import java.nio.file.Path;
import java.util.Map;
import java.util.Properties;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class AgentConfigTest {
    @Test
    void disabledWhenSecretIsMissing() {
        AgentConfig config = AgentConfig.from(new Properties(), "");

        assertFalse(config.enabled());
    }

    @Test
    void parsesPropertiesAndAgentArgs() {
        Properties properties = new Properties();
        properties.setProperty("solaris.clientAgent.secret", "from-property");
        properties.setProperty("solaris.clientAgent.port", "39094");
        properties.setProperty("solaris.clientAgent.runDir", "property-run");

        AgentConfig config = AgentConfig.from(properties, "secret=from-args,port=39095,runDir=arg-run");

        assertTrue(config.enabled());
        assertEquals("from-args", config.secret());
        assertEquals(39095, config.port());
        assertEquals(Path.of("arg-run"), config.runDirectory());
    }

    @Test
    void readsSecretFromEnvironment() {
        AgentConfig config = AgentConfig.from(
            new Properties(),
            "port=39096,runDir=arg-run",
            Map.of("SOLARIS_CLIENT_AGENT_SECRET", "from-env")
        );

        assertTrue(config.enabled());
        assertEquals("from-env", config.secret());
        assertEquals(39096, config.port());
        assertEquals(Path.of("arg-run"), config.runDirectory());
    }
}
