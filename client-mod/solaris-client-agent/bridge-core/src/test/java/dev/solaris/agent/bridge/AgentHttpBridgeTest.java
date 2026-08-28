package dev.solaris.agent.bridge;

import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
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
    void bindsIpv4LoopbackForRunnerUrlCompatibility() throws Exception {
        try (AgentHttpBridge bridge = AgentHttpBridge.start("run-secret", 0, new CommandRegistry())) {
            assertEquals("127.0.0.1", bridge.hostAddress());
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

    @Test
    void convertsCommandErrorsIntoStructuredDiagnosticsInsteadOfDroppingConnection() throws Exception {
        CommandRegistry registry = new CommandRegistry();
        registry.register("explode", request -> {
            throw new AssertionError("long scenario invariant failed");
        });

        try (AgentHttpBridge bridge = AgentHttpBridge.start("run-secret", 0, registry)) {
            HttpResponse<String> response = post(bridge.port(), """
                {"id":31,"secret":"run-secret","command":"explode","payload":{}}
                """);

            assertEquals(500, response.statusCode());
            assertTrue(response.body().contains("\"ok\":false"));
            assertTrue(response.body().contains("\"code\":\"command-error\""));
            assertTrue(response.body().contains("java.lang.AssertionError: long scenario invariant failed"));
        }
    }

    @Test
    void servesObservationWhileLongCommandIsRunning() throws Exception {
        CountDownLatch longCommandStarted = new CountDownLatch(1);
        CountDownLatch releaseLongCommand = new CountDownLatch(1);
        CountDownLatch observationExecuted = new CountDownLatch(1);
        CommandRegistry registry = new CommandRegistry();
        registry.register("long-command", request -> {
            longCommandStarted.countDown();
            if (!releaseLongCommand.await(5, TimeUnit.SECONDS)) {
                throw new AssertionError("test did not release long command");
            }
            return new JsonObject();
        });
        registry.registerConcurrent("state", request -> {
            observationExecuted.countDown();
            return new JsonObject();
        });

        ExecutorService clients = Executors.newFixedThreadPool(2);
        try (AgentHttpBridge bridge = AgentHttpBridge.start("run-secret", 0, registry)) {
            Future<HttpResponse<String>> longCommand = clients.submit(() -> post(bridge.port(), """
                {"id":4,"secret":"run-secret","command":"long-command","payload":{}}
                """));
            assertTrue(longCommandStarted.await(2, TimeUnit.SECONDS));

            Future<HttpResponse<String>> observation = clients.submit(() -> post(bridge.port(), """
                {"id":5,"secret":"run-secret","command":"state","payload":{}}
                """));
            assertTrue(
                observationExecuted.await(2, TimeUnit.SECONDS),
                "state command must execute before the long command completes"
            );

            releaseLongCommand.countDown();
            assertEquals(200, observation.get(2, TimeUnit.SECONDS).statusCode());
            assertEquals(200, longCommand.get(2, TimeUnit.SECONDS).statusCode());
        } finally {
            releaseLongCommand.countDown();
            clients.shutdownNow();
        }
    }

    @Test
    void dispatchesControlWhileNotificationWaitsOccupyLegacyWorkerCount() throws Exception {
        int legacyWorkerCount = 4;
        CountDownLatch waitsStarted = new CountDownLatch(legacyWorkerCount);
        CountDownLatch releaseWaits = new CountDownLatch(1);
        CountDownLatch controlExecuted = new CountDownLatch(1);
        CommandRegistry registry = new CommandRegistry();
        registry.registerConcurrent("wait_state_change", request -> {
            waitsStarted.countDown();
            if (!releaseWaits.await(5, TimeUnit.SECONDS)) {
                throw new AssertionError("test did not release notification wait");
            }
            return new JsonObject();
        });
        registry.register("disconnect", request -> {
            controlExecuted.countDown();
            return new JsonObject();
        });

        ExecutorService clients = Executors.newFixedThreadPool(legacyWorkerCount + 1);
        List<Future<HttpResponse<String>>> waits = new ArrayList<>();
        try (AgentHttpBridge bridge = AgentHttpBridge.start("run-secret", 0, registry)) {
            for (int index = 0; index < legacyWorkerCount; index += 1) {
                long requestId = 100L + index;
                waits.add(clients.submit(() -> post(bridge.port(), """
                    {"id":%d,"secret":"run-secret","command":"wait_state_change","payload":{}}
                    """.formatted(requestId))));
            }
            assertTrue(waitsStarted.await(2, TimeUnit.SECONDS));

            Future<HttpResponse<String>> control = clients.submit(() -> post(bridge.port(), """
                {"id":200,"secret":"run-secret","command":"disconnect","payload":{}}
                """));
            assertTrue(
                controlExecuted.await(2, TimeUnit.SECONDS),
                "control RPC must dispatch before notification waits complete"
            );

            assertEquals(200, control.get(2, TimeUnit.SECONDS).statusCode());
            releaseWaits.countDown();
            for (Future<HttpResponse<String>> wait : waits) {
                assertEquals(200, wait.get(2, TimeUnit.SECONDS).statusCode());
            }
        } finally {
            releaseWaits.countDown();
            clients.shutdownNow();
        }
    }

    @Test
    void closeInterruptsOutstandingNotificationWaits() throws Exception {
        CountDownLatch waitStarted = new CountDownLatch(1);
        CountDownLatch waitInterrupted = new CountDownLatch(1);
        CountDownLatch releaseWait = new CountDownLatch(1);
        CommandRegistry registry = new CommandRegistry();
        registry.registerConcurrent("wait_state_change", request -> {
            waitStarted.countDown();
            try {
                if (!releaseWait.await(10, TimeUnit.SECONDS)) {
                    throw new AssertionError("test did not close or release notification wait");
                }
            } catch (InterruptedException error) {
                waitInterrupted.countDown();
                throw error;
            }
            return new JsonObject();
        });

        ExecutorService clients = Executors.newFixedThreadPool(2);
        AgentHttpBridge bridge = AgentHttpBridge.start("run-secret", 0, registry);
        Future<?> close = null;
        try {
            clients.submit(() -> post(bridge.port(), """
                {"id":300,"secret":"run-secret","command":"wait_state_change","payload":{}}
                """));
            assertTrue(waitStarted.await(2, TimeUnit.SECONDS));

            close = clients.submit(bridge::close);
            assertTrue(
                waitInterrupted.await(2, TimeUnit.SECONDS),
                "bridge close must interrupt outstanding notification waits"
            );
            close.get(2, TimeUnit.SECONDS);
        } finally {
            releaseWait.countDown();
            if (close == null) {
                bridge.close();
            } else {
                close.get(7, TimeUnit.SECONDS);
            }
            clients.shutdownNow();
        }
    }

    @Test
    void closeFailsWhenHandlerOutlivesShutdownBoundAndCanTerminateAfterRelease() throws Exception {
        Duration shutdownTimeout = Duration.ofMillis(50);
        CountDownLatch waitStarted = new CountDownLatch(1);
        CountDownLatch interruptObserved = new CountDownLatch(1);
        CountDownLatch releaseWait = new CountDownLatch(1);
        CountDownLatch handlerTerminated = new CountDownLatch(1);
        CommandRegistry registry = new CommandRegistry();
        registry.registerConcurrent("wait_state_change", request -> {
            waitStarted.countDown();
            try {
                releaseWait.await();
            } catch (InterruptedException error) {
                interruptObserved.countDown();
                releaseWait.await();
            } finally {
                handlerTerminated.countDown();
            }
            return new JsonObject();
        });

        ExecutorService clients = Executors.newFixedThreadPool(2);
        AgentHttpBridge bridge = AgentHttpBridge.start(
            "run-secret",
            0,
            registry,
            shutdownTimeout
        );
        try {
            clients.submit(() -> post(bridge.port(), """
                {"id":400,"secret":"run-secret","command":"wait_state_change","payload":{}}
                """));
            assertTrue(waitStarted.await(2, TimeUnit.SECONDS));

            Future<?> close = clients.submit(bridge::close);
            assertTrue(interruptObserved.await(2, TimeUnit.SECONDS));
            ExecutionException failure = assertThrows(
                ExecutionException.class,
                () -> close.get(2, TimeUnit.SECONDS)
            );
            assertTrue(failure.getCause() instanceof IllegalStateException);
            assertEquals(
                "HTTP bridge executor did not terminate within " + shutdownTimeout,
                failure.getCause().getMessage()
            );

            releaseWait.countDown();
            assertTrue(handlerTerminated.await(2, TimeUnit.SECONDS));
        } finally {
            releaseWait.countDown();
            assertTrue(handlerTerminated.await(2, TimeUnit.SECONDS));
            bridge.close();
            clients.shutdownNow();
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
