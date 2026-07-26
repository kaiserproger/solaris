package dev.solaris.loader.forge;

import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;

final class LoaderForgeMcpRuntime {
    private static final String TOKEN_ENV = "SOLARIS_CLIENT_MCP_TOKEN";
    private final Method start;
    private final Method runPreTickActions;
    private final Method publishTick;
    private final Method publishState;
    private boolean started;

    LoaderForgeMcpRuntime(
            Method start,
            Method runPreTickActions,
            Method publishTick,
            Method publishState) {
        this.start = start;
        this.runPreTickActions = runPreTickActions;
        this.publishTick = publishTick;
        this.publishState = publishState;
    }

    static LoaderForgeMcpRuntime createIfEnabled() {
        return create(
                System.getenv(TOKEN_ENV),
                LoaderForgeMcpRuntime.class.getClassLoader());
    }

    static LoaderForgeMcpRuntime create(String token, ClassLoader loader) {
        if (token == null || token.isBlank()) {
            return null;
        }
        try {
            Class<?> agent = Class.forName(
                    "dev.solaris.agent.javaagent.SolarisClientAgent",
                    true,
                    loader);
            Class<?> scenarioClient = Class.forName(
                    "dev.solaris.agent.javaagent.MinecraftScenarioClient",
                    true,
                    loader);
            Class<?> stateEvents = Class.forName(
                    "dev.solaris.agent.javaagent.ClientStateEvents",
                    true,
                    loader);
            return new LoaderForgeMcpRuntime(
                    agent.getMethod("startFromRuntime"),
                    scenarioClient.getMethod("runPreTickActions"),
                    stateEvents.getMethod("publishTick"),
                    stateEvents.getMethod("publishState"));
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException(
                    "Forge Loader MCP runtime classes are unavailable",
                    error);
        }
    }

    void beforeTick() {
        if (started) {
            invoke(runPreTickActions);
        }
    }

    void afterTick() {
        invoke(publishTick);
        invoke(publishState);
        if (!started) {
            Object result = invoke(start);
            if (!Boolean.TRUE.equals(result)) {
                throw new IllegalStateException("Forge Loader MCP runtime did not start");
            }
            started = true;
        }
    }

    void publishState() {
        invoke(publishState);
    }

    private static Object invoke(Method method) {
        try {
            return method.invoke(null);
        } catch (IllegalAccessException error) {
            throw new IllegalStateException(
                    "Forge Loader MCP runtime method is inaccessible",
                    error);
        } catch (InvocationTargetException error) {
            Throwable cause = error.getCause();
            if (cause instanceof RuntimeException runtime) {
                throw runtime;
            }
            if (cause instanceof Error fatal) {
                throw fatal;
            }
            throw new IllegalStateException(
                    "Forge Loader MCP runtime method failed",
                    cause);
        }
    }
}
