package dev.solaris.agent.javaagent;

import java.time.Duration;
import java.util.List;

interface ScenarioClient {
    ScenarioBlockPair findOccupiedPair(ScenarioReach reach) throws Exception;

    ScenarioBlockPair findPlaceablePair(ScenarioReach reach) throws Exception;

    ScenarioBlockPair findDryPlaceablePair(ScenarioReach reach) throws Exception;

    ScenarioHeldItem giveAndSelect(String itemId, int count, int hotbarSlot, Duration timeout) throws Exception;

    ScenarioUseResult useItemOn(ScenarioBlockTarget clicked, ScenarioHeldItem heldItem) throws Exception;

    boolean waitForBlock(ScenarioBlockTarget target, String blockId, Duration duration) throws Exception;

    default boolean waitForAnyBlock(ScenarioBlockTarget target, List<String> blockIds, Duration duration)
        throws Exception {
        throw new UnsupportedOperationException("scenario client does not support multi-block waits");
    }

    boolean waitForStableBlocks(ScenarioBlockPair pair, Duration duration) throws Exception;

    boolean waitForNoFluid(ScenarioBlockTarget target, Duration duration) throws Exception;

    default boolean waitForSignEditor(ScenarioBlockTarget target, Duration duration) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support sign editor waits");
    }

    default void updateSignText(ScenarioBlockTarget target, List<String> lines) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support sign text updates");
    }

    default boolean waitForSignText(ScenarioBlockTarget target, List<String> lines, Duration duration)
        throws Exception {
        throw new UnsupportedOperationException("scenario client does not support sign text waits");
    }

    default void placeRecipe(int containerId, int recipeDisplayId, boolean useMaxItems) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support recipe placement");
    }

    default int inventoryCount(String itemId) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support inventory count reads");
    }

    default boolean waitForInventoryCount(String itemId, int count, Duration duration) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support inventory count waits");
    }

    default boolean waitForScreenClassName(String className, Duration duration) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support screen waits");
    }

    default boolean closeCurrentScreen(Duration duration) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support screen close");
    }

    default boolean moveSelectedItemToContainerSlot(
        int containerSlot,
        String itemId,
        int count,
        Duration duration
    ) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support container transfers");
    }

    default boolean waitForContainerSlot(
        int containerSlot,
        String itemId,
        int count,
        Duration duration
    ) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support container slot waits");
    }

    default boolean moveContainerSlotToInventory(
        int containerSlot,
        String itemId,
        int count,
        Duration duration
    ) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support container slot quick moves");
    }

    default boolean waitForContainerSlotEmpty(int containerSlot, Duration duration) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support empty container slot waits");
    }

    default ScenarioEntityObservation summonEntityNearPlayer(
        String entityTypeId,
        double offsetX,
        double offsetY,
        double offsetZ,
        Duration timeout
    ) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support entity summon probes");
    }

    default void sendCommand(String command) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support chat commands");
    }

    default boolean teleportTo(double x, double y, double z, Duration timeout) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support teleport setup");
    }

    default boolean waitForDeathScreen(Duration duration) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support death screen waits");
    }

    default boolean performRespawn(Duration duration) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support respawn");
    }

    ScenarioBreakResult breakBlock(
        ScenarioBlockTarget target,
        String expectedDropItemId,
        int expectedSelectedCount,
        Duration timeout
    ) throws Exception;

    default ScenarioBreakResult breakBlockUntilDropVisible(
        ScenarioBlockTarget target,
        String expectedDropItemId,
        Duration timeout
    ) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support visible drop break probes");
    }

    default boolean waitForVisibleItemDrop(String itemId, ScenarioBlockTarget near, Duration timeout)
        throws Exception {
        throw new UnsupportedOperationException("scenario client does not support visible item drop waits");
    }

    default ScenarioBreakResult collectVisibleItemDrop(
        ScenarioBlockTarget near,
        String expectedDropItemId,
        int expectedSelectedCount,
        Duration timeout
    ) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support visible item drop pickup probes");
    }

    default boolean waitForNoVisibleItemDrop(String itemId, ScenarioBlockTarget near, Duration timeout)
        throws Exception {
        throw new UnsupportedOperationException("scenario client does not support visible item drop removal waits");
    }

    ScenarioHeldItem selectedItem() throws Exception;
}
