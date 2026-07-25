package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;
import org.junit.jupiter.api.Test;

import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class M94M40M41RouteScenarioTest {
    @Test
    void focusedDeepWaterRouteProvesInputPoseFluidEyeAirAndConnection() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new M94M40M41RouteScenario().run(
            M94M40M41RouteScenario.DEEP_WATER_ID,
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertTrue(client.operations.contains("ticks:120"));
        assertTrue(client.operations.contains("command:debug water-corridor 4 96 0"));
        assertTrue(client.operations.contains("wait-block:deep-water-fixture-bottom:minecraft:water"));
        assertTrue(client.operations.contains("wait-block:deep-water-fixture-top:minecraft:water"));
        assertTrue(client.operations.contains("chat:Debug water corridor at 4 96 0 verified 68/68 block states"));
        assertTrue(client.operations.contains("inputs:jump:8"));
        assertTrue(client.operations.contains("inputs:sneak:8"));
        assertTrue(client.operations.contains("inputs:sprint+forward:30"));
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("ascent delta_y=0.7000"))
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("dive delta_y=-0.3000"))
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("swim horizontal_delta=3.4000"))
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("deep-water checks: passed"))
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("air_loss=true"))
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("air_recovery=true"))
        );
    }

    @Test
    void focusedDeepWaterRouteFailsWhenAscentIsCorrectedBack() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.waterObservations.set(1, water(
            0.5, 62.05, 1024.5, 1.62, 1.8,
            true, true, false, 1.83, 296, 300, 20.0F, "standing", true
        ));

        ClientScenarioReport report = new M94M40M41RouteScenario().run(
            M94M40M41RouteScenario.DEEP_WATER_ID,
            Path.of("run/screenshots"),
            client
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("retained water ascent"))
        );
        assertFalse(
            report.observations().stream().anyMatch(entry -> entry.contains("deep-water checks: passed"))
        );
    }

    @Test
    void broadRouteRunsDeepWaterDropAndEntityThenBlocksOnlyRemainingRows() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new M94M40M41RouteScenario().run(
            M94M40M41RouteScenario.ID,
            Path.of("run/screenshots"),
            client
        );

        assertEquals("blocked", report.result());
        assertTrue(
            client.operations.contains("inputs:sprint+forward:30"),
            "route must exercise the real deep-water sprint-swim input path"
        );
        assertTrue(
            client.operations.contains("use:minecraft:water_bucket:water-clicked"),
            "route must exercise the real water-bucket placement path"
        );
        assertTrue(
            client.operations.contains("break:solid-break-clicked:minecraft:dirt:1"),
            "route must exercise the real visible drop and pickup path"
        );
        assertTrue(
            client.operations.contains("summon:minecraft:cow:0.0:0.0:4.0"),
            "route must exercise a real client-visible mob observation"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("deep-water subprobe result: passed")),
            "route must report the deep-water subprobe result"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("water-bucket subprobe result: passed")),
            "route must report the water-bucket subprobe result"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("solid/drop subprobe result: passed")),
            "route must report the drop/pickup subprobe result"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("visible entity: passed")),
            "route must report the entity visibility result"
        );
        assertFalse(
            report.observations().stream().anyMatch(entry -> entry.contains("blocked: swim feel")),
            "B4 must no longer remain in the broad route blocker text"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("sugar cane"))
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("TPS/lock"))
        );
    }

    @Test
    void blocksUnknownScenarioIds() {
        ClientScenarioReport report = new M94M40M41RouteScenario().run(
            "m94-unknown",
            Path.of("run/screenshots"),
            new FakeScenarioClient()
        );

        assertEquals("blocked", report.result());
        assertTrue(report.observations().get(0).contains("unsupported scenario"));
    }

    private static ScenarioWaterObservation water(
        double x,
        double y,
        double z,
        double eyeHeight,
        double bodyHeight,
        boolean inWater,
        boolean underWater,
        boolean swimming,
        double fluidHeight,
        int air,
        int maxAir,
        float health,
        String pose,
        boolean connected
    ) {
        return new ScenarioWaterObservation(
            x,
            y,
            z,
            y + eyeHeight,
            eyeHeight,
            bodyHeight,
            inWater,
            underWater,
            swimming,
            fluidHeight,
            inWater ? "minecraft:water" : "minecraft:air",
            inWater ? "minecraft:water" : "minecraft:empty",
            inWater,
            inWater ? Math.max(fluidHeight, 1.0) : 0.0,
            underWater ? "minecraft:water" : "minecraft:air",
            underWater ? "minecraft:water" : "minecraft:empty",
            underWater,
            underWater ? 1.0 : 0.0,
            air,
            maxAir,
            health,
            pose,
            connected
        );
    }

    private static final class FakeScenarioClient implements ScenarioClient {
        final List<String> operations = new ArrayList<>();
        final List<ScenarioWaterObservation> waterObservations = new ArrayList<>(List.of(
            water(0.5, 62.0, 1024.5, 1.62, 1.8, true, true, false, 1.88, 300, 300, 20.0F, "standing", true),
            water(0.5, 62.7, 1024.5, 1.62, 1.8, true, false, false, 1.18, 296, 300, 20.0F, "standing", true),
            water(0.5, 63.45, 1024.5, 1.62, 1.8, true, false, false, 0.55, 295, 300, 20.0F, "standing", true),
            water(0.5, 63.15, 1024.5, 1.62, 1.8, true, true, false, 0.85, 292, 300, 20.0F, "standing", true),
            water(0.5, 62.0, 1024.5, 1.62, 1.8, true, true, false, 1.88, 290, 300, 20.0F, "standing", true),
            water(0.5, 62.0, 1027.9, 0.4, 0.6, true, true, true, 1.0, 270, 300, 20.0F, "swimming", true),
            water(0.5, 62.0, 1027.9, 0.4, 0.6, true, true, true, 1.0, 230, 300, 20.0F, "swimming", true),
            water(0.5, 64.25, 1024.5, 1.62, 1.8, false, false, false, 0.0, 270, 300, 20.0F, "standing", true)
        ));
        int waterObservationIndex;
        int findPlaceableCalls;
        ScenarioHeldItem selected = new ScenarioHeldItem("minecraft:air", 0);

        @Override
        public void sendCommand(String command) {
            operations.add("command:" + command);
        }

        @Override
        public ScenarioWaterObservation waterObservation() {
            operations.add("water-observation:" + waterObservationIndex);
            return waterObservations.get(waterObservationIndex++);
        }

        @Override
        public void setView(float yaw, float pitch) {
            operations.add("view:" + yaw + ":" + pitch);
        }

        @Override
        public void pressInputs(List<String> inputs, int ticks, Duration timeout) {
            operations.add("inputs:" + String.join("+", inputs) + ":" + ticks);
        }

        @Override
        public boolean teleportTo(double x, double y, double z, Duration timeout) {
            operations.add("teleport:" + x + ":" + y + ":" + z);
            return true;
        }

        @Override
        public boolean waitForChatMessage(String expectedText, Duration timeout) {
            operations.add("chat:" + expectedText);
            return true;
        }

        @Override
        public boolean waitForTicks(long ticks, Duration timeout) {
            operations.add("ticks:" + ticks);
            return true;
        }

        @Override
        public ScenarioBlockPair findOccupiedPair(ScenarioReach reach) {
            throw new UnsupportedOperationException("not used by M40/M41 route scenario");
        }

        @Override
        public ScenarioBlockPair findPlaceablePair(ScenarioReach reach) {
            operations.add("find-placeable:" + reach.label());
            findPlaceableCalls += 1;
            if (findPlaceableCalls == 1) {
                return new ScenarioBlockPair(
                    new ScenarioBlockTarget(0, 64, 0, "east", "solid-break-clicked", "minecraft:grass_block"),
                    new ScenarioBlockTarget(1, 64, 0, "west", "solid-break-target", "minecraft:air")
                );
            }
            return new ScenarioBlockPair(
                new ScenarioBlockTarget(2, 64, 0, "east", "solid-place-clicked", "minecraft:grass_block"),
                new ScenarioBlockTarget(3, 64, 0, "west", "solid-place-target", "minecraft:air")
            );
        }

        @Override
        public ScenarioBlockPair findDryPlaceablePair(ScenarioReach reach) {
            operations.add("find-dry-placeable:" + reach.label());
            return new ScenarioBlockPair(
                new ScenarioBlockTarget(4, 64, 0, "east", "water-clicked", "minecraft:grass_block"),
                new ScenarioBlockTarget(5, 64, 0, "west", "water-target", "minecraft:air")
            );
        }

        @Override
        public ScenarioHeldItem giveAndSelect(String itemId, int count, int hotbarSlot, Duration timeout) {
            operations.add("give:" + itemId + ":" + count + ":" + hotbarSlot);
            selected = count <= 0
                ? new ScenarioHeldItem("minecraft:air", 0)
                : new ScenarioHeldItem(itemId, count);
            return selected;
        }

        @Override
        public ScenarioUseResult useItemOn(ScenarioBlockTarget clicked, ScenarioHeldItem heldItem) {
            operations.add("use:" + heldItem.itemId() + ":" + clicked.label());
            selected = switch (heldItem.itemId()) {
                case "minecraft:dirt" -> new ScenarioHeldItem("minecraft:air", 0);
                case "minecraft:water_bucket" -> new ScenarioHeldItem("minecraft:bucket", 1);
                case "minecraft:bucket" -> new ScenarioHeldItem("minecraft:water_bucket", 1);
                default -> heldItem;
            };
            return new ScenarioUseResult("success");
        }

        @Override
        public boolean waitForBlock(ScenarioBlockTarget target, String blockId, Duration duration) {
            operations.add("wait-block:" + target.label() + ":" + blockId);
            return true;
        }

        @Override
        public boolean waitForStableBlocks(ScenarioBlockPair pair, Duration duration) {
            throw new UnsupportedOperationException("not used by M40/M41 route scenario");
        }

        @Override
        public boolean waitForNoFluid(ScenarioBlockTarget target, Duration duration) {
            operations.add("nofluid:" + target.label());
            return true;
        }

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
                16.0,
                null
            );
        }

        @Override
        public ScenarioBreakResult breakBlock(
            ScenarioBlockTarget target,
            String expectedDropItemId,
            int expectedSelectedCount,
            Duration timeout
        ) {
            operations.add("break:" + target.label() + ":" + expectedDropItemId + ":" + expectedSelectedCount);
            selected = new ScenarioHeldItem(expectedDropItemId, expectedSelectedCount);
            return new ScenarioBreakResult(true, true, true, true, selected);
        }

        @Override
        public ScenarioHeldItem selectedItem() {
            operations.add("held:" + selected.itemId() + ":" + selected.count());
            return selected;
        }
    }
}
