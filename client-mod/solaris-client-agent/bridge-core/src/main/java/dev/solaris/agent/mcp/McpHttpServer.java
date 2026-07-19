package dev.solaris.agent.mcp;

import com.google.gson.Gson;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParseException;
import com.google.gson.JsonParser;
import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;
import dev.solaris.agent.bridge.BridgeCommand;
import dev.solaris.agent.bridge.BridgeRequest;
import dev.solaris.agent.bridge.CommandRegistry;

import java.io.IOException;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.time.Duration;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.atomic.AtomicLong;

public final class McpHttpServer implements AutoCloseable {
    public static final String CURRENT_PROTOCOL_VERSION = "2025-11-25";
    static final int MAX_SESSIONS = 64;
    private static final Set<String> SUPPORTED_PROTOCOL_VERSIONS = Set.of(
        CURRENT_PROTOCOL_VERSION,
        "2025-06-18"
    );
    private static final int MAX_REQUEST_BYTES = 1_048_576;
    private static final long SESSION_IDLE_NANOS = Duration.ofMinutes(30).toNanos();
    private static final Duration EXECUTOR_CLOSE_TIMEOUT = Duration.ofSeconds(5);
    private static final Gson GSON = new Gson();

    private final HttpServer server;
    private final ExecutorService executor;
    private final String authorization;
    private final CommandRegistry commands;
    private final Map<String, McpToolDefinition> tools;
    private final Duration executorCloseTimeout;
    private final Map<String, Session> sessions = new ConcurrentHashMap<>();
    private final AtomicLong requestSequence = new AtomicLong();

    private McpHttpServer(
        HttpServer server,
        ExecutorService executor,
        String token,
        CommandRegistry commands,
        List<McpToolDefinition> definitions,
        Duration executorCloseTimeout
    ) {
        this.server = server;
        this.executor = executor;
        this.authorization = "Bearer " + token;
        this.commands = commands;
        this.tools = indexTools(definitions);
        this.executorCloseTimeout = executorCloseTimeout;
    }

    public static McpHttpServer start(
        String token,
        int port,
        CommandRegistry commands,
        List<McpToolDefinition> tools
    ) throws IOException {
        return start(token, port, commands, tools, EXECUTOR_CLOSE_TIMEOUT);
    }

    static McpHttpServer start(
        String token,
        int port,
        CommandRegistry commands,
        List<McpToolDefinition> tools,
        Duration executorCloseTimeout
    ) throws IOException {
        if (token == null || token.isBlank()) {
            throw new IllegalArgumentException("MCP bearer token must not be blank");
        }
        if (port < 0 || port > 65_535) {
            throw new IllegalArgumentException("MCP port must be 0..65535");
        }
        if (executorCloseTimeout.isNegative()) {
            throw new IllegalArgumentException("executor close timeout must not be negative");
        }
        HttpServer server = HttpServer.create(
            new InetSocketAddress(InetAddress.getByName("127.0.0.1"), port),
            0
        );
        ExecutorService executor = Executors.newVirtualThreadPerTaskExecutor();
        McpHttpServer mcp = new McpHttpServer(
            server,
            executor,
            token,
            commands,
            List.copyOf(tools),
            executorCloseTimeout
        );
        server.setExecutor(executor);
        server.createContext("/mcp", mcp::handle);
        server.start();
        return mcp;
    }

    public int port() {
        return server.getAddress().getPort();
    }

    public String hostAddress() {
        return server.getAddress().getAddress().getHostAddress();
    }

    private void handle(HttpExchange exchange) throws IOException {
        try {
            if (!validHost(exchange)) {
                writeError(exchange, 400, null, -32_600, "invalid Host header");
                return;
            }
            if (!validOrigin(exchange)) {
                writeError(exchange, 403, null, -32_600, "Origin must target loopback");
                return;
            }
            if (!authorized(exchange)) {
                exchange.getResponseHeaders().set("WWW-Authenticate", "Bearer realm=\"solaris-minecraft-client\"");
                writeError(exchange, 401, null, -32_600, "missing or invalid bearer token");
                return;
            }
            switch (exchange.getRequestMethod()) {
                case "POST" -> handlePost(exchange);
                case "DELETE" -> handleDelete(exchange);
                case "GET" -> {
                    exchange.getResponseHeaders().set("Allow", "POST, DELETE");
                    writeEmpty(exchange, 405);
                }
                default -> {
                    exchange.getResponseHeaders().set("Allow", "POST, GET, DELETE");
                    writeEmpty(exchange, 405);
                }
            }
        } catch (RuntimeException error) {
            writeError(exchange, 500, null, -32_603, safeMessage(error));
        }
    }

    private void handlePost(HttpExchange exchange) throws IOException {
        byte[] bytes = exchange.getRequestBody().readNBytes(MAX_REQUEST_BYTES + 1);
        if (bytes.length > MAX_REQUEST_BYTES) {
            writeError(exchange, 413, null, -32_600, "MCP request exceeds 1048576 bytes");
            return;
        }

        final JsonObject request;
        try {
            JsonElement parsed = JsonParser.parseString(new String(bytes, StandardCharsets.UTF_8));
            if (!parsed.isJsonObject()) {
                writeError(exchange, 400, null, -32_600, "JSON-RPC request must be an object");
                return;
            }
            request = parsed.getAsJsonObject();
        } catch (JsonParseException error) {
            writeError(exchange, 400, null, -32_700, "invalid JSON");
            return;
        }

        JsonElement id = request.has("id") ? request.get("id").deepCopy() : null;
        if (!request.has("jsonrpc")
            || !request.get("jsonrpc").isJsonPrimitive()
            || !request.get("jsonrpc").getAsJsonPrimitive().isString()
            || !"2.0".equals(request.get("jsonrpc").getAsString())) {
            writeError(exchange, 400, id, -32_600, "jsonrpc must be 2.0");
            return;
        }
        if (!request.has("method")
            || !request.get("method").isJsonPrimitive()
            || !request.get("method").getAsJsonPrimitive().isString()) {
            writeError(exchange, 400, id, -32_600, "method must be a string");
            return;
        }
        String method = request.get("method").getAsString();
        final JsonObject params;
        try {
            params = objectOrEmpty(request.get("params"));
        } catch (IllegalArgumentException error) {
            writeError(exchange, 200, id, -32_602, error.getMessage());
            return;
        }

        if ("initialize".equals(method)) {
            initialize(exchange, id, params);
            return;
        }

        Session session = requireSession(exchange, id);
        if (session == null) {
            return;
        }
        if (!validProtocolVersion(exchange, session)) {
            writeError(exchange, 400, id, -32_600, "unsupported MCP-Protocol-Version");
            return;
        }

        if (id == null) {
            if ("notifications/initialized".equals(method)) {
                session.initialized = true;
            }
            writeEmpty(exchange, 202);
            return;
        }

        switch (method) {
            case "ping" -> writeResult(exchange, id, new JsonObject());
            case "tools/list" -> listTools(exchange, id);
            case "tools/call" -> callTool(exchange, id, params);
            default -> writeError(exchange, 200, id, -32_601, "method not found: " + method);
        }
    }

    private void initialize(HttpExchange exchange, JsonElement id, JsonObject params) throws IOException {
        if (id == null) {
            writeError(exchange, 400, null, -32_600, "initialize requires an id");
            return;
        }
        String requested = CURRENT_PROTOCOL_VERSION;
        if (params.has("protocolVersion")) {
            JsonElement version = params.get("protocolVersion");
            if (!version.isJsonPrimitive() || !version.getAsJsonPrimitive().isString()) {
                writeError(exchange, 200, id, -32_602, "protocolVersion must be a string");
                return;
            }
            requested = version.getAsString();
        }
        String negotiated = SUPPORTED_PROTOCOL_VERSIONS.contains(requested)
            ? requested
            : CURRENT_PROTOCOL_VERSION;
        final String sessionId;
        synchronized (sessions) {
            purgeExpiredSessionsLocked(System.nanoTime());
            if (sessions.size() >= MAX_SESSIONS) {
                writeError(exchange, 429, id, -32_000, "too many active MCP sessions");
                return;
            }
            sessionId = UUID.randomUUID().toString();
            sessions.put(sessionId, new Session(negotiated));
        }

        JsonObject capabilities = new JsonObject();
        JsonObject toolCapabilities = new JsonObject();
        toolCapabilities.addProperty("listChanged", false);
        capabilities.add("tools", toolCapabilities);

        JsonObject serverInfo = new JsonObject();
        serverInfo.addProperty("name", "solaris-minecraft-client");
        serverInfo.addProperty("version", "0.1.0");

        JsonObject result = new JsonObject();
        result.addProperty("protocolVersion", negotiated);
        result.add("capabilities", capabilities);
        result.add("serverInfo", serverInfo);
        result.addProperty(
            "instructions",
            "Inspect the real Minecraft client with bounded observation tools before issuing controls. "
                + "Screenshots are optional visual context, not authoritative state."
        );
        exchange.getResponseHeaders().set("Mcp-Session-Id", sessionId);
        try {
            writeResult(exchange, id, result);
        } catch (IOException | RuntimeException error) {
            sessions.remove(sessionId);
            throw error;
        }
    }

    private void listTools(HttpExchange exchange, JsonElement id) throws IOException {
        JsonObject result = new JsonObject();
        com.google.gson.JsonArray listed = new com.google.gson.JsonArray();
        for (McpToolDefinition tool : tools.values()) {
            listed.add(tool.toJson());
        }
        result.add("tools", listed);
        writeResult(exchange, id, result);
    }

    private void callTool(HttpExchange exchange, JsonElement id, JsonObject params) throws IOException {
        if (!params.has("name") || !params.get("name").isJsonPrimitive()) {
            writeError(exchange, 200, id, -32_602, "tools/call requires a tool name");
            return;
        }
        String name = params.get("name").getAsString();
        McpToolDefinition tool = tools.get(name);
        if (tool == null) {
            writeError(exchange, 200, id, -32_602, "unknown tool: " + name);
            return;
        }
        final JsonObject arguments;
        try {
            arguments = objectOrEmpty(params.get("arguments"));
        } catch (IllegalArgumentException error) {
            writeError(exchange, 200, id, -32_602, error.getMessage());
            return;
        }
        try {
            JsonObject payload = executeTool(tool, arguments);
            writeResult(exchange, id, toolResult(payload, false));
        } catch (Exception error) {
            JsonObject payload = new JsonObject();
            payload.addProperty("code", "tool-execution-failed");
            payload.addProperty("message", safeMessage(error));
            writeResult(exchange, id, toolResult(payload, true));
        }
    }

    private JsonObject executeTool(McpToolDefinition tool, JsonObject arguments) throws Exception {
        return switch (tool.execution()) {
            case DIRECT -> executeCommand(tool.command(), arguments);
            case WAIT_FOR_BLOCK_STATE -> waitForBlockState(arguments);
        };
    }

    private JsonObject waitForBlockState(JsonObject arguments) throws Exception {
        String expectedBlockId = requiredString(arguments, "block_id", 128);
        Map<String, String> expectedProperties = expectedBlockProperties(arguments);
        double timeoutSeconds = optionalNumber(arguments, "timeout_seconds", 0.1, 120.0, 8.0);
        long deadlineNanos = System.nanoTime() + (long) (timeoutSeconds * 1_000_000_000L);
        int stateEvents = 0;

        JsonObject blockRequest = new JsonObject();
        copyRequired(arguments, blockRequest, "x");
        copyRequired(arguments, blockRequest, "y");
        copyRequired(arguments, blockRequest, "z");

        while (true) {
            long observedVersion = stateVersion();
            blockRequest.addProperty("timeout_seconds", remainingSeconds(deadlineNanos));
            JsonObject block = executeCommand("wait_loaded_block", blockRequest);
            ensureBeforeDeadline(deadlineNanos);
            if (matchesBlock(block, expectedBlockId, expectedProperties)) {
                JsonObject result = new JsonObject();
                result.addProperty("status", "matched");
                result.addProperty("state_events", stateEvents);
                result.add("block", block.deepCopy());
                return result;
            }

            JsonObject waitRequest = new JsonObject();
            waitRequest.addProperty("observed_version", observedVersion);
            waitRequest.addProperty("timeout_seconds", remainingSeconds(deadlineNanos));
            executeCommand("wait_state_change", waitRequest);
            stateEvents += 1;
        }
    }

    private long stateVersion() throws Exception {
        JsonObject state = executeCommand("state", new JsonObject());
        if (!state.has("state_version")
            || !state.get("state_version").isJsonPrimitive()
            || !state.get("state_version").getAsJsonPrimitive().isNumber()) {
            throw new IllegalStateException("state command did not return state_version");
        }
        try {
            return state.get("state_version").getAsBigDecimal().longValueExact();
        } catch (ArithmeticException | NumberFormatException | UnsupportedOperationException error) {
            throw new IllegalStateException("state command returned an invalid state_version", error);
        }
    }

    private JsonObject executeCommand(String name, JsonObject payload) throws Exception {
        BridgeCommand command = commands.find(name).orElseThrow(
            () -> new IllegalStateException("tool command is unavailable: " + name)
        );
        return command.execute(new BridgeRequest(
            requestSequence.incrementAndGet(),
            "",
            name,
            payload.deepCopy()
        ));
    }

    private static boolean matchesBlock(
        JsonObject block,
        String expectedBlockId,
        Map<String, String> expectedProperties
    ) {
        if (!block.has("block_id")
            || !block.get("block_id").isJsonPrimitive()
            || !block.get("block_id").getAsJsonPrimitive().isString()
            || !expectedBlockId.equals(block.get("block_id").getAsString())) {
            return false;
        }
        if (expectedProperties.isEmpty()) {
            return true;
        }
        if (!block.has("properties") || !block.get("properties").isJsonObject()) {
            return false;
        }
        JsonObject actualProperties = block.getAsJsonObject("properties");
        return expectedProperties.entrySet().stream().allMatch(expected -> {
            JsonElement actual = actualProperties.get(expected.getKey());
            return actual != null
                && actual.isJsonPrimitive()
                && actual.getAsJsonPrimitive().isString()
                && expected.getValue().equals(actual.getAsString());
        });
    }

    private static Map<String, String> expectedBlockProperties(JsonObject arguments) {
        if (!arguments.has("properties")) {
            return Map.of();
        }
        if (!arguments.get("properties").isJsonObject()) {
            throw new IllegalArgumentException("properties must be an object");
        }
        JsonObject properties = arguments.getAsJsonObject("properties");
        if (properties.size() > 32) {
            throw new IllegalArgumentException("properties must contain at most 32 entries");
        }
        Map<String, String> expected = new LinkedHashMap<>();
        for (Map.Entry<String, JsonElement> entry : properties.entrySet()) {
            if (entry.getKey().isBlank() || entry.getKey().length() > 128) {
                throw new IllegalArgumentException("property names must contain 1..128 characters");
            }
            JsonElement value = entry.getValue();
            if (!value.isJsonPrimitive() || !value.getAsJsonPrimitive().isString()) {
                throw new IllegalArgumentException("property values must be strings");
            }
            String text = value.getAsString();
            if (text.length() > 128) {
                throw new IllegalArgumentException("property values must contain at most 128 characters");
            }
            expected.put(entry.getKey(), text);
        }
        return Collections.unmodifiableMap(expected);
    }

    private static String requiredString(JsonObject arguments, String name, int maximumLength) {
        if (!arguments.has(name)
            || !arguments.get(name).isJsonPrimitive()
            || !arguments.get(name).getAsJsonPrimitive().isString()) {
            throw new IllegalArgumentException(name + " must be a string");
        }
        String value = arguments.get(name).getAsString();
        if (value.isBlank() || value.length() > maximumLength) {
            throw new IllegalArgumentException(
                name + " must contain 1.." + maximumLength + " characters"
            );
        }
        return value;
    }

    private static double optionalNumber(
        JsonObject arguments,
        String name,
        double minimum,
        double maximum,
        double defaultValue
    ) {
        if (!arguments.has(name)) {
            return defaultValue;
        }
        JsonElement element = arguments.get(name);
        if (!element.isJsonPrimitive() || !element.getAsJsonPrimitive().isNumber()) {
            throw new IllegalArgumentException(name + " must be a number");
        }
        double value = element.getAsDouble();
        if (!Double.isFinite(value) || value < minimum || value > maximum) {
            throw new IllegalArgumentException(
                name + " must be between " + minimum + " and " + maximum
            );
        }
        return value;
    }

    private static void copyRequired(JsonObject source, JsonObject target, String name) {
        if (!source.has(name)) {
            throw new IllegalArgumentException(name + " is required");
        }
        target.add(name, source.get(name).deepCopy());
    }

    private static double remainingSeconds(long deadlineNanos) throws TimeoutException {
        long remainingNanos = deadlineNanos - System.nanoTime();
        if (remainingNanos <= 0L) {
            throw new TimeoutException("block did not reach the requested state before timeout");
        }
        return Math.max(0.1, remainingNanos / 1_000_000_000.0);
    }

    private static void ensureBeforeDeadline(long deadlineNanos) throws TimeoutException {
        if (System.nanoTime() > deadlineNanos) {
            throw new TimeoutException("block did not reach the requested state before timeout");
        }
    }

    private void handleDelete(HttpExchange exchange) throws IOException {
        String sessionId = exchange.getRequestHeaders().getFirst("Mcp-Session-Id");
        Session session = sessionId == null ? null : sessions.get(sessionId);
        if (session == null || session.expired(System.nanoTime())) {
            if (sessionId != null && session != null) {
                sessions.remove(sessionId, session);
            }
            writeEmpty(exchange, 404);
            return;
        }
        if (!validProtocolVersion(exchange, session)) {
            writeError(exchange, 400, null, -32_600, "unsupported MCP-Protocol-Version");
            return;
        }
        sessions.remove(sessionId, session);
        writeEmpty(exchange, 204);
    }

    private Session requireSession(HttpExchange exchange, JsonElement id) throws IOException {
        String sessionId = exchange.getRequestHeaders().getFirst("Mcp-Session-Id");
        if (sessionId == null) {
            writeError(exchange, 400, id, -32_600, "Mcp-Session-Id is required");
            return null;
        }
        Session session = sessions.get(sessionId);
        long now = System.nanoTime();
        if (session == null || session.expired(now)) {
            if (session != null) {
                sessions.remove(sessionId, session);
            }
            writeError(exchange, 404, id, -32_600, "MCP session is unknown or expired");
            return null;
        }
        session.touch(now);
        return session;
    }

    private void purgeExpiredSessionsLocked(long now) {
        sessions.entrySet().removeIf(entry -> entry.getValue().expired(now));
    }

    private boolean validProtocolVersion(HttpExchange exchange, Session session) {
        String version = exchange.getRequestHeaders().getFirst("MCP-Protocol-Version");
        return version == null || (SUPPORTED_PROTOCOL_VERSIONS.contains(version) && session.version.equals(version));
    }

    private boolean authorized(HttpExchange exchange) {
        String supplied = exchange.getRequestHeaders().getFirst("Authorization");
        if (supplied == null) {
            return false;
        }
        return MessageDigest.isEqual(
            authorization.getBytes(StandardCharsets.UTF_8),
            supplied.getBytes(StandardCharsets.UTF_8)
        );
    }

    private static boolean validHost(HttpExchange exchange) {
        return isAllowedHost(exchange.getRequestHeaders().getFirst("Host"));
    }

    static boolean isAllowedHost(String host) {
        if (host == null || host.isBlank()) {
            return false;
        }
        try {
            URI parsed = URI.create("http://" + host);
            return parsed.getRawUserInfo() == null
                && parsed.getRawQuery() == null
                && parsed.getRawFragment() == null
                && (parsed.getRawPath() == null || parsed.getRawPath().isEmpty())
                && isLoopbackHost(parsed.getHost())
                && parsed.getPort() >= -1
                && parsed.getPort() <= 65_535;
        } catch (IllegalArgumentException error) {
            return false;
        }
    }

    private static boolean validOrigin(HttpExchange exchange) {
        String origin = exchange.getRequestHeaders().getFirst("Origin");
        return origin == null || isAllowedOrigin(origin);
    }

    static boolean isAllowedOrigin(String origin) {
        try {
            URI parsed = URI.create(origin);
            String scheme = parsed.getScheme();
            return scheme != null
                && Set.of("http", "https").contains(scheme.toLowerCase(java.util.Locale.ROOT))
                && parsed.getRawUserInfo() == null
                && parsed.getRawQuery() == null
                && parsed.getRawFragment() == null
                && (parsed.getRawPath() == null || parsed.getRawPath().isEmpty())
                && isLoopbackHost(parsed.getHost())
                && parsed.getPort() >= -1
                && parsed.getPort() <= 65_535;
        } catch (IllegalArgumentException error) {
            return false;
        }
    }

    private static boolean isLoopbackHost(String host) {
        if (host == null) {
            return false;
        }
        String normalized = host.toLowerCase(java.util.Locale.ROOT);
        return Set.of("127.0.0.1", "localhost", "::1", "[::1]").contains(normalized);
    }

    private static JsonObject toolResult(JsonObject payload, boolean error) {
        JsonObject result = new JsonObject();
        com.google.gson.JsonArray content = new com.google.gson.JsonArray();
        JsonObject text = new JsonObject();
        text.addProperty("type", "text");
        text.addProperty("text", GSON.toJson(payload));
        content.add(text);
        result.add("content", content);
        result.add("structuredContent", payload.deepCopy());
        result.addProperty("isError", error);
        return result;
    }

    private static JsonObject objectOrEmpty(JsonElement value) {
        if (value == null || value.isJsonNull()) {
            return new JsonObject();
        }
        if (!value.isJsonObject()) {
            throw new IllegalArgumentException("params/arguments must be an object");
        }
        return value.getAsJsonObject();
    }

    private static Map<String, McpToolDefinition> indexTools(List<McpToolDefinition> definitions) {
        Map<String, McpToolDefinition> indexed = new LinkedHashMap<>();
        for (McpToolDefinition definition : definitions) {
            if (indexed.putIfAbsent(definition.name(), definition) != null) {
                throw new IllegalArgumentException("duplicate MCP tool: " + definition.name());
            }
        }
        return Collections.unmodifiableMap(indexed);
    }

    private static void writeResult(HttpExchange exchange, JsonElement id, JsonObject result) throws IOException {
        JsonObject response = new JsonObject();
        response.addProperty("jsonrpc", "2.0");
        response.add("id", id.deepCopy());
        response.add("result", result);
        writeJson(exchange, 200, response);
    }

    private static void writeError(
        HttpExchange exchange,
        int status,
        JsonElement id,
        int code,
        String message
    ) throws IOException {
        JsonObject error = new JsonObject();
        error.addProperty("code", code);
        error.addProperty("message", message);
        JsonObject response = new JsonObject();
        response.addProperty("jsonrpc", "2.0");
        if (id == null) {
            response.add("id", com.google.gson.JsonNull.INSTANCE);
        } else {
            response.add("id", id.deepCopy());
        }
        response.add("error", error);
        writeJson(exchange, status, response);
    }

    private static void writeJson(HttpExchange exchange, int status, JsonObject response) throws IOException {
        byte[] bytes = GSON.toJson(response).getBytes(StandardCharsets.UTF_8);
        exchange.getResponseHeaders().set("Content-Type", "application/json; charset=utf-8");
        exchange.sendResponseHeaders(status, bytes.length);
        exchange.getResponseBody().write(bytes);
        exchange.close();
    }

    private static void writeEmpty(HttpExchange exchange, int status) throws IOException {
        exchange.sendResponseHeaders(status, -1);
        exchange.close();
    }

    private static String safeMessage(Throwable error) {
        Throwable current = error;
        while (current.getCause() != null && current.getCause() != current) {
            current = current.getCause();
        }
        String message = current.getMessage();
        return message == null || message.isBlank() ? current.getClass().getSimpleName() : message;
    }

    @Override
    public void close() {
        server.stop(0);
        sessions.clear();
        executor.shutdownNow();
        try {
            if (!executor.awaitTermination(executorCloseTimeout.toNanos(), TimeUnit.NANOSECONDS)) {
                throw new IllegalStateException("MCP request executor did not terminate after shutdown");
            }
        } catch (InterruptedException error) {
            Thread.currentThread().interrupt();
            throw new IllegalStateException("interrupted while closing MCP request executor", error);
        }
    }

    private static final class Session {
        private final String version;
        private volatile boolean initialized;
        private volatile long lastAccessNanos;

        private Session(String version) {
            this.version = version;
            this.lastAccessNanos = System.nanoTime();
        }

        private void touch(long now) {
            lastAccessNanos = now;
        }

        private boolean expired(long now) {
            return now - lastAccessNanos >= SESSION_IDLE_NANOS;
        }
    }
}
