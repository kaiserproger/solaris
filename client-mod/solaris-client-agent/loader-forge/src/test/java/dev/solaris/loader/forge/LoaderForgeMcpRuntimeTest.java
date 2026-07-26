package dev.solaris.loader.forge;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;

import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

final class LoaderForgeMcpRuntimeTest {
    @BeforeEach
    void resetHooks() {
        Hooks.starts = 0;
        Hooks.preTicks = 0;
        Hooks.ticks = 0;
        Hooks.states = 0;
    }

    @Test
    void disabledRuntimeDoesNotResolveAgentClasses() {
        ClassLoader missingAgentClasses = new ClassLoader(null) {};

        assertNull(LoaderForgeMcpRuntime.create(null, missingAgentClasses));
        assertNull(LoaderForgeMcpRuntime.create("  ", missingAgentClasses));
    }

    @Test
    void startsOnceThenRunsPreTickAndPublishesTickState() throws Exception {
        LoaderForgeMcpRuntime runtime = new LoaderForgeMcpRuntime(
                Hooks.class.getMethod("start"),
                Hooks.class.getMethod("preTick"),
                Hooks.class.getMethod("tick"),
                Hooks.class.getMethod("state"));

        runtime.beforeTick();
        runtime.afterTick();
        runtime.afterTick();
        runtime.beforeTick();
        runtime.publishState();

        assertEquals(1, Hooks.starts);
        assertEquals(1, Hooks.preTicks);
        assertEquals(2, Hooks.ticks);
        assertEquals(3, Hooks.states);
    }

    @Test
    void reportsMissingRuntimeClasses() {
        ClassLoader missingAgentClasses = new ClassLoader(null) {};

        IllegalStateException error = assertThrows(
                IllegalStateException.class,
                () -> LoaderForgeMcpRuntime.create("token", missingAgentClasses));

        assertEquals(
                "Forge Loader MCP runtime classes are unavailable",
                error.getMessage());
    }

    public static final class Hooks {
        private static int starts;
        private static int preTicks;
        private static int ticks;
        private static int states;

        public static boolean start() {
            starts += 1;
            return true;
        }

        public static void preTick() {
            preTicks += 1;
        }

        public static void tick() {
            ticks += 1;
        }

        public static void state() {
            states += 1;
        }
    }
}
