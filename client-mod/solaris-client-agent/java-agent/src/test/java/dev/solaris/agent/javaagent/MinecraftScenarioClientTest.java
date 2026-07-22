package dev.solaris.agent.javaagent;

import org.junit.jupiter.api.Test;

import java.time.Duration;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class MinecraftScenarioClientTest {
    @Test
    void directMovementDoesNotDetourWithoutCollision() {
        assertEquals(
            0,
            MovementDetour.choose(0, 1, false, true, true, true)
        );
    }

    @Test
    void collisionChoosesThePreferredOpenSide() {
        assertEquals(
            -1,
            MovementDetour.choose(0, -1, true, false, true, true)
        );
        assertEquals(
            -1,
            MovementDetour.choose(0, 1, true, false, true, false)
        );
    }

    @Test
    void detourPersistsUntilTheDirectPathIsClear() {
        assertEquals(
            1,
            MovementDetour.choose(1, -1, true, false, true, true)
        );
        assertEquals(
            -1,
            MovementDetour.choose(1, -1, true, false, true, false)
        );
        assertEquals(
            0,
            MovementDetour.choose(1, -1, false, true, true, true)
        );
    }

    @Test
    void blockNavigationAcceptsOnlyObservedGroundedCollisionFreeArrival() {
        assertTrue(BlockNavigation.arrived(10.5, 64.0, -2.5, 10, 64, -3, true, true));
        assertFalse(BlockNavigation.arrived(10.5, 64.0, -2.5, 10, 64, -3, false, true));
        assertFalse(BlockNavigation.arrived(10.5, 64.0, -2.5, 10, 64, -3, true, false));
        assertFalse(BlockNavigation.arrived(12.1, 64.0, -2.5, 10, 64, -3, true, true));
    }

    @Test
    void blockNavigationRejectsTargetsOutsideItsBoundedRoute() {
        assertTrue(BlockNavigation.withinBounds(0.0, 64.0, 0.0, 47, 64, 0));
        assertFalse(BlockNavigation.withinBounds(0.0, 64.0, 0.0, 49, 64, 0));
        assertFalse(BlockNavigation.withinBounds(0.0, 64.0, 0.0, 0, 73, 0));
    }

    @Test
    void blockNavigationDeclaresACompletelyBlockedRouteUnreachable() {
        assertTrue(BlockNavigation.unreachable(new MovementClearance(false, false, false, false)));
        assertFalse(BlockNavigation.unreachable(new MovementClearance(false, true, false, false)));
        assertFalse(BlockNavigation.unreachable(new MovementClearance(false, false, false, true)));
    }

    @Test
    void placementRejectsNeighbourBlockIntersectingPlayerHitbox() {
        assertTrue(BlockPlacementClearance.intersects(
            26.78, 76.0, 8.61,
            27.38, 77.8, 9.21,
            26, 76, 8
        ));
        assertFalse(BlockPlacementClearance.intersects(
            26.78, 76.0, 8.61,
            27.38, 77.8, 9.21,
            25, 76, 8
        ));
    }

    @Test
    void fullBlockPlacementAcceptsConstrainedButUnobstructedSpace() {
        assertTrue(BlockPlacementClearance.allowsFullBlockPlacement(true, true));
    }

    @Test
    void fullBlockPlacementRejectsEntityObstruction() {
        assertFalse(BlockPlacementClearance.allowsFullBlockPlacement(true, false));
    }

    @Test
    void blockPickupUsesTotalInventoryDeltaInsteadOfTheSelectedStack() {
        assertTrue(MinecraftScenarioClient.pickupCountReached(0, 1, 1));
        assertTrue(MinecraftScenarioClient.pickupCountReached(7, 8, 1));
        assertTrue(MinecraftScenarioClient.pickupCountReached(7, 9, 1));
        assertFalse(MinecraftScenarioClient.pickupCountReached(7, 7, 1));
        assertFalse(MinecraftScenarioClient.pickupCountReached(7, 8, 2));
    }

    @Test
    void blockPickupAcceptsAnInventoryEventAppliedBeforeItsFirstSample() throws Exception {
        InventoryPickupProbe probe = new InventoryPickupProbe(
            new MinecraftScenarioClient.InventoryPickupSample(8, false)
        );

        MinecraftScenarioClient.InventoryPickupResult result =
            MinecraftScenarioClient.waitForInventoryPickup(probe, 7, 1, Long.MAX_VALUE);

        assertTrue(result.confirmed());
        assertFalse(result.sawDrop());
        assertEquals(0, probe.awaitCalls);
    }

    @Test
    void blockPickupKeepsDropEvidenceAcrossUnrelatedAppliedEvents() throws Exception {
        InventoryPickupProbe probe = new InventoryPickupProbe(
            new MinecraftScenarioClient.InventoryPickupSample(7, false),
            new MinecraftScenarioClient.InventoryPickupSample(7, true),
            new MinecraftScenarioClient.InventoryPickupSample(8, false)
        );

        MinecraftScenarioClient.InventoryPickupResult result =
            MinecraftScenarioClient.waitForInventoryPickup(probe, 7, 1, Long.MAX_VALUE);

        assertTrue(result.confirmed());
        assertTrue(result.sawDrop());
        assertEquals(2, probe.awaitCalls);
        assertEquals(3, probe.sampleCalls);
    }

    @Test
    void blockPickupTimeoutIsFailure() throws Exception {
        InventoryPickupProbe probe = new InventoryPickupProbe(
            new MinecraftScenarioClient.InventoryPickupSample(7, true)
        );

        MinecraftScenarioClient.InventoryPickupResult result =
            MinecraftScenarioClient.waitForInventoryPickup(probe, 7, 1, 0L);

        assertFalse(result.confirmed());
        assertTrue(result.sawDrop());
        assertEquals(1, probe.awaitCalls);
        assertEquals(1, probe.sampleCalls);
    }

    @Test
    void experienceWaitUsesAppliedPacketEvents() throws Exception {
        String source = java.nio.file.Files.readString(java.nio.file.Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftScenarioClient.java"
        ));
        String method = source.substring(
            source.indexOf("public int waitForTotalExperienceAbove("),
            source.indexOf("public boolean waitForDayTimeAtOrAfter(")
        );

        assertTrue(method.contains("ClientStateEvents.version()"));
        assertTrue(method.contains("awaitClientStateChange(observedVersion, deadlineNanos)"));
    }

    @Test
    void entityIdentityFenceRejectsEntityIdReuse() {
        UUID expectedUuid = UUID.fromString("01234567-89ab-cdef-0123-456789abcdef");
        ScenarioEntityIdentity identity = new ScenarioEntityIdentity(
            42,
            expectedUuid,
            "minecraft:cow"
        );

        assertTrue(identity.matches(
            42,
            expectedUuid,
            "minecraft:cow"
        ));
        assertFalse(identity.matches(
            42,
            UUID.fromString("fedcba98-7654-3210-fedc-ba9876543210"),
            "minecraft:cow"
        ));
        assertFalse(identity.matches(
            42,
            expectedUuid,
            "minecraft:sheep"
        ));
        assertFalse(identity.matches(
            43,
            expectedUuid,
            "minecraft:cow"
        ));
    }

    @Test
    void entityMotionWaitWakesOnlyAfterAnAppliedStateNotification() throws Exception {
        EntityWaitProbe probe = new EntityWaitProbe(motion(0.0), true);
        CompletableFuture<ScenarioEntityMotionObservation> result = CompletableFuture.supplyAsync(() -> {
            try {
                return MinecraftScenarioClient.waitForEntityMotion(
                    probe,
                    42,
                    Double.NaN,
                    Double.NaN,
                    Double.NaN,
                    false,
                    0.5,
                    0.0,
                    Duration.ofSeconds(1)
                );
            } catch (Exception error) {
                throw new IllegalStateException(error);
            }
        });

        assertTrue(probe.awaitEntered.await(1, TimeUnit.SECONDS));
        assertEquals(1, probe.snapshotCalls.get());
        probe.motion = motion(0.5);
        probe.publishChange();

        ScenarioEntityMotionObservation observation = result.get(1, TimeUnit.SECONDS);
        assertEquals(0.5, observation.horizontalDistance());
        assertEquals(2, probe.snapshotCalls.get());
        assertEquals(1, probe.awaitCalls.get());
    }

    @Test
    void entityMotionWaitFailsClosedWhenTheClientLevelIsReplaced() throws Exception {
        EntityWaitProbe probe = new EntityWaitProbe(motion(0.0), true);
        CompletableFuture<ScenarioEntityMotionObservation> result = CompletableFuture.supplyAsync(() -> {
            try {
                return MinecraftScenarioClient.waitForEntityMotion(
                    probe,
                    42,
                    Double.NaN,
                    Double.NaN,
                    Double.NaN,
                    false,
                    0.5,
                    0.0,
                    Duration.ofSeconds(1)
                );
            } catch (Exception error) {
                throw new IllegalStateException(error);
            }
        });

        assertTrue(probe.awaitEntered.await(1, TimeUnit.SECONDS));
        probe.level = new Object();
        probe.publishChange();

        Exception failure = org.junit.jupiter.api.Assertions.assertThrows(
            Exception.class,
            () -> result.get(1, TimeUnit.SECONDS)
        );
        assertTrue(rootCause(failure) instanceof IllegalStateException);
        assertTrue(rootCause(failure).getMessage().contains("client level changed"));
    }

    @Test
    void entityRemovalWaitDoesNotTreatAWorldTransitionAsRemoval() throws Exception {
        EntityWaitProbe probe = new EntityWaitProbe(motion(0.0), true);
        CompletableFuture<Boolean> result = CompletableFuture.supplyAsync(() -> {
            try {
                return MinecraftScenarioClient.waitForEntityRemoved(
                    probe,
                    Duration.ofSeconds(1)
                );
            } catch (Exception error) {
                throw new IllegalStateException(error);
            }
        });

        assertTrue(probe.awaitEntered.await(1, TimeUnit.SECONDS));
        probe.level = new Object();
        probe.present = false;
        probe.publishChange();

        Exception failure = org.junit.jupiter.api.Assertions.assertThrows(
            Exception.class,
            () -> result.get(1, TimeUnit.SECONDS)
        );
        assertTrue(rootCause(failure) instanceof IllegalStateException);
        assertTrue(rootCause(failure).getMessage().contains("client level changed"));
    }

    @Test
    void entityRemovalWaitWakesOnTheExactAppliedStateNotification() throws Exception {
        EntityWaitProbe probe = new EntityWaitProbe(motion(0.0), true);
        CompletableFuture<Boolean> result = CompletableFuture.supplyAsync(() -> {
            try {
                return MinecraftScenarioClient.waitForEntityRemoved(
                    probe,
                    Duration.ofSeconds(1)
                );
            } catch (Exception error) {
                throw new IllegalStateException(error);
            }
        });

        assertTrue(probe.awaitEntered.await(1, TimeUnit.SECONDS));
        assertEquals(1, probe.snapshotCalls.get());
        probe.motion = null;
        probe.present = false;
        probe.publishChange();

        assertTrue(result.get(1, TimeUnit.SECONDS));
        assertEquals(2, probe.snapshotCalls.get());
        assertEquals(1, probe.awaitCalls.get());
    }

    @Test
    void entityWaitTimeoutIsFailureAndNeverAReadinessSignal() throws Exception {
        EntityWaitProbe motionProbe = new EntityWaitProbe(motion(0.0), true);
        motionProbe.timeout = true;
        ScenarioEntityMotionObservation motion = MinecraftScenarioClient.waitForEntityMotion(
            motionProbe,
            42,
            Double.NaN,
            Double.NaN,
            Double.NaN,
            false,
            0.5,
            0.0,
            Duration.ofSeconds(1)
        );
        assertTrue(motion.horizontalDistance() < 0.5);

        EntityWaitProbe removalProbe = new EntityWaitProbe(motion(0.0), true);
        removalProbe.timeout = true;
        assertFalse(MinecraftScenarioClient.waitForEntityRemoved(
            removalProbe,
            Duration.ofSeconds(1)
        ));
        assertEquals(1, removalProbe.snapshotCalls.get());
    }

    @Test
    void entityAttackWaitsForVanillaCooldown() {
        assertFalse(AttackCadence.ready(0.89F));
        assertTrue(AttackCadence.ready(0.90F));
        assertTrue(AttackCadence.ready(1.0F));
    }

    @Test
    void entityReachPartitionsAtOneExactSquaredBoundary() {
        double boundary = 20.25;
        double immediatelyOutside = Math.nextUp(boundary);

        assertTrue(ScenarioReach.WITHIN_SURVIVAL_REACH.includes(boundary));
        assertFalse(ScenarioReach.OUTSIDE_SURVIVAL_REACH.includes(boundary));
        assertFalse(ScenarioReach.WITHIN_SURVIVAL_REACH.includes(immediatelyOutside));
        assertTrue(ScenarioReach.OUTSIDE_SURVIVAL_REACH.includes(immediatelyOutside));
    }

    @Test
    void mapsAllVanillaDyeColorsToWoolItems() throws Exception {
        Map<String, String> expected = Map.ofEntries(
            Map.entry("WHITE", "minecraft:white_wool"),
            Map.entry("ORANGE", "minecraft:orange_wool"),
            Map.entry("MAGENTA", "minecraft:magenta_wool"),
            Map.entry("LIGHT_BLUE", "minecraft:light_blue_wool"),
            Map.entry("YELLOW", "minecraft:yellow_wool"),
            Map.entry("LIME", "minecraft:lime_wool"),
            Map.entry("PINK", "minecraft:pink_wool"),
            Map.entry("GRAY", "minecraft:gray_wool"),
            Map.entry("LIGHT_GRAY", "minecraft:light_gray_wool"),
            Map.entry("CYAN", "minecraft:cyan_wool"),
            Map.entry("PURPLE", "minecraft:purple_wool"),
            Map.entry("BLUE", "minecraft:blue_wool"),
            Map.entry("BROWN", "minecraft:brown_wool"),
            Map.entry("GREEN", "minecraft:green_wool"),
            Map.entry("RED", "minecraft:red_wool"),
            Map.entry("BLACK", "minecraft:black_wool")
        );
        assertEquals(16, expected.size());
        for (Map.Entry<String, String> mapping : expected.entrySet()) {
            assertEquals(mapping.getValue(), SheepWoolColor.itemId(mapping.getKey()), mapping.getKey());
        }
    }

    private static MinecraftScenarioClient.EntityMotionSample motion(double x) {
        return new MinecraftScenarioClient.EntityMotionSample(
            "minecraft:cow",
            x,
            64.0,
            0.0,
            x,
            0.0
        );
    }

    private static Throwable rootCause(Throwable failure) {
        Throwable current = failure;
        while (current.getCause() != null) {
            current = current.getCause();
        }
        return current;
    }

    private static final class InventoryPickupProbe
        implements MinecraftScenarioClient.InventoryPickupWaitSource {
        private final MinecraftScenarioClient.InventoryPickupSample[] samples;
        private int index;
        private int sampleCalls;
        private int awaitCalls;

        private InventoryPickupProbe(MinecraftScenarioClient.InventoryPickupSample... samples) {
            this.samples = samples;
        }

        @Override
        public long stateVersion() {
            return index;
        }

        @Override
        public MinecraftScenarioClient.InventoryPickupSample sample() {
            sampleCalls += 1;
            return samples[index];
        }

        @Override
        public boolean awaitStateChange(long observedVersion, long deadlineNanos) {
            awaitCalls += 1;
            if (index + 1 >= samples.length) {
                return false;
            }
            index += 1;
            return true;
        }
    }

    private static final class EntityWaitProbe implements MinecraftScenarioClient.EntityWaitSource {
        private final Object initialLevel = new Object();
        private final CountDownLatch awaitEntered = new CountDownLatch(1);
        private final CountDownLatch changed = new CountDownLatch(1);
        private final AtomicInteger snapshotCalls = new AtomicInteger();
        private final AtomicInteger awaitCalls = new AtomicInteger();
        private volatile Object level = initialLevel;
        private volatile MinecraftScenarioClient.EntityMotionSample motion;
        private volatile boolean present;
        private volatile boolean timeout;
        private volatile long version;

        private EntityWaitProbe(MinecraftScenarioClient.EntityMotionSample motion, boolean present) {
            this.motion = motion;
            this.present = present;
        }

        @Override
        public Object captureLevel() {
            return level;
        }

        @Override
        public MinecraftScenarioClient.EntityStateSnapshot snapshot() {
            snapshotCalls.incrementAndGet();
            return new MinecraftScenarioClient.EntityStateSnapshot(level, motion, present);
        }

        @Override
        public long stateVersion() {
            return version;
        }

        @Override
        public boolean awaitStateChange(long observedVersion, long deadlineNanos)
            throws InterruptedException {
            awaitCalls.incrementAndGet();
            awaitEntered.countDown();
            if (timeout) {
                return false;
            }
            if (!changed.await(1, TimeUnit.SECONDS)) {
                throw new AssertionError("test state event was not published");
            }
            return version != observedVersion;
        }

        private void publishChange() {
            version += 1;
            changed.countDown();
        }
    }
}
