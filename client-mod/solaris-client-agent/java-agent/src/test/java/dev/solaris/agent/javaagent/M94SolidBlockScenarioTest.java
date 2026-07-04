package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;
import org.junit.jupiter.api.Test;

import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class M94SolidBlockScenarioTest {
    @Test
    void runsFocusedSolidPlaceBreakAndPickupScenarioThroughClientActions() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new M94SolidBlockScenario().run(
            "m94-02a-solid-place-break-drop",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "find-placeable:within-survival-reach",
            "give:minecraft:dirt:0:0",
            "break:place-clicked:minecraft:dirt:1",
            "find-placeable:within-survival-reach",
            "use:minecraft:dirt:second-place-clicked",
            "wait-block:second-place-target:minecraft:dirt",
            "held:minecraft:air:0"
        ), client.operations);
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("solid placement: passed")),
            "scenario report must name the accepted placement result"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("break/drop/pickup: passed")),
            "scenario report must name the break/drop/pickup result"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("degraded: door/trapdoor")),
            "scenario report must explicitly keep unsupported broad M94 sub-steps degraded"
        );
    }

    @Test
    void blocksWhenNoPlaceableTargetIsLoaded() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.placeablePair = null;

        ClientScenarioReport report = new M94SolidBlockScenario().run(
            "m94-02a-solid-place-break-drop",
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
        ClientScenarioReport report = new M94SolidBlockScenario().run(
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
        ScenarioBlockPair placeablePair = new ScenarioBlockPair(
            new ScenarioBlockTarget(0, 64, 0, "east", "place-clicked", "minecraft:grass_block"),
            new ScenarioBlockTarget(1, 64, 0, "west", "place-target", "minecraft:air")
        );
        ScenarioBlockPair secondPlaceablePair = new ScenarioBlockPair(
            new ScenarioBlockTarget(2, 64, 0, "east", "second-place-clicked", "minecraft:grass_block"),
            new ScenarioBlockTarget(3, 64, 0, "west", "second-place-target", "minecraft:air")
        );
        ScenarioHeldItem selected = new ScenarioHeldItem("minecraft:air", 0);

        @Override
        public ScenarioBlockPair findOccupiedPair(ScenarioReach reach) {
            throw new UnsupportedOperationException("not used by solid-block scenario");
        }

        @Override
        public ScenarioBlockPair findPlaceablePair(ScenarioReach reach) {
            operations.add("find-placeable:" + reach.label());
            findPlaceableCalls += 1;
            return findPlaceableCalls == 1 ? placeablePair : secondPlaceablePair;
        }

        @Override
        public ScenarioBlockPair findDryPlaceablePair(ScenarioReach reach) {
            throw new UnsupportedOperationException("not used by solid-block scenario");
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
            selected = heldItem.count() <= 1
                ? new ScenarioHeldItem("minecraft:air", 0)
                : new ScenarioHeldItem(heldItem.itemId(), heldItem.count() - 1);
            return new ScenarioUseResult("success");
        }

        @Override
        public boolean waitForBlock(ScenarioBlockTarget target, String blockId, Duration duration) {
            operations.add("wait-block:" + target.label() + ":" + blockId);
            return true;
        }

        @Override
        public boolean waitForStableBlocks(ScenarioBlockPair pair, Duration duration) {
            throw new UnsupportedOperationException("not used by solid-block scenario");
        }

        @Override
        public boolean waitForNoFluid(ScenarioBlockTarget target, Duration duration) {
            throw new UnsupportedOperationException("not used by solid-block scenario");
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
