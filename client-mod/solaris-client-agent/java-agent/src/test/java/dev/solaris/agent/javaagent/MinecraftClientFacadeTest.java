package dev.solaris.agent.javaagent;

import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.TimeoutException;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class MinecraftClientFacadeTest {
    @Test
    void blockObservationIncludesClientSkyAndBlockLight() throws Exception {
        String source = Files.readString(Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftClientObservation.java"
        ));
        String method = source.substring(
            source.indexOf("static JsonObject readBlock("),
            source.indexOf("static JsonObject scanBlocks(")
        );

        assertEquals(true, method.contains("LightLayer.SKY"));
        assertEquals(true, method.contains("LightLayer.BLOCK"));
        assertEquals(true, method.contains("\"sky_light\""));
        assertEquals(true, method.contains("\"block_light\""));
    }

    @Test
    void playerObservationIncludesExperienceState() throws Exception {
        String source = Files.readString(Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftClientObservation.java"
        ));
        String method = source.substring(
            source.indexOf("private static JsonObject player("),
            source.indexOf("private static JsonObject entity(")
        );

        assertEquals(true, method.contains("\"experience_level\""));
        assertEquals(true, method.contains("\"experience_progress\""));
        assertEquals(true, method.contains("\"total_experience\""));
    }

    @Test
    void itemObservationIncludesExactEnchantments() throws Exception {
        String source = Files.readString(Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftClientObservation.java"
        ));
        String method = source.substring(
            source.indexOf("private static JsonObject item("),
            source.indexOf("private static JsonObject screen(")
        );

        assertEquals(true, method.contains("getEnchantments().entrySet()"));
        assertEquals(true, method.contains("getRegisteredName()"));
        assertEquals(true, method.contains("\"enchantments\""));
        assertEquals(true, method.contains("\"level\""));
    }

    @Test
    void respawnForwardsTimeoutAndRejectsUnconfirmedRespawn() throws Exception {
        Duration timeout = Duration.ofSeconds(7);
        RespawnScenarioClient confirmed = new RespawnScenarioClient(true);

        MinecraftClientFacade.respawn(confirmed, timeout);

        assertSame(timeout, confirmed.respawnTimeout);

        RespawnScenarioClient unconfirmed = new RespawnScenarioClient(false);
        assertThrows(
            TimeoutException.class,
            () -> MinecraftClientFacade.respawn(unconfirmed, timeout)
        );
        assertSame(timeout, unconfirmed.respawnTimeout);
    }

    @Test
    void interactEntityReturnsTheObservedVanillaResultForTheCanonicalTarget() throws Exception {
        RespawnScenarioClient client = new RespawnScenarioClient(true);
        ScenarioEntityIdentity identity = new ScenarioEntityIdentity(
            42,
            java.util.UUID.fromString("01234567-89ab-cdef-0123-456789abcdef"),
            "minecraft:cow"
        );
        client.entityInteractionResult = new ScenarioEntityInteractionResult(
            "pass",
            false,
            8.25,
            65.5,
            -3.75
        );

        var result = MinecraftClientFacade.interactEntity(
            client,
            new ScenarioEntityInteraction(identity, "off_hand")
        );

        assertTrue(result.get("dispatched").getAsBoolean());
        assertEquals("pass", result.get("result").getAsString());
        assertEquals(42, result.get("entity_id").getAsInt());
        assertEquals(identity.entityUuid().toString(), result.get("entity_uuid").getAsString());
        assertEquals("minecraft:cow", result.get("entity_type").getAsString());
        assertEquals("off_hand", result.get("hand").getAsString());
        assertEquals(8.25, result.get("hit_x").getAsDouble());
        assertEquals(65.5, result.get("hit_y").getAsDouble());
        assertEquals(-3.75, result.get("hit_z").getAsDouble());
        assertEquals(identity, client.entityInteraction.identity());
        assertEquals("off_hand", client.entityInteraction.hand());
    }

    @Test
    void entityIdentityJsonUsesCanonicalNamesWithoutAliases() {
        ScenarioEntityIdentity identity = new ScenarioEntityIdentity(
            42,
            java.util.UUID.fromString("01234567-89ab-cdef-0123-456789abcdef"),
            "minecraft:cow"
        );

        var result = MinecraftClientObservation.entityIdentity(identity);

        assertEquals(42, result.get("entity_id").getAsInt());
        assertEquals(identity.entityUuid().toString(), result.get("entity_uuid").getAsString());
        assertEquals("minecraft:cow", result.get("entity_type").getAsString());
        assertEquals(false, result.has("id"));
        assertEquals(false, result.has("uuid"));
        assertEquals(false, result.has("type"));
    }

    @Test
    void genericScenarioInteractionUsesTheDiscoveredObservationIdentity() throws Exception {
        RespawnScenarioClient client = new RespawnScenarioClient(true);
        client.entityInteractionResult = new ScenarioEntityInteractionResult("pass", false, 1.0, 2.0, 3.0);
        ScenarioEntityObservation discovered = new ScenarioEntityObservation(
            "minecraft:cow",
            42,
            java.util.UUID.fromString("01234567-89ab-cdef-0123-456789abcdef"),
            8.0,
            64.0,
            -3.0,
            9.0,
            null
        );

        client.interactEntity(discovered, "main_hand");

        assertEquals(discovered.identity(), client.entityInteraction.identity());
        assertEquals("main_hand", client.entityInteraction.hand());
    }

    @Test
    void screenshotBaseDirectoryLetsVanillaWriteRequestedScreenshotsPath() {
        assertEquals(
            Path.of("run"),
            MinecraftClientFacade.screenshotBaseDirectory(Path.of("run/screenshots/m94-02b.png"))
        );
        assertEquals(
            Path.of("."),
            MinecraftClientFacade.screenshotBaseDirectory(Path.of("screenshots/m94-02b.png"))
        );
    }

    @Test
    void screenshotBaseDirectoryRejectsPathsOutsideScreenshotsDirectory() {
        assertThrows(
            IllegalArgumentException.class,
            () -> MinecraftClientFacade.screenshotBaseDirectory(Path.of("run/m94-02b.png"))
        );
        assertThrows(
            IllegalArgumentException.class,
            () -> MinecraftClientFacade.screenshotBaseDirectory(Path.of("m94-02b.png"))
        );
    }

    @Test
    void disconnectClosesNetworkBeforeClearingClientState() {
        List<String> calls = new ArrayList<>();

        DisconnectSequence.run(
            () -> calls.add("network"),
            () -> calls.add("client")
        );

        assertEquals(List.of("network", "client"), calls);
    }

    private static final class RespawnScenarioClient implements ScenarioClient {
        private final boolean respawned;
        private Duration respawnTimeout;
        private ScenarioEntityInteraction entityInteraction;
        private ScenarioEntityInteractionResult entityInteractionResult;

        private RespawnScenarioClient(boolean respawned) {
            this.respawned = respawned;
        }

        @Override
        public boolean performRespawn(Duration timeout) {
            respawnTimeout = timeout;
            return respawned;
        }

        @Override
        public ScenarioEntityInteractionResult interactEntity(ScenarioEntityInteraction interaction) {
            entityInteraction = interaction;
            return entityInteractionResult;
        }

        @Override
        public ScenarioBlockPair findOccupiedPair(ScenarioReach reach) {
            throw new UnsupportedOperationException("not used");
        }

        @Override
        public ScenarioBlockPair findPlaceablePair(ScenarioReach reach) {
            throw new UnsupportedOperationException("not used");
        }

        @Override
        public ScenarioBlockPair findDryPlaceablePair(ScenarioReach reach) {
            throw new UnsupportedOperationException("not used");
        }

        @Override
        public ScenarioHeldItem giveAndSelect(String itemId, int count, int hotbarSlot, Duration timeout) {
            throw new UnsupportedOperationException("not used");
        }

        @Override
        public ScenarioUseResult useItemOn(ScenarioBlockTarget clicked, ScenarioHeldItem heldItem) {
            throw new UnsupportedOperationException("not used");
        }

        @Override
        public boolean waitForBlock(ScenarioBlockTarget target, String blockId, Duration duration) {
            throw new UnsupportedOperationException("not used");
        }

        @Override
        public boolean waitForStableBlocks(ScenarioBlockPair pair, Duration duration) {
            throw new UnsupportedOperationException("not used");
        }

        @Override
        public boolean waitForNoFluid(ScenarioBlockTarget target, Duration duration) {
            throw new UnsupportedOperationException("not used");
        }

        @Override
        public ScenarioBreakResult breakBlock(
            ScenarioBlockTarget target,
            String expectedDropItemId,
            int expectedSelectedCount,
            Duration timeout
        ) {
            throw new UnsupportedOperationException("not used");
        }

        @Override
        public ScenarioHeldItem selectedItem() {
            throw new UnsupportedOperationException("not used");
        }
    }
}
