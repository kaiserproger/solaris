package dev.solaris.agent.javaagent;

import net.minecraft.client.Minecraft;
import net.minecraft.world.phys.Vec3;

final class MovementDetour {
    private static final double PROBE_DISTANCE = 0.8;

    private MovementDetour() {
    }

    static int choose(
        int currentDirection,
        int preferredDirection,
        boolean horizontalCollision,
        boolean directClear,
        boolean leftClear,
        boolean rightClear
    ) {
        if (currentDirection != 0) {
            if (directClear) {
                return 0;
            }
            if (currentDirection < 0 && leftClear) {
                return -1;
            }
            if (currentDirection > 0 && rightClear) {
                return 1;
            }
            if (currentDirection < 0 && rightClear) {
                return 1;
            }
            if (currentDirection > 0 && leftClear) {
                return -1;
            }
            return currentDirection;
        }
        if (!horizontalCollision) {
            return 0;
        }
        int preferred = preferredDirection < 0 ? -1 : 1;
        if ((preferred < 0 && leftClear) || (preferred > 0 && rightClear)) {
            return preferred;
        }
        if ((preferred < 0 && rightClear) || (preferred > 0 && leftClear)) {
            return -preferred;
        }
        return preferred;
    }

    static MovementClearance clearance(Minecraft minecraft, Vec3 target) {
        Vec3 offset = target.subtract(minecraft.player.position());
        Vec3 forward = new Vec3(offset.x, 0.0, offset.z);
        double distance = forward.length();
        if (distance < 1.0E-6) {
            return new MovementClearance(true, true, true, false);
        }
        forward = forward.scale(1.0 / distance);
        Vec3 left = new Vec3(forward.z, 0.0, -forward.x);
        Vec3 right = left.scale(-1.0);
        double forwardDistance = Math.min(distance, PROBE_DISTANCE);
        boolean directClear = probeClear(minecraft, forward, forwardDistance);
        return new MovementClearance(
            directClear,
            probeClear(minecraft, left, PROBE_DISTANCE),
            probeClear(minecraft, right, PROBE_DISTANCE),
            !directClear && raisedForwardClear(minecraft, forward, forwardDistance)
        );
    }

    private static boolean probeClear(Minecraft minecraft, Vec3 direction, double distance) {
        return minecraft.level.noCollision(
            minecraft.player,
            minecraft.player.getBoundingBox()
                .expandTowards(direction.x * distance, 0.0, direction.z * distance)
                .deflate(0.01)
        );
    }

    private static boolean raisedForwardClear(Minecraft minecraft, Vec3 direction, double distance) {
        return minecraft.level.noCollision(
            minecraft.player,
            minecraft.player.getBoundingBox()
                .move(0.0, 1.0, 0.0)
                .expandTowards(direction.x * distance, 0.0, direction.z * distance)
                .deflate(0.01)
        );
    }
}

record MovementClearance(boolean direct, boolean left, boolean right, boolean raisedForward) {
}
