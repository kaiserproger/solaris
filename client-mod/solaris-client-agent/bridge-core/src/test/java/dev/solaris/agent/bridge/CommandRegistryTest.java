package dev.solaris.agent.bridge;

import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class CommandRegistryTest {
    @Test
    void serializesExecutionAcrossAllTransportsSharingTheRegistry() throws Exception {
        CommandRegistry registry = new CommandRegistry();
        CountDownLatch firstEntered = new CountDownLatch(1);
        CountDownLatch secondStarted = new CountDownLatch(1);
        CountDownLatch release = new CountDownLatch(1);
        AtomicInteger active = new AtomicInteger();
        AtomicInteger maximumActive = new AtomicInteger();
        registry.register("hold", request -> {
            int current = active.incrementAndGet();
            maximumActive.accumulateAndGet(current, Math::max);
            firstEntered.countDown();
            try {
                assertTrue(release.await(2, TimeUnit.SECONDS));
                return new JsonObject();
            } finally {
                active.decrementAndGet();
            }
        });

        BridgeCommand command = registry.find("hold").orElseThrow();
        BridgeRequest request = new BridgeRequest(1, "", "hold", new JsonObject());
        try (var executor = Executors.newVirtualThreadPerTaskExecutor()) {
            Future<JsonObject> first = executor.submit(() -> command.execute(request));
            assertTrue(firstEntered.await(2, TimeUnit.SECONDS));
            Future<JsonObject> second = executor.submit(() -> {
                secondStarted.countDown();
                return command.execute(request);
            });
            assertTrue(secondStarted.await(2, TimeUnit.SECONDS));

            assertEquals(1, maximumActive.get());
            release.countDown();
            first.get(2, TimeUnit.SECONDS);
            second.get(2, TimeUnit.SECONDS);
        }
    }
}
