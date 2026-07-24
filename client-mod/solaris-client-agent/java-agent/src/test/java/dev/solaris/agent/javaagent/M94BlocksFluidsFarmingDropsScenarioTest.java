package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;
import org.junit.jupiter.api.Test;

import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.assertFalse;

final class M94BlocksFluidsFarmingDropsScenarioTest {
    @Test
    void runsSolidAndWaterSubprobesThenBlocksRemainingBroadSubrows() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new M94BlocksFluidsFarmingDropsScenario().run(
            "m94-02-blocks-fluids-farming-drops",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("blocked", report.result());
        assertTrue(
            client.operations.contains("break:solid-break-clicked:minecraft:dirt:1"),
            "broad scenario must run the real solid break/drop subprobe"
        );
        assertTrue(
            client.operations.contains("use:minecraft:water_bucket:water-clicked"),
            "broad scenario must run the real water-bucket placement subprobe"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("solid subprobe result: passed")),
            "broad scenario must report the solid subprobe result"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("water subprobe result: passed")),
            "broad scenario must report the water subprobe result"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("blocked: door/trapdoor")),
            "broad scenario must name the remaining broad-row blockers"
        );
    }

    @Test
    void runsOnlySolidSubprobeWhenSolidScenarioIdRequested() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new M94BlocksFluidsFarmingDropsScenario().run(
            M94SolidBlockScenario.ID,
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertTrue(
            client.operations.contains("break:solid-break-clicked:minecraft:dirt:1"),
            "targeted scenario id should execute the solid subprobe"
        );
        assertFalse(
            client.operations.stream().anyMatch(operation -> operation.startsWith("find-dry-placeable")),
            "solid phase should not execute dry placeable/fluid probe"
        );
    }

    @Test
    void runsOnlyWaterSubprobeWhenWaterScenarioIdRequested() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new M94BlocksFluidsFarmingDropsScenario().run(
            M94WaterBucketScenario.ID,
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertTrue(
            client.operations.contains("find-dry-placeable:within-survival-reach"),
            "water phase should execute the dry placeable/fluid probe"
        );
        assertFalse(
            client.operations.stream().anyMatch(operation -> operation.startsWith("break:solid-break")),
            "water phase should not execute block-break probe"
        );
    }

    @Test
    void blocksUnknownScenarioIds() {
        ClientScenarioReport report = new M94BlocksFluidsFarmingDropsScenario().run(
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
            throw new UnsupportedOperationException("not used by broad m94-02 scenario");
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
            throw new UnsupportedOperationException("not used by broad m94-02 scenario");
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
