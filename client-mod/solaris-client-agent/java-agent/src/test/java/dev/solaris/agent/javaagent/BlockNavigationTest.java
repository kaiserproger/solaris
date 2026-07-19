package dev.solaris.agent.javaagent;

import org.junit.jupiter.api.Test;

import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class BlockNavigationTest {
    private static final BlockNavigation.Route ROUTE = new BlockNavigation.Route(
        0.5, 64.0, 0.5,
        8, 64, 0
    );

    @Test
    void observedCollisionWithRaisedForwardClearanceDrivesOneBlockStep() throws Exception {
        FakeRuntime runtime = new FakeRuntime(List.of(
            moving(0.5, 64.0, 0.5, false, blocked(false)),
            moving(1.2, 64.0, 0.5, true, blocked(true)),
            moving(1.8, 65.0, 0.5, false, clear()),
            arrived(8.5, 64.0, 0.5)
        ));

        BlockNavigation.Result result = BlockNavigation.run(ROUTE, Duration.ofSeconds(1), runtime);

        assertEquals(BlockNavigation.Terminal.ARRIVED, result.terminal());
        assertEquals(3, runtime.awaitedVersions.size());
        assertEquals(List.of(0L, 1L, 2L), runtime.awaitedVersions);
        assertTrue(runtime.inputs.get(0).forward());
        assertFalse(runtime.inputs.get(0).jump());
        assertTrue(runtime.inputs.get(1).forward());
        assertTrue(runtime.inputs.get(1).jump());
        assertFalse(runtime.inputs.get(2).jump());
        assertTrue(runtime.cleaned);
        assertFalse(runtime.anyKeyDown());
    }

    @Test
    void detourLeavingRouteCorridorIsObservedUnreachable() throws Exception {
        FakeRuntime runtime = new FakeRuntime(List.of(
            moving(2.0, 64.0, 0.5, true, new MovementClearance(false, true, false, false)),
            moving(3.0, 64.0, 4.0, false, clear())
        ));

        BlockNavigation.Result result = BlockNavigation.run(ROUTE, Duration.ofSeconds(1), runtime);

        assertEquals(BlockNavigation.Terminal.UNREACHABLE, result.terminal());
        assertEquals(List.of(0L), runtime.awaitedVersions);
        assertTrue(runtime.cleaned);
        assertFalse(runtime.anyKeyDown());
    }

    @Test
    void blockedAndInvalidObservationsAreTerminalAndCleanKeys() throws Exception {
        FakeRuntime blocked = new FakeRuntime(List.of(
            moving(1.0, 64.0, 0.5, true, blocked(false))
        ));
        FakeRuntime invalid = new FakeRuntime(List.of(
            moving(Double.NaN, 64.0, 0.5, false, clear())
        ));

        assertEquals(
            BlockNavigation.Terminal.UNREACHABLE,
            BlockNavigation.run(ROUTE, Duration.ofSeconds(1), blocked).terminal()
        );
        assertEquals(
            BlockNavigation.Terminal.INVALID_OBSERVATION,
            BlockNavigation.run(ROUTE, Duration.ofSeconds(1), invalid).terminal()
        );
        assertTrue(blocked.cleaned);
        assertTrue(invalid.cleaned);
        assertFalse(blocked.anyKeyDown());
        assertFalse(invalid.anyKeyDown());
    }

    @Test
    void unloadedTargetObservationIsInvalidAndCleansKeys() throws Exception {
        BlockNavigation.Observation unloaded = new BlockNavigation.Observation(
            1.0, 64.0, 0.5,
            true, true, false, false, clear()
        );
        FakeRuntime runtime = new FakeRuntime(List.of(unloaded));

        BlockNavigation.Result result = BlockNavigation.run(ROUTE, Duration.ofSeconds(1), runtime);

        assertEquals(BlockNavigation.Terminal.TARGET_UNLOADED, result.terminal());
        assertTrue(runtime.cleaned);
        assertFalse(runtime.anyKeyDown());
    }

    @Test
    void timeoutOnlyFailsAnUnfinishedWaitAndCleansKeys() throws Exception {
        FakeRuntime runtime = new FakeRuntime(List.of(
            moving(1.0, 64.0, 0.5, false, clear())
        ));
        runtime.deliverTickEvent = false;

        BlockNavigation.Result result = BlockNavigation.run(ROUTE, Duration.ofSeconds(1), runtime);

        assertEquals(BlockNavigation.Terminal.TIMED_OUT, result.terminal());
        assertEquals(List.of(0L), runtime.awaitedVersions);
        assertTrue(runtime.cleaned);
        assertFalse(runtime.anyKeyDown());
    }

    @Test
    void arrivalCleansKeysWithoutWaitingForAnotherTick() throws Exception {
        FakeRuntime runtime = new FakeRuntime(List.of(arrived(8.5, 64.0, 0.5)));

        BlockNavigation.Result result = BlockNavigation.run(ROUTE, Duration.ofSeconds(1), runtime);

        assertEquals(BlockNavigation.Terminal.ARRIVED, result.terminal());
        assertTrue(runtime.awaitedVersions.isEmpty());
        assertTrue(runtime.cleaned);
        assertFalse(runtime.anyKeyDown());
    }

    private static BlockNavigation.Observation moving(
        double x,
        double y,
        double z,
        boolean horizontalCollision,
        MovementClearance clearance
    ) {
        return new BlockNavigation.Observation(
            x, y, z,
            true, true, true, horizontalCollision, clearance
        );
    }

    private static BlockNavigation.Observation arrived(double x, double y, double z) {
        return moving(x, y, z, false, clear());
    }

    private static MovementClearance clear() {
        return new MovementClearance(true, true, true, false);
    }

    private static MovementClearance blocked(boolean raisedForwardClear) {
        return new MovementClearance(false, false, false, raisedForwardClear);
    }

    private static final class FakeRuntime implements BlockNavigation.Runtime {
        private final List<BlockNavigation.Observation> observations;
        private final List<BlockNavigation.Inputs> inputs = new ArrayList<>();
        private final List<Long> awaitedVersions = new ArrayList<>();
        private int observationIndex;
        private long tickVersion;
        private boolean deliverTickEvent = true;
        private boolean cleaned;
        private BlockNavigation.Inputs currentInputs = new BlockNavigation.Inputs(
            false, false, false, false, false
        );

        private FakeRuntime(List<BlockNavigation.Observation> observations) {
            this.observations = observations;
        }

        @Override
        public long nanoTime() {
            return 0L;
        }

        @Override
        public long tickVersion() {
            return tickVersion;
        }

        @Override
        public BlockNavigation.Observation observe() {
            return observations.get(observationIndex);
        }

        @Override
        public void apply(BlockNavigation.Inputs nextInputs) {
            inputs.add(nextInputs);
            currentInputs = nextInputs;
        }

        @Override
        public boolean awaitTickChange(long observedVersion, Duration timeout) {
            awaitedVersions.add(observedVersion);
            if (!deliverTickEvent) {
                return false;
            }
            tickVersion++;
            observationIndex++;
            return true;
        }

        @Override
        public void clearInputs() {
            cleaned = true;
            currentInputs = new BlockNavigation.Inputs(false, false, false, false, false);
        }

        private boolean anyKeyDown() {
            return currentInputs.sprint()
                || currentInputs.forward()
                || currentInputs.jump()
                || currentInputs.left()
                || currentInputs.right();
        }
    }
}
