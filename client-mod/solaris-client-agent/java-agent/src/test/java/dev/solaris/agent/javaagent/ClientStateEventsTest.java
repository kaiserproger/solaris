package dev.solaris.agent.javaagent;

import org.junit.jupiter.api.Test;

import java.time.Duration;
import java.util.UUID;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class ClientStateEventsTest {
    @Test
    void itemTakeEventsAreCollectorBoundBoundedAndClearable() {
        ClientStateEvents.clearItemTakenEvents();
        ScenarioItemDropIdentity first = new ScenarioItemDropIdentity(1, new UUID(0, 1));
        ClientStateEvents.publishItemTaken(first, 7);

        assertFalse(ClientStateEvents.consumeItemTakenBy(first, 8));
        assertTrue(ClientStateEvents.consumeItemTakenBy(first, 7));

        for (int index = 0; index < 65; index += 1) {
            ClientStateEvents.publishItemTaken(
                new ScenarioItemDropIdentity(100 + index, new UUID(1, index)),
                7
            );
        }
        ScenarioItemDropIdentity evicted = new ScenarioItemDropIdentity(100, new UUID(1, 0));
        ScenarioItemDropIdentity retained = new ScenarioItemDropIdentity(164, new UUID(1, 64));
        assertFalse(ClientStateEvents.consumeItemTakenBy(evicted, 7));
        assertTrue(ClientStateEvents.consumeItemTakenBy(retained, 7));

        ClientStateEvents.clearItemTakenEvents();
        ScenarioItemDropIdentity cleared = new ScenarioItemDropIdentity(163, new UUID(1, 63));
        assertFalse(ClientStateEvents.consumeItemTakenBy(cleared, 7));
    }

    @Test
    void publishedStateEventWakesStateWaiter() throws Exception {
        long observedVersion = ClientStateEvents.version();
        CompletableFuture<Boolean> waiter = CompletableFuture.supplyAsync(() -> {
            try {
                return ClientStateEvents.awaitChange(observedVersion, Duration.ofSeconds(1));
            } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
                throw new IllegalStateException(error);
            }
        });

        ClientStateEvents.publishState();

        assertTrue(waiter.get(1, TimeUnit.SECONDS));
    }

    @Test
    void tickEventDoesNotWakeStateWaiter() throws Exception {
        long observedStateVersion = ClientStateEvents.version();
        CompletableFuture<Boolean> stateWaiter = CompletableFuture.supplyAsync(() -> {
            try {
                return ClientStateEvents.awaitChange(observedStateVersion, Duration.ofSeconds(1));
            } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
                throw new IllegalStateException(error);
            }
        });

        ClientStateEvents.publishTick();

        assertEquals(observedStateVersion, ClientStateEvents.version());
        assertFalse(stateWaiter.isDone());
        ClientStateEvents.publishState();
        assertTrue(stateWaiter.get(1, TimeUnit.SECONDS));
    }

    @Test
    void tickEventWakesTickWaiter() throws Exception {
        long observedTickVersion = ClientStateEvents.tickVersion();
        CompletableFuture<Boolean> tickWaiter = CompletableFuture.supplyAsync(() -> {
            try {
                return ClientStateEvents.awaitTickChange(
                    observedTickVersion,
                    Duration.ofSeconds(1)
                );
            } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
                throw new IllegalStateException(error);
            }
        });

        ClientStateEvents.publishTick();

        assertTrue(tickWaiter.get(1, TimeUnit.SECONDS));
    }

    @Test
    void serverTimePacketWakesOnlyTheServerTimeWaiter() throws Exception {
        long observedVersion = ClientStateEvents.serverTimeVersion();
        CompletableFuture<Boolean> waiter = CompletableFuture.supplyAsync(() -> {
            try {
                return ClientStateEvents.awaitServerTimeChange(observedVersion, Duration.ofSeconds(1));
            } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
                throw new IllegalStateException(error);
            }
        });

        ClientStateEvents.publishServerTime(42L);

        assertTrue(waiter.get(1, TimeUnit.SECONDS));
        assertEquals(42L, ClientStateEvents.serverGameTime());
    }
}
