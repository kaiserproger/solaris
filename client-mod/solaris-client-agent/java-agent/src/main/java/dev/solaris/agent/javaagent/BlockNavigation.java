package dev.solaris.agent.javaagent;

import java.time.Duration;

final class BlockNavigation {
    static final double MAX_HORIZONTAL_DISTANCE = 48.0;
    static final double MAX_VERTICAL_DISTANCE = 8.0;
    static final double CORRIDOR_HORIZONTAL_MARGIN = 3.0;
    static final double CORRIDOR_VERTICAL_MARGIN = 2.0;
    private static final double ARRIVAL_HORIZONTAL_DISTANCE_SQUARED = 2.25;
    private static final double ARRIVAL_VERTICAL_DISTANCE = 1.25;

    private BlockNavigation() {
    }

    static Result run(Route route, Duration timeout, Runtime runtime) throws Exception {
        long deadlineNanos = runtime.nanoTime() + timeout.toNanos();
        int detourDirection = 0;
        int preferredDetourDirection = 1;
        Observation latest = null;
        try {
            while (true) {
                long observedTickVersion = runtime.tickVersion();
                latest = runtime.observe();
                Decision decision = decide(
                    route,
                    latest,
                    detourDirection,
                    preferredDetourDirection
                );
                if (decision.terminal() != null) {
                    return new Result(decision.terminal(), latest);
                }
                runtime.apply(decision.inputs());
                if (detourDirection == 0 && decision.detourDirection() != 0) {
                    preferredDetourDirection = -decision.detourDirection();
                }
                detourDirection = decision.detourDirection();

                long remainingNanos = deadlineNanos - runtime.nanoTime();
                if (remainingNanos <= 0L || !runtime.awaitTickChange(
                    observedTickVersion,
                    Duration.ofNanos(remainingNanos)
                )) {
                    return new Result(Terminal.TIMED_OUT, latest);
                }
            }
        } finally {
            runtime.clearInputs();
        }
    }

    static boolean withinBounds(
        double playerX,
        double playerY,
        double playerZ,
        int targetX,
        int targetY,
        int targetZ
    ) {
        double horizontalDistanceSquared = horizontalDistanceSquared(playerX, playerZ, targetX, targetZ);
        return horizontalDistanceSquared <= MAX_HORIZONTAL_DISTANCE * MAX_HORIZONTAL_DISTANCE
            && Math.abs(playerY - targetY) <= MAX_VERTICAL_DISTANCE;
    }

    static boolean arrived(
        double playerX,
        double playerY,
        double playerZ,
        int targetX,
        int targetY,
        int targetZ,
        boolean onGround,
        boolean collisionFree
    ) {
        return onGround
            && collisionFree
            && horizontalDistanceSquared(playerX, playerZ, targetX, targetZ)
                <= ARRIVAL_HORIZONTAL_DISTANCE_SQUARED
            && Math.abs(playerY - targetY) <= ARRIVAL_VERTICAL_DISTANCE;
    }

    static boolean unreachable(MovementClearance clearance) {
        return !clearance.direct()
            && !clearance.left()
            && !clearance.right()
            && !clearance.raisedForward();
    }

    private static Decision decide(
        Route route,
        Observation observation,
        int currentDetourDirection,
        int preferredDetourDirection
    ) {
        if (!observation.finite()) {
            return Decision.terminal(Terminal.INVALID_OBSERVATION);
        }
        if (!observation.targetLoaded()) {
            return Decision.terminal(Terminal.TARGET_UNLOADED);
        }
        if (observation.clearance() == null) {
            return Decision.terminal(Terminal.INVALID_OBSERVATION);
        }
        if (arrived(
            observation.playerX(),
            observation.playerY(),
            observation.playerZ(),
            route.targetX(),
            route.targetY(),
            route.targetZ(),
            observation.onGround(),
            observation.collisionFree()
        )) {
            return Decision.terminal(Terminal.ARRIVED);
        }
        if (!route.contains(observation.playerX(), observation.playerY(), observation.playerZ())) {
            return Decision.terminal(Terminal.UNREACHABLE);
        }

        MovementClearance clearance = observation.clearance();
        if (observation.horizontalCollision() && unreachable(clearance)) {
            return Decision.terminal(Terminal.UNREACHABLE);
        }
        boolean jump = observation.horizontalCollision() && clearance.raisedForward();
        int nextDetourDirection = MovementDetour.choose(
            currentDetourDirection,
            preferredDetourDirection,
            observation.horizontalCollision() && !jump,
            clearance.direct(),
            clearance.left(),
            clearance.right()
        );
        return new Decision(
            null,
            nextDetourDirection,
            new Inputs(
                true,
                nextDetourDirection == 0,
                jump,
                nextDetourDirection < 0,
                nextDetourDirection > 0
            )
        );
    }

    private static double horizontalDistanceSquared(
        double playerX,
        double playerZ,
        int targetX,
        int targetZ
    ) {
        double dx = playerX - (targetX + 0.5);
        double dz = playerZ - (targetZ + 0.5);
        return dx * dx + dz * dz;
    }

    enum Terminal {
        ARRIVED,
        UNREACHABLE,
        TARGET_UNLOADED,
        INVALID_OBSERVATION,
        TIMED_OUT
    }

    record Route(
        double startX,
        double startY,
        double startZ,
        int targetX,
        int targetY,
        int targetZ
    ) {
        private boolean contains(double playerX, double playerY, double playerZ) {
            double minY = Math.min(startY, targetY) - CORRIDOR_VERTICAL_MARGIN;
            double maxY = Math.max(startY, targetY) + CORRIDOR_VERTICAL_MARGIN;
            return playerY >= minY
                && playerY <= maxY
                && distanceToHorizontalSegmentSquared(playerX, playerZ)
                    <= CORRIDOR_HORIZONTAL_MARGIN * CORRIDOR_HORIZONTAL_MARGIN;
        }

        private double distanceToHorizontalSegmentSquared(double playerX, double playerZ) {
            double endX = targetX + 0.5;
            double endZ = targetZ + 0.5;
            double routeX = endX - startX;
            double routeZ = endZ - startZ;
            double routeLengthSquared = routeX * routeX + routeZ * routeZ;
            if (routeLengthSquared < 1.0E-12) {
                double dx = playerX - startX;
                double dz = playerZ - startZ;
                return dx * dx + dz * dz;
            }
            double projection = ((playerX - startX) * routeX + (playerZ - startZ) * routeZ)
                / routeLengthSquared;
            double clamped = Math.max(0.0, Math.min(1.0, projection));
            double closestX = startX + routeX * clamped;
            double closestZ = startZ + routeZ * clamped;
            double dx = playerX - closestX;
            double dz = playerZ - closestZ;
            return dx * dx + dz * dz;
        }
    }

    record Observation(
        double playerX,
        double playerY,
        double playerZ,
        boolean onGround,
        boolean collisionFree,
        boolean targetLoaded,
        boolean horizontalCollision,
        MovementClearance clearance
    ) {
        private boolean finite() {
            return Double.isFinite(playerX)
                && Double.isFinite(playerY)
                && Double.isFinite(playerZ);
        }
    }

    record Inputs(boolean sprint, boolean forward, boolean jump, boolean left, boolean right) {
    }

    record Result(Terminal terminal, Observation observation) {
    }

    interface Runtime {
        long nanoTime();

        long tickVersion();

        Observation observe() throws Exception;

        void apply(Inputs inputs) throws Exception;

        boolean awaitTickChange(long observedVersion, Duration timeout) throws InterruptedException;

        void clearInputs() throws Exception;
    }

    private record Decision(Terminal terminal, int detourDirection, Inputs inputs) {
        private static Decision terminal(Terminal terminal) {
            return new Decision(terminal, 0, null);
        }
    }
}
