package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;
import org.junit.jupiter.api.Test;

import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class M94M40M41RouteScenarioTest {
    @Test
    void runsWaterDropAndEntitySubprobesThenBlocksManualMetricsRows() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new M94M40M41RouteScenario().run(
            "m94-07-m40-m41-route-with-metrics",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("blocked", report.result());
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
            report.observations().stream().anyMatch(entry -> entry.contains("water subprobe result: passed")),
            "route must report the water subprobe result"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("solid/drop subprobe result: passed")),
            "route must report the drop/pickup subprobe result"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("visible entity: passed")),
            "route must report the entity visibility result"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("blocked: swim feel")),
            "route must keep unautomated swim feel degraded"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("TPS/lock")),
            "route must name missing performance and lock evidence"
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

    private static final class FakeScenarioClient implements ScenarioClient {
        final List<String> operations = new ArrayList<>();
        int findPlaceableCalls;
        ScenarioHeldItem selected = new ScenarioHeldItem("minecraft:air", 0);

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
