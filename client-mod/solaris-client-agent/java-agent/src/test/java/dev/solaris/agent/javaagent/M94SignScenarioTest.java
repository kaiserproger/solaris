package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;
import org.junit.jupiter.api.Test;

import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class M94SignScenarioTest {
    @Test
    void runsFocusedRegularSignPlaceAndTextScenarioThroughClientActions() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new M94SignScenario().run(
            "m94-04a-regular-sign-place-text",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "find-dry-placeable:within-survival-reach",
            "give:minecraft:oak_sign:1:0",
            "use:minecraft:oak_sign:place-clicked",
            "wait-any-block:place-target:minecraft:oak_sign|minecraft:oak_wall_sign",
            "held:minecraft:air:0",
            "wait-sign-editor:place-target",
            "update-sign:place-target:Solaris|M94|real-client|sign",
            "wait-sign:place-target:Solaris|M94|real-client|sign",
            "closeScreen",
            "wait-sign:place-target:Solaris|M94|real-client|sign"
        ), client.operations);
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("sign placement: passed")),
            "scenario report must name the regular sign placement result"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("sign text update: passed")),
            "scenario report must name the plain text update result"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("degraded: hanging signs")),
            "scenario report must keep broad sign/block-entity paths degraded"
        );
    }

    @Test
    void blocksWhenNoSignTargetIsLoaded() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.placeablePair = null;

        ClientScenarioReport report = new M94SignScenario().run(
            "m94-04a-regular-sign-place-text",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("blocked", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("placeable sign target")),
            "blocked report must explain that no sign placement target was found"
        );
    }

    @Test
    void blocksUnknownScenarioIds() {
        ClientScenarioReport report = new M94SignScenario().run(
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
            new ScenarioBlockTarget(0, 64, 0, "up", "place-clicked", "minecraft:grass_block"),
            new ScenarioBlockTarget(0, 65, 0, "down", "place-target", "minecraft:air")
        );
        ScenarioHeldItem selected = new ScenarioHeldItem("minecraft:air", 0);

        @Override
        public ScenarioBlockPair findOccupiedPair(ScenarioReach reach) {
            throw new UnsupportedOperationException("not used by sign scenario");
        }

        @Override
        public ScenarioBlockPair findPlaceablePair(ScenarioReach reach) {
            throw new UnsupportedOperationException("sign scenario must request a dry target");
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
            selected = new ScenarioHeldItem("minecraft:air", 0);
            return new ScenarioUseResult("success");
        }

        @Override
        public boolean waitForBlock(ScenarioBlockTarget target, String blockId, Duration duration) {
            throw new UnsupportedOperationException("sign scenario must accept floor or wall signs");
        }

        @Override
        public boolean waitForAnyBlock(ScenarioBlockTarget target, List<String> blockIds, Duration duration) {
            operations.add("wait-any-block:" + target.label() + ":" + String.join("|", blockIds));
            return true;
        }

        @Override
        public boolean waitForStableBlocks(ScenarioBlockPair pair, Duration duration) {
            throw new UnsupportedOperationException("not used by sign scenario");
        }

        @Override
        public boolean waitForNoFluid(ScenarioBlockTarget target, Duration duration) {
            throw new UnsupportedOperationException("not used by sign scenario");
        }

        @Override
        public boolean waitForSignEditor(ScenarioBlockTarget target, Duration duration) {
            operations.add("wait-sign-editor:" + target.label());
            return true;
        }

        @Override
        public void updateSignText(ScenarioBlockTarget target, List<String> lines) {
            operations.add("update-sign:" + target.label() + ":" + String.join("|", lines));
        }

        @Override
        public boolean waitForSignText(ScenarioBlockTarget target, List<String> lines, Duration duration) {
            operations.add("wait-sign:" + target.label() + ":" + String.join("|", lines));
            return true;
        }

        @Override
        public boolean closeCurrentScreen(Duration duration) {
            operations.add("closeScreen");
            return true;
        }

        @Override
        public ScenarioBreakResult breakBlock(
            ScenarioBlockTarget target,
            String expectedDropItemId,
            int expectedSelectedCount,
            Duration timeout
        ) {
            throw new UnsupportedOperationException("not used by sign scenario");
        }

        @Override
        public ScenarioHeldItem selectedItem() {
            operations.add("held:" + selected.itemId() + ":" + selected.count());
            return selected;
        }
    }
}
