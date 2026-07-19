package dev.solaris.agent.javaagent;

final class BlockPlacementClearance {
    private BlockPlacementClearance() {
    }

    static boolean intersects(
        double minX,
        double minY,
        double minZ,
        double maxX,
        double maxY,
        double maxZ,
        int blockX,
        int blockY,
        int blockZ
    ) {
        return maxX > blockX && minX < blockX + 1.0
            && maxY > blockY && minY < blockY + 1.0
            && maxZ > blockZ && minZ < blockZ + 1.0;
    }

    static boolean allowsFullBlockPlacement(boolean canSurvive, boolean unobstructed) {
        return canSurvive && unobstructed;
    }
}
