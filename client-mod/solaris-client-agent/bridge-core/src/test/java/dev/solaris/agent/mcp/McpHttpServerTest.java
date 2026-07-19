package dev.solaris.agent.mcp;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import dev.solaris.agent.bridge.CommandRegistry;
import org.junit.jupiter.api.Test;

import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class McpHttpServerTest {
    private static final String TOKEN = "mcp-test-token";
    private static final String VERSION = "2025-11-25";

    @Test
    void returnsAnAlreadyMatchingBlockAtTheMinimumTimeout() throws Exception {
        CommandRegistry commands = new CommandRegistry();
        commands.registerConcurrent("state", request -> state(3));
        commands.registerConcurrent("wait_loaded_block", request -> block("minecraft:air", ""));
        commands.registerConcurrent("wait_state_change", request -> {
            throw new AssertionError("an already matched state must not wait for another event");
        });
        McpToolDefinition tool = ClientMcpTools.definitions().stream()
            .filter(candidate -> candidate.name().equals("minecraft_wait_for_block_state"))
            .findFirst()
            .orElseThrow();

        try (McpHttpServer server = McpHttpServer.start(TOKEN, 0, commands, List.of(tool))) {
            String session = initialize(server.port());
            JsonObject result = json(post(server.port(), session, VERSION, """
                {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"minecraft_wait_for_block_state","arguments":{"x":4,"y":64,"z":9,"block_id":"minecraft:air","timeout_seconds":0.1}}}
                """)).getAsJsonObject("result");

            assertFalse(result.get("isError").getAsBoolean());
            assertEquals(0, result.getAsJsonObject("structuredContent").get("state_events").getAsInt());
        }
    }

    @Test
    void waitsForExactBlockStateOnlyAfterAClientStateEvent() throws Exception {
        CommandRegistry commands = new CommandRegistry();
        AtomicInteger stateVersion = new AtomicInteger(10);
        AtomicReference<String> blockId = new AtomicReference<>("minecraft:wheat");
        AtomicReference<String> age = new AtomicReference<>("6");
        AtomicInteger waitCalls = new AtomicInteger();
        CountDownLatch waitStarted = new CountDownLatch(1);
        CountDownLatch packetApplied = new CountDownLatch(1);
        commands.registerConcurrent("state", request -> state(stateVersion.get()));
        commands.registerConcurrent("wait_loaded_block", request -> block(blockId.get(), age.get()));
        commands.registerConcurrent("wait_state_change", request -> {
            waitCalls.incrementAndGet();
            assertEquals(10, request.payload().get("observed_version").getAsInt());
            waitStarted.countDown();
            if (!packetApplied.await(2, TimeUnit.SECONDS)) {
                throw new AssertionError("test packet event was not published");
            }
            return state(stateVersion.get());
        });

        McpToolDefinition tool = ClientMcpTools.definitions().stream()
            .filter(candidate -> candidate.name().equals("minecraft_wait_for_block_state"))
            .findFirst()
            .orElseThrow();
        try (McpHttpServer server = McpHttpServer.start(TOKEN, 0, commands, List.of(tool))) {
            String session = initialize(server.port());
            CompletableFuture<HttpResponse<String>> call = asyncPost(server.port(), session, VERSION, """
                {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"minecraft_wait_for_block_state","arguments":{"x":4,"y":64,"z":9,"block_id":"minecraft:wheat","properties":{"age":"7"},"timeout_seconds":2}}}
                """);

            assertTrue(waitStarted.await(2, TimeUnit.SECONDS));
            assertFalse(call.isDone(), "the tool must remain blocked until the exact client event arrives");
            age.set("7");
            stateVersion.incrementAndGet();
            packetApplied.countDown();

            JsonObject result = json(call.get(2, TimeUnit.SECONDS)).getAsJsonObject("result");
            assertFalse(result.get("isError").getAsBoolean());
            JsonObject observation = result.getAsJsonObject("structuredContent");
            assertEquals(1, observation.get("state_events").getAsInt());
            assertEquals("minecraft:wheat", observation.getAsJsonObject("block").get("block_id").getAsString());
            assertEquals("7", observation.getAsJsonObject("block")
                .getAsJsonObject("properties").get("age").getAsString());
            assertEquals(1, waitCalls.get());
        }
    }

    @Test
    void dispatchesRespawnDirectlyToItsCommand() throws Exception {
        CommandRegistry commands = new CommandRegistry();
        AtomicReference<JsonObject> payload = new AtomicReference<>();
        commands.register("respawn", request -> {
            payload.set(request.payload());
            JsonObject result = new JsonObject();
            result.addProperty("status", "respawned");
            return result;
        });
        McpToolDefinition tool = ClientMcpTools.definitions().stream()
            .filter(candidate -> candidate.name().equals("minecraft_respawn"))
            .findFirst()
            .orElseThrow();

        try (McpHttpServer server = McpHttpServer.start(TOKEN, 0, commands, List.of(tool))) {
            String session = initialize(server.port());
            JsonObject result = json(post(server.port(), session, VERSION, """
                {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"minecraft_respawn","arguments":{"timeout_seconds":8.0}}}
                """)).getAsJsonObject("result");

            assertFalse(result.get("isError").getAsBoolean());
            assertEquals("respawned", result.getAsJsonObject("structuredContent")
                .get("status").getAsString());
            assertEquals(8.0, payload.get().get("timeout_seconds").getAsDouble());
        }
    }

    @Test
    void initializesListsAndCallsTypedToolOverStreamableHttp() throws Exception {
        CommandRegistry commands = new CommandRegistry();
        commands.register("echo", request -> {
            JsonObject response = new JsonObject();
            response.addProperty("echo", request.payload().get("value").getAsString());
            return response;
        });
        McpToolDefinition tool = new McpToolDefinition(
            "minecraft_echo",
            "Echo a value through the in-client command registry.",
            "echo",
            objectSchema("value", "string", true),
            true
        );

        try (McpHttpServer server = McpHttpServer.start(TOKEN, 0, commands, List.of(tool))) {
            HttpResponse<String> initialized = post(server.port(), null, null, """
                {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}
                """);

            assertEquals(200, initialized.statusCode());
            String session = initialized.headers().firstValue("Mcp-Session-Id").orElseThrow();
            JsonObject initializeResult = json(initialized).getAsJsonObject("result");
            assertEquals(VERSION, initializeResult.get("protocolVersion").getAsString());
            assertEquals("solaris-minecraft-client", initializeResult.getAsJsonObject("serverInfo").get("name").getAsString());

            HttpResponse<String> listed = post(server.port(), session, VERSION, """
                {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
                """);
            JsonObject listedTool = json(listed)
                .getAsJsonObject("result")
                .getAsJsonArray("tools")
                .get(0)
                .getAsJsonObject();
            assertEquals("minecraft_echo", listedTool.get("name").getAsString());
            assertTrue(listedTool.getAsJsonObject("annotations").get("readOnlyHint").getAsBoolean());

            HttpResponse<String> called = post(server.port(), session, VERSION, """
                {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"minecraft_echo","arguments":{"value":"hello"}}}
                """);
            JsonObject callResult = json(called).getAsJsonObject("result");
            assertFalse(callResult.get("isError").getAsBoolean());
            assertEquals("hello", callResult.getAsJsonObject("structuredContent").get("echo").getAsString());
            assertTrue(callResult.getAsJsonArray("content").get(0).getAsJsonObject().get("text").getAsString().contains("hello"));
        }
    }

    @Test
    void rejectsMissingBearerAndNonLoopbackOriginBeforeDispatch() throws Exception {
        CommandRegistry commands = new CommandRegistry();
        commands.register("explode", request -> {
            throw new AssertionError("unauthorized request must not dispatch");
        });
        McpToolDefinition tool = new McpToolDefinition(
            "minecraft_explode",
            "Must not execute.",
            "explode",
            objectSchema(null, null, false),
            false
        );

        try (McpHttpServer server = McpHttpServer.start(TOKEN, 0, commands, List.of(tool))) {
            HttpResponse<String> missingBearer = rawPost(server.port(), null, null, null, """
                {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}
                """);
            assertEquals(401, missingBearer.statusCode());

            HttpResponse<String> foreignOrigin = rawPost(
                server.port(),
                "Bearer " + TOKEN,
                null,
                "https://example.com",
                """
                    {"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}
                    """
            );
            assertEquals(403, foreignOrigin.statusCode());
        }
    }

    @Test
    void acceptsInitializedNotificationAndDeletesSession() throws Exception {
        try (McpHttpServer server = McpHttpServer.start(TOKEN, 0, new CommandRegistry(), List.of())) {
            HttpResponse<String> initialized = post(server.port(), null, null, """
                {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}
                """);
            String session = initialized.headers().firstValue("Mcp-Session-Id").orElseThrow();

            HttpResponse<String> notification = post(server.port(), session, "2025-06-18", """
                {"jsonrpc":"2.0","method":"notifications/initialized"}
                """);
            assertEquals(202, notification.statusCode());
            assertTrue(notification.body().isEmpty());

            HttpResponse<String> deleted = delete(server.port(), session, "2025-06-18");
            assertEquals(204, deleted.statusCode());

            HttpResponse<String> stale = post(server.port(), session, "2025-06-18", """
                {"jsonrpc":"2.0","id":2,"method":"ping","params":{}}
                """);
            assertEquals(404, stale.statusCode());
        }
    }

    @Test
    void returnsToolErrorWithoutKillingMcpSession() throws Exception {
        CommandRegistry commands = new CommandRegistry();
        commands.register("fail", request -> {
            throw new IllegalStateException("client is not in play");
        });
        McpToolDefinition tool = new McpToolDefinition(
            "minecraft_fail",
            "Fail predictably.",
            "fail",
            objectSchema(null, null, false),
            false
        );

        try (McpHttpServer server = McpHttpServer.start(TOKEN, 0, commands, List.of(tool))) {
            HttpResponse<String> initialized = post(server.port(), null, null, """
                {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}
                """);
            String session = initialized.headers().firstValue("Mcp-Session-Id").orElseThrow();

            HttpResponse<String> failed = post(server.port(), session, VERSION, """
                {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"minecraft_fail","arguments":{}}}
                """);
            JsonObject toolResult = json(failed).getAsJsonObject("result");
            assertTrue(toolResult.get("isError").getAsBoolean());
            assertEquals("client is not in play", toolResult.getAsJsonObject("structuredContent").get("message").getAsString());

            HttpResponse<String> ping = post(server.port(), session, VERSION, """
                {"jsonrpc":"2.0","id":3,"method":"ping","params":{}}
                """);
            assertEquals(200, ping.statusCode());
            assertTrue(json(ping).getAsJsonObject("result").isEmpty());
        }
    }

    @Test
    void reportsInvalidParamsWithoutKillingMcpSession() throws Exception {
        try (McpHttpServer server = McpHttpServer.start(TOKEN, 0, new CommandRegistry(), List.of())) {
            HttpResponse<String> initialized = post(server.port(), null, null, """
                {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}
                """);
            String session = initialized.headers().firstValue("Mcp-Session-Id").orElseThrow();

            HttpResponse<String> invalid = post(server.port(), session, VERSION, """
                {"jsonrpc":"2.0","id":2,"method":"tools/list","params":[]}
                """);

            assertEquals(200, invalid.statusCode());
            assertEquals(-32_602, json(invalid).getAsJsonObject("error").get("code").getAsInt());

            HttpResponse<String> ping = post(server.port(), session, VERSION, """
                {"jsonrpc":"2.0","id":3,"method":"ping","params":{}}
                """);
            assertEquals(200, ping.statusCode());
        }
    }

    @Test
    void hostAndOriginValidationRejectsAuthorityPrefixTricks() {
        assertTrue(McpHttpServer.isAllowedHost("127.0.0.1:39095"));
        assertTrue(McpHttpServer.isAllowedHost("localhost:39095"));
        assertFalse(McpHttpServer.isAllowedHost("127.0.0.1:39095.evil.example"));
        assertFalse(McpHttpServer.isAllowedHost("localhost.evil.example:39095"));
        assertFalse(McpHttpServer.isAllowedHost("user@localhost:39095"));

        assertTrue(McpHttpServer.isAllowedOrigin("http://127.0.0.1:3000"));
        assertTrue(McpHttpServer.isAllowedOrigin("https://localhost"));
        assertFalse(McpHttpServer.isAllowedOrigin("file://localhost/tmp"));
        assertFalse(McpHttpServer.isAllowedOrigin("https://localhost.evil.example"));
        assertFalse(McpHttpServer.isAllowedOrigin("https://user@localhost"));
    }

    @Test
    void deleteRejectsMismatchedProtocolWithoutRemovingSession() throws Exception {
        try (McpHttpServer server = McpHttpServer.start(TOKEN, 0, new CommandRegistry(), List.of())) {
            HttpResponse<String> initialized = post(server.port(), null, null, """
                {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}
                """);
            String session = initialized.headers().firstValue("Mcp-Session-Id").orElseThrow();

            assertEquals(400, delete(server.port(), session, "2025-06-18").statusCode());
            assertEquals(200, post(server.port(), session, VERSION, """
                {"jsonrpc":"2.0","id":2,"method":"ping","params":{}}
                """).statusCode());
        }
    }

    @Test
    void capsAbandonedSessions() throws Exception {
        try (McpHttpServer server = McpHttpServer.start(TOKEN, 0, new CommandRegistry(), List.of())) {
            for (int index = 0; index < McpHttpServer.MAX_SESSIONS; index++) {
                assertEquals(200, post(server.port(), null, null, """
                    {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}
                    """).statusCode());
            }
            assertEquals(429, post(server.port(), null, null, """
                {"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}
                """).statusCode());
        }
    }

    @Test
    void closeInterruptsHandlersAndWaitsForExecutorTermination() throws Exception {
        CommandRegistry commands = new CommandRegistry();
        CountDownLatch handlerStarted = new CountDownLatch(1);
        CountDownLatch handlerInterrupted = new CountDownLatch(1);
        commands.registerConcurrent("blocking", request -> {
            handlerStarted.countDown();
            try {
                new CountDownLatch(1).await();
                throw new AssertionError("handler wait returned without interruption");
            } catch (InterruptedException expected) {
                handlerInterrupted.countDown();
                JsonObject result = new JsonObject();
                result.addProperty("status", "interrupted");
                return result;
            }
        });
        McpToolDefinition tool = new McpToolDefinition(
            "minecraft_blocking",
            "Block until server close interrupts the request.",
            "blocking",
            objectSchema(null, null, false),
            false
        );
        McpHttpServer server = McpHttpServer.start(
            TOKEN,
            0,
            commands,
            List.of(tool),
            Duration.ofSeconds(1)
        );
        String session = initialize(server.port());
        CompletableFuture<HttpResponse<String>> call = asyncPost(server.port(), session, VERSION, """
            {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"minecraft_blocking","arguments":{}}}
            """);

        assertTrue(handlerStarted.await(1, TimeUnit.SECONDS));
        server.close();

        assertTrue(handlerInterrupted.await(1, TimeUnit.SECONDS));
        call.cancel(true);
    }

    @Test
    void closeReportsAnInterruptResistantHandlerThatOutlivesItsDeadline() throws Exception {
        CommandRegistry commands = new CommandRegistry();
        CountDownLatch handlerStarted = new CountDownLatch(1);
        CountDownLatch releaseHandler = new CountDownLatch(1);
        CountDownLatch handlerExited = new CountDownLatch(1);
        commands.registerConcurrent("stuck", request -> {
            handlerStarted.countDown();
            boolean released = false;
            while (!released) {
                try {
                    releaseHandler.await();
                    released = true;
                } catch (InterruptedException ignored) {
                    // Deliberately resist shutdown to verify that close reports the stuck task.
                }
            }
            handlerExited.countDown();
            return new JsonObject();
        });
        McpToolDefinition tool = new McpToolDefinition(
            "minecraft_stuck",
            "Remain active after interruption for lifecycle testing.",
            "stuck",
            objectSchema(null, null, false),
            false
        );
        McpHttpServer server = McpHttpServer.start(
            TOKEN,
            0,
            commands,
            List.of(tool),
            Duration.ZERO
        );
        String session = initialize(server.port());
        CompletableFuture<HttpResponse<String>> call = asyncPost(server.port(), session, VERSION, """
            {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"minecraft_stuck","arguments":{}}}
            """);

        try {
            assertTrue(handlerStarted.await(1, TimeUnit.SECONDS));
            IllegalStateException failure = assertThrows(IllegalStateException.class, server::close);
            assertTrue(failure.getMessage().contains("did not terminate"));
        } finally {
            releaseHandler.countDown();
            assertTrue(handlerExited.await(1, TimeUnit.SECONDS));
            call.cancel(true);
        }
    }

    @Test
    void interruptedClosePreservesTheCallingThreadsInterruptStatus() throws Exception {
        CommandRegistry commands = new CommandRegistry();
        CountDownLatch handlerStarted = new CountDownLatch(1);
        CountDownLatch shutdownInterruptObserved = new CountDownLatch(1);
        CountDownLatch releaseHandler = new CountDownLatch(1);
        commands.registerConcurrent("stuck", request -> {
            handlerStarted.countDown();
            boolean released = false;
            while (!released) {
                try {
                    releaseHandler.await();
                    released = true;
                } catch (InterruptedException ignored) {
                    shutdownInterruptObserved.countDown();
                }
            }
            return new JsonObject();
        });
        McpToolDefinition tool = new McpToolDefinition(
            "minecraft_stuck",
            "Remain active while close interruption is tested.",
            "stuck",
            objectSchema(null, null, false),
            false
        );
        McpHttpServer server = McpHttpServer.start(
            TOKEN,
            0,
            commands,
            List.of(tool),
            Duration.ofSeconds(10)
        );
        String session = initialize(server.port());
        CompletableFuture<HttpResponse<String>> call = asyncPost(server.port(), session, VERSION, """
            {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"minecraft_stuck","arguments":{}}}
            """);
        assertTrue(handlerStarted.await(1, TimeUnit.SECONDS));

        AtomicReference<Throwable> closeFailure = new AtomicReference<>();
        AtomicBoolean interruptPreserved = new AtomicBoolean();
        CountDownLatch closeReturned = new CountDownLatch(1);
        Thread closer = Thread.ofPlatform().start(() -> {
            try {
                server.close();
            } catch (Throwable failure) {
                closeFailure.set(failure);
                interruptPreserved.set(Thread.currentThread().isInterrupted());
            } finally {
                closeReturned.countDown();
            }
        });

        try {
            assertTrue(shutdownInterruptObserved.await(1, TimeUnit.SECONDS));
            closer.interrupt();
            assertTrue(closeReturned.await(1, TimeUnit.SECONDS));
            assertTrue(closeFailure.get() instanceof IllegalStateException);
            assertTrue(closeFailure.get().getMessage().contains("interrupted while closing"));
            assertTrue(interruptPreserved.get());
        } finally {
            releaseHandler.countDown();
            closer.join();
            call.cancel(true);
        }
    }

    private static JsonObject objectSchema(String property, String type, boolean required) {
        JsonObject schema = JsonParser.parseString("{\"type\":\"object\",\"additionalProperties\":false}")
            .getAsJsonObject();
        JsonObject properties = new JsonObject();
        if (property != null) {
            JsonObject value = new JsonObject();
            value.addProperty("type", type);
            properties.add(property, value);
        }
        schema.add("properties", properties);
        if (required) {
            schema.add("required", JsonParser.parseString("[\"" + property + "\"]"));
        }
        return schema;
    }

    private static JsonObject state(int version) {
        JsonObject state = new JsonObject();
        state.addProperty("state_version", version);
        return state;
    }

    private static JsonObject block(String blockId, String age) {
        JsonObject block = new JsonObject();
        block.addProperty("block_id", blockId);
        JsonObject properties = new JsonObject();
        properties.addProperty("age", age);
        block.add("properties", properties);
        return block;
    }

    private static String initialize(int port) throws Exception {
        return post(port, null, null, """
            {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}
            """).headers().firstValue("Mcp-Session-Id").orElseThrow();
    }

    private static CompletableFuture<HttpResponse<String>> asyncPost(
        int port,
        String session,
        String version,
        String body
    ) {
        HttpRequest request = requestBuilder(port, session, version)
            .header("Authorization", "Bearer " + TOKEN)
            .POST(HttpRequest.BodyPublishers.ofString(body))
            .build();
        return HttpClient.newHttpClient().sendAsync(request, HttpResponse.BodyHandlers.ofString());
    }

    private static HttpResponse<String> post(int port, String session, String version, String body) throws Exception {
        return rawPost(port, "Bearer " + TOKEN, session, null, body, version);
    }

    private static HttpResponse<String> rawPost(
        int port,
        String authorization,
        String session,
        String origin,
        String body
    ) throws Exception {
        return rawPost(port, authorization, session, origin, body, null);
    }

    private static HttpResponse<String> rawPost(
        int port,
        String authorization,
        String session,
        String origin,
        String body,
        String version
    ) throws Exception {
        HttpRequest.Builder builder = requestBuilder(port, session, version);
        if (authorization != null) {
            builder.header("Authorization", authorization);
        }
        if (origin != null) {
            builder.header("Origin", origin);
        }
        return HttpClient.newHttpClient().send(
            builder.POST(HttpRequest.BodyPublishers.ofString(body)).build(),
            HttpResponse.BodyHandlers.ofString()
        );
    }

    private static HttpRequest.Builder requestBuilder(int port, String session, String version) {
        HttpRequest.Builder builder = HttpRequest.newBuilder()
            .uri(URI.create("http://127.0.0.1:" + port + "/mcp"))
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json");
        if (session != null) {
            builder.header("Mcp-Session-Id", session);
        }
        if (version != null) {
            builder.header("MCP-Protocol-Version", version);
        }
        return builder;
    }

    private static HttpResponse<String> delete(int port, String session, String version) throws Exception {
        HttpRequest request = HttpRequest.newBuilder()
            .uri(URI.create("http://127.0.0.1:" + port + "/mcp"))
            .header("Authorization", "Bearer " + TOKEN)
            .header("Mcp-Session-Id", session)
            .header("MCP-Protocol-Version", version)
            .DELETE()
            .build();
        return HttpClient.newHttpClient().send(request, HttpResponse.BodyHandlers.ofString());
    }

    private static JsonObject json(HttpResponse<String> response) {
        return JsonParser.parseString(response.body()).getAsJsonObject();
    }
}
