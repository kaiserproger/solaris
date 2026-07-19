package dev.solaris.agent.mcp;

import com.google.gson.JsonObject;

import java.util.Objects;

public record McpToolDefinition(
    String name,
    String description,
    String command,
    JsonObject inputSchema,
    boolean readOnly,
    boolean destructive,
    boolean idempotent,
    boolean openWorld,
    Execution execution
) {
    enum Execution {
        DIRECT,
        WAIT_FOR_BLOCK_STATE
    }

    public McpToolDefinition(
        String name,
        String description,
        String command,
        JsonObject inputSchema,
        boolean readOnly,
        boolean destructive,
        boolean idempotent,
        boolean openWorld
    ) {
        this(
            name,
            description,
            command,
            inputSchema,
            readOnly,
            destructive,
            idempotent,
            openWorld,
            Execution.DIRECT
        );
    }

    public McpToolDefinition(
        String name,
        String description,
        String command,
        JsonObject inputSchema,
        boolean readOnly
    ) {
        this(
            name,
            description,
            command,
            inputSchema,
            readOnly,
            false,
            readOnly,
            true,
            Execution.DIRECT
        );
    }

    public McpToolDefinition {
        name = requireText(name, "tool name");
        description = requireText(description, "tool description");
        command = requireText(command, "tool command");
        inputSchema = Objects.requireNonNull(inputSchema, "tool input schema").deepCopy();
        execution = Objects.requireNonNull(execution, "tool execution");
    }

    JsonObject toJson() {
        JsonObject tool = new JsonObject();
        tool.addProperty("name", name);
        tool.addProperty("description", description);
        tool.add("inputSchema", inputSchema.deepCopy());
        JsonObject annotations = new JsonObject();
        annotations.addProperty("readOnlyHint", readOnly);
        annotations.addProperty("destructiveHint", destructive);
        annotations.addProperty("idempotentHint", idempotent);
        annotations.addProperty("openWorldHint", openWorld);
        tool.add("annotations", annotations);
        return tool;
    }

    private static String requireText(String value, String label) {
        if (value == null || value.isBlank()) {
            throw new IllegalArgumentException(label + " must not be blank");
        }
        return value;
    }
}
