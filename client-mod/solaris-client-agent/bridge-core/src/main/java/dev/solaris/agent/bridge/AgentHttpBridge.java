package dev.solaris.agent.bridge;

import com.google.gson.JsonObject;
import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;

import java.io.IOException;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public final class AgentHttpBridge implements AutoCloseable {
    private final HttpServer server;
    private final ExecutorService executor;

    private AgentHttpBridge(HttpServer server, ExecutorService executor) {
        this.server = server;
        this.executor = executor;
    }

    public static AgentHttpBridge start(String secret, int port, CommandRegistry registry) throws IOException {
        if (secret == null || secret.isBlank()) {
            throw new IllegalArgumentException("shared secret must not be blank");
        }
        HttpServer server = HttpServer.create(new InetSocketAddress(InetAddress.getLoopbackAddress(), port), 0);
        ExecutorService executor = Executors.newSingleThreadExecutor(runnable -> {
            Thread thread = new Thread(runnable, "solaris-client-agent-bridge");
            thread.setDaemon(true);
            return thread;
        });
        server.setExecutor(executor);
        server.createContext("/rpc", exchange -> handle(exchange, secret, registry));
        server.start();
        return new AgentHttpBridge(server, executor);
    }

    public int port() {
        return server.getAddress().getPort();
    }

    private static void handle(HttpExchange exchange, String secret, CommandRegistry registry) throws IOException {
        if (!"POST".equals(exchange.getRequestMethod())) {
            write(exchange, 405, BridgeCodec.encodeResponse(
                BridgeResponse.failure(0, new BridgeError("method-not-allowed", "POST required"))));
            return;
        }

        BridgeRequest request;
        try {
            String body = new String(exchange.getRequestBody().readAllBytes(), StandardCharsets.UTF_8);
            request = BridgeCodec.decodeRequest(body);
        } catch (RuntimeException error) {
            write(exchange, 400, BridgeCodec.encodeResponse(
                BridgeResponse.failure(0, new BridgeError("bad-request", error.getMessage()))));
            return;
        }

        if (!secret.equals(request.secret())) {
            write(exchange, 403, BridgeCodec.encodeResponse(
                BridgeResponse.failure(request.id(), new BridgeError("forbidden", "shared secret mismatch"))));
            return;
        }

        BridgeCommand command = registry.find(request.command()).orElse(null);
        if (command == null) {
            write(exchange, 404, BridgeCodec.encodeResponse(BridgeResponse.failure(
                request.id(), new BridgeError("unknown-command", "unsupported command: " + request.command()))));
            return;
        }

        try {
            JsonObject payload = command.execute(request);
            write(exchange, 200, BridgeCodec.encodeResponse(BridgeResponse.success(request.id(), payload)));
        } catch (Exception error) {
            write(exchange, 500, BridgeCodec.encodeResponse(BridgeResponse.failure(
                request.id(), new BridgeError("command-failed", error.getMessage()))));
        }
    }

    private static void write(HttpExchange exchange, int status, String body) throws IOException {
        byte[] bytes = body.getBytes(StandardCharsets.UTF_8);
        exchange.getResponseHeaders().set("Content-Type", "application/json; charset=utf-8");
        exchange.sendResponseHeaders(status, bytes.length);
        exchange.getResponseBody().write(bytes);
        exchange.close();
    }

    @Override
    public void close() {
        server.stop(0);
        executor.shutdownNow();
    }
}
