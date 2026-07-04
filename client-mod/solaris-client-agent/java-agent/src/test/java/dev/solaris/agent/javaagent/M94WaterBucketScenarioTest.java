package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;
import org.junit.jupiter.api.Test;

import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class M94WaterBucketScenarioTest {
    @Test
    void runsFocusedWaterBucketPlaceAndPickupScenarioThroughClientActions() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new M94WaterBucketScenario().run(
            "m94-02c-water-bucket-place-pickup",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "find-dry-placeable:within-survival-reach",
            "give:minecraft:water_bucket:1:0",
            "use:minecraft:water_bucket:place-clicked",
            "wait-block:place-target:minecraft:water",
            "held:minecraft:bucket:1",
            "use:minecraft:bucket:place-target",
            "nofluid:place-target",
            "held:minecraft:water_bucket:1"
        ), client.operations);
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("water placement: passed")),
            "scenario report must name the accepted water placement result"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("water pickup: passed")),
            "scenario report must name the water pickup result"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("degraded: lava")),
            "scenario report must keep broad fluid paths degraded"
        );
    }

    @Test
    void blocksWhenNoFluidTargetIsLoaded() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.placeablePair = null;

        ClientScenarioReport report = new M94WaterBucketScenario().run(
            "m94-02c-water-bucket-place-pickup",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("blocked", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("placeable")),
            "blocked report must explain that no placeable target was found"
        );
    }

    @Test
    void blocksUnknownScenarioIds() {
        ClientScenarioReport report = new M94WaterBucketScenario().run(
            "m94-unknown",
            Path.of("run/screenshots"),
            new FakeScenarioClient()
        );

        assertEquals("blocked", report.result());
        assertTrue(report.observations().get(0).contains("unsupported scenario"));
    }

    private static final class FakeScenarioClient implements ScenarioClient {
        final List<String> operations = new ArrayList<>();
        ScenarioBlockPair placeablePair = new ScenarioBlockPair(
            new ScenarioBlockTarget(0, 64, 0, "east", "place-clicked", "minecraft:grass_block"),
            new ScenarioBlockTarget(1, 64, 0, "west", "place-target", "minecraft:air")
        );
        ScenarioHeldItem selected = new ScenarioHeldItem("minecraft:air", 0);

        @Override
        public ScenarioBlockPair findOccupiedPair(ScenarioReach reach) {
            throw new UnsupportedOperationException("not used by water-bucket scenario");
        }

        @Override
        public ScenarioBlockPair findPlaceablePair(ScenarioReach reach) {
            throw new UnsupportedOperationException("water-bucket scenario must request a dry target");
        }

        @Override
        public ScenarioBlockPair findDryPlaceablePair(ScenarioReach reach) {
            operations.add("find-dry-placeable:" + reach.label());
            return placeablePair;
        }

        @Override
        public ScenarioHeldItem giveAndSelect(String itemId, int count, int hotbarSlot, Duration timeout) {
            operations.add("give:" + itemId + ":" + count + ":" + hotbarSlot);
            selected = new ScenarioHeldItem(itemId, count);
            return selected;
        }

        @Override
        public ScenarioUseResult useItemOn(ScenarioBlockTarget clicked, ScenarioHeldItem heldItem) {
            operations.add("use:" + heldItem.itemId() + ":" + clicked.label());
            selected = switch (heldItem.itemId()) {
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
            throw new UnsupportedOperationException("not used by water-bucket scenario");
        }

        @Override
        public boolean waitForNoFluid(ScenarioBlockTarget target, Duration duration) {
            operations.add("nofluid:" + target.label());
            return true;
        }

        @Override
        public ScenarioBreakResult breakBlock(
            ScenarioBlockTarget target,
            String expectedDropItemId,
            int expectedSelectedCount,
            Duration timeout
        ) {
            throw new UnsupportedOperationException("not used by water-bucket scenario");
        }

        @Override
        public ScenarioHeldItem selectedItem() {
            operations.add("held:" + selected.itemId() + ":" + selected.count());
            return selected;
        }
    }
}
