package dev.solaris.agent.bridge;

import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Optional;

public final class CommandRegistry {
    private final Map<String, BridgeCommand> commands = new LinkedHashMap<>();
    private final Object executionLock = new Object();

    public void register(String name, BridgeCommand command) {
        commands.put(name, request -> {
            synchronized (executionLock) {
                return command.execute(request);
            }
        });
    }

    public void registerConcurrent(String name, BridgeCommand command) {
        commands.put(name, command);
    }

    public Optional<BridgeCommand> find(String name) {
        return Optional.ofNullable(commands.get(name));
    }
}
