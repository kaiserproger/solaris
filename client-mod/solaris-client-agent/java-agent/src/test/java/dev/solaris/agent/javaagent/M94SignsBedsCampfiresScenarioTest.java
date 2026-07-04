package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;
import org.junit.jupiter.api.Test;

import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class M94SignsBedsCampfiresScenarioTest {
    @Test
    void runsRegularSignSubprobeThenBlocksRemainingBroadSubrows() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new M94SignsBedsCampfiresScenario().run(
            "m94-04-signs-beds-campfires-and-block-entities",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("blocked", report.result());
        assertTrue(
            client.operations.contains("use:minecraft:oak_sign:sign-clicked"),
            "broad scenario must run the real sign placement subprobe"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("sign subprobe result: passed")),
            "broad scenario must report the sign subprobe result"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("blocked: beds")),
            "broad scenario must name remaining broad-row blockers"
        );
    }

    @Test
    void blocksUnknownScenarioIds() {
        ClientScenarioReport report = new M94SignsBedsCampfiresScenario().run(
            "m94-unknown",
            Path.of("run/screenshots"),
            new FakeScenarioClient()
        );

        assertEquals("blocked", report.result());
        assertTrue(report.observations().get(0).contains("unsupported scenario"));
    }

    private static final class FakeScenarioClient implements ScenarioClient {
        final List<String> operations = new ArrayList<>();
        ScenarioHeldItem selected = new ScenarioHeldItem("minecraft:air", 0);

        @Override
        public ScenarioBlockPair findOccupiedPair(ScenarioReach reach) {
            throw new UnsupportedOperationException("not used by broad m94-04 scenario");
        }

        @Override
        public ScenarioBlockPair findPlaceablePair(ScenarioReach reach) {
            throw new UnsupportedOperationException("broad m94-04 sign subprobe must request a dry target");
        }

        @Override
        public ScenarioBlockPair findDryPlaceablePair(ScenarioReach reach) {
            operations.add("find-dry-placeable:" + reach.label());
            return new ScenarioBlockPair(
                new ScenarioBlockTarget(0, 64, 0, "up", "sign-clicked", "minecraft:grass_block"),
                new ScenarioBlockTarget(0, 65, 0, "down", "sign-target", "minecraft:air")
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
            selected = new ScenarioHeldItem("minecraft:air", 0);
            return new ScenarioUseResult("success");
        }

        @Override
        public boolean waitForBlock(ScenarioBlockTarget target, String blockId, Duration duration) {
            throw new UnsupportedOperationException("sign subprobe must accept floor or wall signs");
        }

        @Override
        public boolean waitForAnyBlock(ScenarioBlockTarget target, List<String> blockIds, Duration duration) {
            operations.add("wait-any-block:" + target.label() + ":" + String.join("|", blockIds));
            return true;
        }

        @Override
        public boolean waitForStableBlocks(ScenarioBlockPair pair, Duration duration) {
            throw new UnsupportedOperationException("not used by broad m94-04 scenario");
        }

        @Override
        public boolean waitForNoFluid(ScenarioBlockTarget target, Duration duration) {
            throw new UnsupportedOperationException("not used by broad m94-04 scenario");
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
            throw new UnsupportedOperationException("not used by broad m94-04 scenario");
        }

        @Override
        public ScenarioHeldItem selectedItem() {
            operations.add("held:" + selected.itemId() + ":" + selected.count());
            return selected;
        }
    }
}
