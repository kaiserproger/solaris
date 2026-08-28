package dev.solaris.agent.bridge;

import com.google.gson.JsonObject;
import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;

import java.io.IOException;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;

public final class AgentHttpBridge implements AutoCloseable {
    private static final Duration DEFAULT_SHUTDOWN_TIMEOUT = Duration.ofSeconds(5);
    private final HttpServer server;
    private final ExecutorService executor;
    private final Duration shutdownTimeout;

    private AgentHttpBridge(
        HttpServer server,
        ExecutorService executor,
        Duration shutdownTimeout
    ) {
        this.server = server;
        this.executor = executor;
        this.shutdownTimeout = shutdownTimeout;
    }

    public static AgentHttpBridge start(String secret, int port, CommandRegistry registry) throws IOException {
        return start(secret, port, registry, DEFAULT_SHUTDOWN_TIMEOUT);
    }

    static AgentHttpBridge start(
        String secret,
        int port,
        CommandRegistry registry,
        Duration shutdownTimeout
    ) throws IOException {
        if (secret == null || secret.isBlank()) {
            throw new IllegalArgumentException("shared secret must not be blank");
        }
        if (shutdownTimeout == null || shutdownTimeout.isZero() || shutdownTimeout.isNegative()) {
            throw new IllegalArgumentException("shutdown timeout must be positive");
        }
        HttpServer server = HttpServer.create(new InetSocketAddress(InetAddress.getByName("127.0.0.1"), port), 0);
        ExecutorService executor = Executors.newThreadPerTaskExecutor(
            Thread.ofVirtual().name("solaris-client-agent-bridge-", 0).factory()
        );
        server.setExecutor(executor);
        server.createContext("/rpc", exchange -> handle(exchange, secret, registry));
        server.start();
        return new AgentHttpBridge(server, executor, shutdownTimeout);
    }

    public int port() {
        return server.getAddress().getPort();
    }

    public String hostAddress() {
        return server.getAddress().getAddress().getHostAddress();
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
            error.printStackTrace(System.err);
            write(exchange, 500, BridgeCodec.encodeResponse(BridgeResponse.failure(
                request.id(), new BridgeError("command-failed", diagnosticMessage(error)))));
        } catch (Error error) {
            error.printStackTrace(System.err);
            write(exchange, 500, BridgeCodec.encodeResponse(BridgeResponse.failure(
                request.id(), new BridgeError("command-error", diagnosticMessage(error)))));
        }
    }

    private static String diagnosticMessage(Throwable error) {
        String message = error.getMessage();
        if (message == null || message.isBlank()) {
            return error.getClass().getName();
        }
        return error.getClass().getName() + ": " + message;
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
        try {
            if (!executor.awaitTermination(shutdownTimeout.toNanos(), TimeUnit.NANOSECONDS)) {
                throw new IllegalStateException(
                    "HTTP bridge executor did not terminate within " + shutdownTimeout
                );
            }
        } catch (InterruptedException error) {
            Thread.currentThread().interrupt();
            throw new IllegalStateException(
                "interrupted while waiting for HTTP bridge executor termination",
                error
            );
        }
    }
}
