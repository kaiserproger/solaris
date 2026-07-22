package dev.solaris.agent.client;

import com.google.gson.JsonObject;

import java.nio.file.Path;
import java.time.Duration;
import java.util.List;
import java.util.UUID;

public interface ClientFacade {
    ClientSnapshot snapshot();

    long stateVersion();

    boolean awaitStateChange(long observedVersion, Duration timeout) throws InterruptedException;

    JsonObject observe();

    JsonObject readBlock(int x, int y, int z);

    JsonObject waitForLoadedBlock(int x, int y, int z, Duration timeout) throws Exception;

    JsonObject scanBlocks(
        int minX,
        int minY,
        int minZ,
        int maxX,
        int maxY,
        int maxZ,
        int maxBlocks
    );

    JsonObject listEntities(double radius, int limit);

    JsonObject readRecipeBook(int limit);

    JsonObject waitForVisibleEntity(String entityType, double radius, Duration timeout) throws Exception;

    JsonObject waitForEntityMotion(
        int entityId,
        UUID entityUuid,
        String entityType,
        double minimumHorizontalDistance,
        double minimumVerticalRise,
        Duration timeout
    ) throws Exception;

    JsonObject waitForEntityRemoved(
        int entityId,
        UUID entityUuid,
        String entityType,
        Duration timeout
    ) throws Exception;

    JsonObject waitForHealthBelow(double health, Duration timeout) throws Exception;

    JsonObject waitForInventoryCount(String itemId, int count, Duration timeout) throws Exception;

    JsonObject waitForContainerSlot(
        int slot,
        String itemId,
        int count,
        Duration timeout
    ) throws Exception;

    JsonObject waitForVisibleItem(String itemId, int x, int y, int z, Duration timeout) throws Exception;

    JsonObject waitForNoVisibleItem(String itemId, int x, int y, int z, Duration timeout) throws Exception;

    void connect(String host, int port);

    void selectHotbarSlot(int slot);

    JsonObject selectHotbarItem(String itemId, int count, Duration timeout) throws Exception;

    JsonObject navigateToBlock(int x, int y, int z, Duration timeout) throws Exception;

    JsonObject approachEntity(int entityId, Duration timeout) throws Exception;

    JsonObject interactEntity(int entityId, UUID entityUuid, String entityType, String hand) throws Exception;

    JsonObject attackEntityOnce(int entityId, UUID entityUuid, String entityType) throws Exception;

    JsonObject attackEntityUntilDropCollected(
        int entityId,
        String expectedDropItemId,
        int expectedDropCount,
        Duration timeout
    ) throws Exception;

    void lookAtBlock(int x, int y, int z, String face);

    JsonObject useItemOn(int x, int y, int z, String face, String hand);

    default JsonObject breakBlock(
        int x,
        int y,
        int z,
        String face,
        String expectedDropItemId,
        int expectedDropCount,
        Duration timeout
    ) throws Exception {
        throw new UnsupportedOperationException("block breaking is not available");
    }

    void moveForward(int ticks) throws Exception;

    void moveBackward(int ticks) throws Exception;

    void pressInputs(List<String> inputs, int ticks) throws Exception;

    void waitTicks(int ticks) throws Exception;

    void moveByCentimeters(int dxCm, int dzCm);

    void look(int yawDeg, int pitchDeg);

    void closeCurrentScreen() throws Exception;

    void openInventory() throws Exception;

    void respawn(Duration timeout) throws Exception;

    JsonObject quickMoveContainerSlot(int slot, Duration timeout) throws Exception;

    JsonObject clickContainerSlot(int slot, String button, Duration timeout) throws Exception;

    JsonObject clickContainerButton(int buttonId, Duration timeout) throws Exception;

    void sendChat(String message, boolean command);

    JsonObject dropSelectedItem(String itemId, int count, Duration timeout) throws Exception;

    Path screenshot(Path path) throws Exception;

    ClientScenarioReport runScenario(String id, Path screenshotsDir);

    void disconnect();
}
