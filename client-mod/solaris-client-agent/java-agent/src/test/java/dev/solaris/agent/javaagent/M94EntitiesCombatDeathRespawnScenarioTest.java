package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;
import org.junit.jupiter.api.Test;

import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class M94EntitiesCombatDeathRespawnScenarioTest {
    @Test
    void runsVisibleEntityAndDeathRespawnProbeThenBlocksRemainingBroadSubrows() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new M94EntitiesCombatDeathRespawnScenario().run(
            "m94-05-entities-combat-death-respawn",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("blocked", report.result());
        assertEquals(List.of(
            "summon:minecraft:cow:0.0:0.0:4.0",
            "command:debug survival damage 10000",
            "deathScreen",
            "respawn"
        ), client.operations);
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("visible entity: passed")),
            "scenario must record the real visible entity probe"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("death/respawn: passed")),
            "scenario must record the real death and respawn probe"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("blocked: hostile combat")),
            "scenario must keep broad combat/entity/death subrows blocked"
        );
    }

    @Test
    void failsWhenVisibleEntityObservationIsNotNearTheSummonTarget() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.entityDistanceSquared = 30_006.0;

        ClientScenarioReport report = new M94EntitiesCombatDeathRespawnScenario().run(
            "m94-05-entities-combat-death-respawn",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("visible entity: failed")),
            "scenario must not accept an unrelated old entity far from the player"
        );
    }

    @Test
    void blocksUnknownScenarioIds() {
        ClientScenarioReport report = new M94EntitiesCombatDeathRespawnScenario().run(
            "m94-unknown",
            Path.of("run/screenshots"),
            new FakeScenarioClient()
        );

        assertEquals("blocked", report.result());
        assertTrue(report.observations().get(0).contains("unsupported scenario"));
    }

    private static final class FakeScenarioClient implements ScenarioClient {
        final List<String> operations = new ArrayList<>();
        double entityDistanceSquared = 16.0;

        @Override
        public ScenarioEntityObservation summonEntityNearPlayer(
            String entityTypeId,
            double offsetX,
            double offsetY,
            double offsetZ,
            Duration timeout
        ) {
            operations.add("summon:" + entityTypeId + ":" + offsetX + ":" + offsetY + ":" + offsetZ);
            return new ScenarioEntityObservation(
                entityTypeId,
                42,
                new java.util.UUID(0L, 42L),
                1.5,
                64.0,
                4.5,
                entityDistanceSquared,
                null
            );
        }

        @Override
        public void sendCommand(String command) {
            operations.add("command:" + command);
        }

        @Override
        public boolean waitForDeathScreen(Duration duration) {
            operations.add("deathScreen");
            return true;
        }

        @Override
        public boolean performRespawn(Duration duration) {
            operations.add("respawn");
            return true;
        }

        @Override
        public ScenarioBlockPair findOccupiedPair(ScenarioReach reach) {
            throw new UnsupportedOperationException("not used by broad m94-05 scenario");
        }

        @Override
        public ScenarioBlockPair findPlaceablePair(ScenarioReach reach) {
            throw new UnsupportedOperationException("not used by broad m94-05 scenario");
        }

        @Override
        public ScenarioBlockPair findDryPlaceablePair(ScenarioReach reach) {
            throw new UnsupportedOperationException("not used by broad m94-05 scenario");
        }

        @Override
        public ScenarioHeldItem giveAndSelect(String itemId, int count, int hotbarSlot, Duration timeout) {
            throw new UnsupportedOperationException("not used by broad m94-05 scenario");
        }

        @Override
        public ScenarioUseResult useItemOn(ScenarioBlockTarget clicked, ScenarioHeldItem heldItem) {
            throw new UnsupportedOperationException("not used by broad m94-05 scenario");
        }

        @Override
        public boolean waitForBlock(ScenarioBlockTarget target, String blockId, Duration duration) {
            throw new UnsupportedOperationException("not used by broad m94-05 scenario");
        }

        @Override
        public boolean waitForStableBlocks(ScenarioBlockPair pair, Duration duration) {
            throw new UnsupportedOperationException("not used by broad m94-05 scenario");
        }

        @Override
        public boolean waitForNoFluid(ScenarioBlockTarget target, Duration duration) {
            throw new UnsupportedOperationException("not used by broad m94-05 scenario");
        }

        @Override
        public ScenarioBreakResult breakBlock(
            ScenarioBlockTarget target,
            String expectedDropItemId,
            int expectedSelectedCount,
            Duration timeout
        ) {
            throw new UnsupportedOperationException("not used by broad m94-05 scenario");
        }

        @Override
        public ScenarioHeldItem selectedItem() {
            throw new UnsupportedOperationException("not used by broad m94-05 scenario");
        }
    }
}
