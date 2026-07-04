package dev.solaris.agent.bridge;

import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class AgentHttpBridgeTest {
    @Test
    void acceptsLoopbackRpcWithSharedSecret() throws Exception {
        CommandRegistry registry = new CommandRegistry();
        registry.register("ping", request -> {
            JsonObject payload = new JsonObject();
            payload.addProperty("pong", true);
            return payload;
        });

        try (AgentHttpBridge bridge = AgentHttpBridge.start("run-secret", 0, registry)) {
            HttpResponse<String> response = post(bridge.port(), """
                {"id":1,"secret":"run-secret","command":"ping","payload":{}}
                """);

            assertEquals(200, response.statusCode());
            assertTrue(response.body().contains("\"ok\":true"));
            assertTrue(response.body().contains("\"pong\":true"));
        }
    }

    @Test
    void rejectsMissingSecretWithoutExecutingCommand() throws Exception {
        CommandRegistry registry = new CommandRegistry();
        registry.register("ping", request -> {
            throw new AssertionError("command must not execute without secret");
        });

        try (AgentHttpBridge bridge = AgentHttpBridge.start("run-secret", 0, registry)) {
            HttpResponse<String> response = post(bridge.port(), """
                {"id":2,"secret":"wrong","command":"ping","payload":{}}
                """);

            assertEquals(403, response.statusCode());
            assertTrue(response.body().contains("\"ok\":false"));
            assertTrue(response.body().contains("\"code\":\"forbidden\""));
        }
    }

    @Test
    void rejectsUnknownCommandAsStructuredError() throws Exception {
        try (AgentHttpBridge bridge = AgentHttpBridge.start("run-secret", 0, new CommandRegistry())) {
            HttpResponse<String> response = post(bridge.port(), """
                {"id":3,"secret":"run-secret","command":"mine","payload":{}}
                """);

            assertEquals(404, response.statusCode());
            assertTrue(response.body().contains("\"ok\":false"));
            assertTrue(response.body().contains("\"code\":\"unknown-command\""));
        }
    }

    private static HttpResponse<String> post(int port, String body) throws Exception {
        HttpRequest request = HttpRequest.newBuilder()
            .uri(URI.create("http://127.0.0.1:" + port + "/rpc"))
            .header("Content-Type", "application/json")
            .POST(HttpRequest.BodyPublishers.ofString(body))
            .build();
        return HttpClient.newHttpClient().send(request, HttpResponse.BodyHandlers.ofString());
    }
}
