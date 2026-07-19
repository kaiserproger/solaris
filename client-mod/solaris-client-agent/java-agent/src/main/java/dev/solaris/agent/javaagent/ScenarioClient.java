package dev.solaris.agent.javaagent;

import java.time.Duration;
import java.util.List;

interface ScenarioClient {
    static boolean authoritativeContainerUpdateMatches(
        int expectedContainerId,
        int initialStateId,
        int currentContainerId,
        int currentStateId,
        boolean slotMatches
    ) {
        return currentContainerId == expectedContainerId
            && currentStateId != initialStateId
            && slotMatches;
    }

    ScenarioBlockPair findOccupiedPair(ScenarioReach reach) throws Exception;

    ScenarioBlockPair findPlaceablePair(ScenarioReach reach) throws Exception;

    ScenarioBlockPair findDryPlaceablePair(ScenarioReach reach) throws Exception;

    default ScenarioBlockPair findTillableSoil(ScenarioReach reach) throws Exception {
        return findDryPlaceablePair(reach);
    }

    default ScenarioBlockPair findOpenDryPlaceablePair(ScenarioReach reach) throws Exception {
        return findDryPlaceablePair(reach);
    }

    default ScenarioBlockPair findUnobstructedPlaceablePair(ScenarioReach reach) throws Exception {
        return findPlaceablePair(reach);
    }

    ScenarioHeldItem giveAndSelect(String itemId, int count, int hotbarSlot, Duration timeout) throws Exception;

    ScenarioUseResult useItemOn(ScenarioBlockTarget clicked, ScenarioHeldItem heldItem) throws Exception;

    boolean waitForBlock(ScenarioBlockTarget target, String blockId, Duration duration) throws Exception;

    default boolean waitForAnyBlock(ScenarioBlockTarget target, List<String> blockIds, Duration duration)
        throws Exception {
        throw new UnsupportedOperationException("scenario client does not support multi-block waits");
    }

    default boolean waitForBlockProperty(
        ScenarioBlockTarget target,
        String property,
        String value,
        Duration duration
    ) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support block-property waits");
    }

    default ScenarioLightLevel lightLevel(ScenarioBlockTarget target) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support client light reads");
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

    default int totalExperience() throws Exception {
        throw new UnsupportedOperationException("scenario client does not support experience reads");
    }

    default int experienceLevel() throws Exception {
        throw new UnsupportedOperationException("scenario client does not support experience level reads");
    }

    default boolean waitForExperience(int totalExperience, int level, Duration duration) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support exact experience waits");
    }

    default int waitForTotalExperienceAbove(int totalExperience, Duration duration) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support experience waits");
    }

    default boolean waitForDayTimeAtOrAfter(long dayTime, Duration duration) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support world time waits");
    }

    default boolean waitForDayTimeBelow(long dayTime, Duration duration) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support world time waits");
    }

    default boolean waitForScreenClassName(String className, Duration duration) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support screen waits");
    }

    default boolean closeCurrentScreen(Duration duration) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support screen close");
    }

    default int activeContainerId() throws Exception {
        throw new UnsupportedOperationException("scenario client does not support active container id reads");
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

    default boolean quickMoveContainerSlot(int containerSlot, Duration duration) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support direct container quick moves");
    }

    default int findContainerSlot(String itemId, int count) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support container slot scans");
    }

    default boolean clickContainerButton(int buttonId, Duration duration) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support container buttons");
    }

    default boolean containerSlotHasEnchantment(int slot, String enchantmentId, int level) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support container component reads");
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

    default void sendChatMessage(String message) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support normal chat sends");
    }

    default boolean waitForChatMessage(String expectedText, Duration timeout) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support chat message waits");
    }

    default boolean waitForTicks(long ticks, Duration timeout) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support tick events");
    }

    default long serverGameTime() throws Exception {
        throw new UnsupportedOperationException("scenario client does not support server time reads");
    }

    default long waitForServerTimeAfter(long baseline, Duration timeout) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support server time packet waits");
    }

    default ScenarioEntityObservation findVisibleEntity(
        List<String> entityTypeIds,
        ScenarioReach reach,
        Duration timeout
    ) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support natural entity scans");
    }

    default ScenarioEntityObservation findVisibleSheepWithWool(
        String woolItemId,
        ScenarioReach reach,
        Duration timeout
    ) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support colored sheep scans");
    }

    default ScenarioEntityObservation visibleEntity(List<String> entityTypeIds, ScenarioReach reach)
        throws Exception {
        throw new UnsupportedOperationException("scenario client does not support current entity scans");
    }

    default ScenarioEntityMotionObservation waitForEntityMotion(
        ScenarioEntityObservation entity,
        double minimumHorizontalDistance,
        double minimumVerticalRise,
        Duration timeout
    ) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support entity motion probes");
    }

    default ScenarioPlayerObservation waitForVisiblePlayer(String playerName, Duration timeout) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support player visibility scans");
    }

    default boolean waitForNoVisiblePlayer(String playerName, Duration timeout) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support player removal visibility scans");
    }

    default ScenarioPlayerObservation waitForMovedPlayer(
        String playerName,
        ScenarioPlayerObservation baseline,
        double minHorizontalDelta,
        Duration timeout
    ) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support player movement visibility scans");
    }

    default boolean approachEntity(ScenarioEntityObservation entity, Duration timeout) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support natural movement toward entities");
    }

    default ScenarioEntityInteractionResult interactEntity(ScenarioEntityInteraction interaction)
        throws Exception {
        throw new UnsupportedOperationException("scenario client does not support entity interactions");
    }

    default ScenarioEntityInteractionResult interactEntity(
        ScenarioEntityObservation discovered,
        String hand
    ) throws Exception {
        return interactEntity(new ScenarioEntityInteraction(discovered.identity(), hand));
    }

    default ScenarioBreakResult attackEntityUntilDropCollected(
        ScenarioEntityObservation entity,
        String expectedDropItemId,
        int expectedSelectedCount,
        Duration timeout
    ) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support entity attack/drop probes");
    }

    default boolean attackEntityUntilRemoved(ScenarioEntityObservation entity, Duration timeout)
        throws Exception {
        throw new UnsupportedOperationException("scenario client does not support entity removal attacks");
    }

    default boolean drainHungerBySprinting(Duration timeout) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support natural hunger drain");
    }

    default ScenarioFoodUseResult eatSelectedFood(String itemId, int itemCountBefore, Duration timeout)
        throws Exception {
        throw new UnsupportedOperationException("scenario client does not support food use probes");
    }

    default ScenarioShieldBlockResult blockAttackWithSelectedShield(String itemId, Duration timeout)
        throws Exception {
        throw new UnsupportedOperationException("scenario client does not support shield block probes");
    }

    default boolean quickEquipSelectedArmor(String itemId, String armorSlot, Duration duration) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support armor equip probes");
    }

    default ScenarioHeldItem equippedArmor(String armorSlot) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support equipped armor reads");
    }

    default float playerHealth() throws Exception {
        throw new UnsupportedOperationException("scenario client does not support player health reads");
    }

    default float waitForPlayerHealthBelow(float health, Duration duration) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support player health waits");
    }

    default boolean teleportTo(double x, double y, double z, Duration timeout) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support teleport setup");
    }

    default boolean waitForDeathScreen(Duration duration) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support death screen waits");
    }

    default boolean standOnBlockUntilDeath(ScenarioBlockTarget target, Duration duration) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support block hazard death probes");
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

    default List<ScenarioItemDropIdentity> visibleItemDropIdentities(String itemId) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support visible item drop identities");
    }

    default ScenarioItemDropIdentity waitForNewVisibleItemDropIdentity(
        String itemId,
        List<ScenarioItemDropIdentity> excludedIdentities,
        Duration timeout
    ) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support new item drop identity waits");
    }

    default ScenarioBreakResult collectVisibleItemDrop(
        ScenarioBlockTarget near,
        String expectedDropItemId,
        int expectedSelectedCount,
        Duration timeout
    ) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support visible item drop pickup probes");
    }

    default ScenarioBreakResult collectVisibleItemDropByIdentity(
        ScenarioBlockTarget near,
        String expectedDropItemId,
        ScenarioItemDropIdentity expectedIdentity,
        int expectedSelectedCount,
        Duration timeout
    ) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support identity-bound item drop pickup probes");
    }

    default boolean waitForNoVisibleItemDrop(String itemId, ScenarioBlockTarget near, Duration timeout)
        throws Exception {
        throw new UnsupportedOperationException("scenario client does not support visible item drop removal waits");
    }

    default ScenarioBlockTarget findBreakableBlock(List<String> blockIds, ScenarioReach reach) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support natural breakable-block scans");
    }

    default boolean approachBlock(ScenarioBlockTarget target, Duration timeout) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support natural movement toward blocks");
    }

    default boolean approachPosition(int x, int z, Duration timeout) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support natural movement toward positions");
    }

    default ScenarioBlockTarget findLoadedBlockInColumn(int x, int z, List<String> blockIds)
        throws Exception {
        throw new UnsupportedOperationException("scenario client does not support loaded column block scans");
    }

    default ScenarioHeldItem selectHotbarItem(String itemId, int count, Duration timeout) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support hotbar item selection");
    }

    default ScenarioBlockTarget dropSelectedItem(String itemId, int count, Duration timeout) throws Exception {
        throw new UnsupportedOperationException("scenario client does not support selected item drops");
    }

    ScenarioHeldItem selectedItem() throws Exception;
}
