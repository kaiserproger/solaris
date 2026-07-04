package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class M94SaveRestartVisibilityScenarioTest {
    @Test
    void beforeRestartPlacesDirtMarkerAndWritesPhaseFile() {
        FakeScenarioClient client = new FakeScenarioClient();
        Path screenshotsDir = Path.of("build/tmp/m94-06-test/screenshots");

        ClientScenarioReport report = new M94SaveRestartVisibilityScenario().run(
            "m94-06-save-restart-before",
            screenshotsDir,
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "find-dry-placeable:within-survival-reach",
            "give:minecraft:dirt:1:0",
            "use:minecraft:dirt:marker-clicked",
            "wait-block:marker-target:minecraft:dirt",
            "command:save-all"
        ), client.operations);
        assertTrue(
            screenshotsDir.getParent().resolve("m94-06-save-restart-marker.properties").toFile().isFile(),
            "pre-restart phase must persist the marker target for the post-restart phase"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("restart marker placement: passed")),
            "pre-restart report must name marker placement"
        );
    }

    @Test
    void afterRestartChecksMarkerAndBlocksTwoClientSubrows() {
        FakeScenarioClient client = new FakeScenarioClient();
        Path screenshotsDir = Path.of("build/tmp/m94-06-test/screenshots");
        new M94SaveRestartVisibilityScenario().run(
            "m94-06-save-restart-before",
            screenshotsDir,
            client
        );
        client.operations.clear();

        ClientScenarioReport report = new M94SaveRestartVisibilityScenario().run(
            "m94-06-save-restart-after",
            screenshotsDir,
            client
        );

        assertEquals("blocked", report.result());
        assertEquals(List.of(
            "wait-block:restart-marker:minecraft:dirt"
        ), client.operations);
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("restart marker persistence: passed")),
            "post-restart report must name the persisted marker observation"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("blocked: shared container")),
            "post-restart report must keep shared multiplayer gaps blocked"
        );
    }

    @Test
    void twoClientPrimaryPlacesMarkerWithoutSaveAll() {
        FakeScenarioClient client = new FakeScenarioClient();
        Path screenshotsDir = Path.of("build/tmp/m94-06-two-client/screenshots");

        ClientScenarioReport report = new M94SaveRestartVisibilityScenario().run(
            "m94-06-two-client-place",
            screenshotsDir,
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "find-dry-placeable:within-survival-reach",
            "give:minecraft:dirt:1:0",
            "use:minecraft:dirt:marker-clicked",
            "wait-block:marker-target:minecraft:dirt"
        ), client.operations);
        assertTrue(
            screenshotsDir.getParent().resolve("m94-06-save-restart-marker.properties").toFile().isFile(),
            "primary two-client phase must persist marker coordinates for the observer client"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client marker placement: passed")),
            "primary two-client phase must name the live visibility marker placement"
        );
    }

    @Test
    void twoClientSecondaryObservesPrimaryMarkerAndPasses() {
        FakeScenarioClient client = new FakeScenarioClient();
        Path screenshotsDir = Path.of("build/tmp/m94-06-two-client-observe/screenshots");
        new M94SaveRestartVisibilityScenario().run(
            "m94-06-two-client-place",
            screenshotsDir,
            client
        );
        client.operations.clear();

        ClientScenarioReport report = new M94SaveRestartVisibilityScenario().run(
            "m94-06-two-client-observe",
            screenshotsDir,
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "wait-block:restart-marker:minecraft:dirt"
        ), client.operations);
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client block visibility: passed")),
            "secondary two-client phase must record that it saw the primary client's marker"
        );
    }

    @Test
    void twoClientPrimaryBreaksSharedDropAndWritesDropMarker() {
        FakeScenarioClient client = new FakeScenarioClient();
        Path screenshotsDir = Path.of("build/tmp/m94-06-two-client-drop/screenshots");

        ClientScenarioReport report = new M94SaveRestartVisibilityScenario().run(
            "m94-06-two-client-drop-break",
            screenshotsDir,
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "find-placeable:within-survival-reach",
            "give:minecraft:dirt:0:0",
            "break-visible-drop:drop-clicked:minecraft:dirt"
        ), client.operations);
        assertTrue(
            screenshotsDir.getParent().resolve("m94-06-shared-drop-marker.properties").toFile().isFile(),
            "primary two-client drop phase must persist drop coordinates for the observer client"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client shared drop break: passed")),
            "primary two-client drop phase must name the visible shared drop break"
        );
    }

    @Test
    void twoClientSecondaryObservesSharedDropAndPasses() {
        FakeScenarioClient client = new FakeScenarioClient();
        Path screenshotsDir = Path.of("build/tmp/m94-06-two-client-drop-observe/screenshots");
        new M94SaveRestartVisibilityScenario().run(
            "m94-06-two-client-drop-break",
            screenshotsDir,
            client
        );
        client.operations.clear();

        ClientScenarioReport report = new M94SaveRestartVisibilityScenario().run(
            "m94-06-two-client-drop-observe",
            screenshotsDir,
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "wait-drop:drop-marker:minecraft:dirt"
        ), client.operations);
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client shared drop visibility: passed")),
            "secondary two-client drop phase must record that it saw the primary client's item drop"
        );
    }

    @Test
    void twoClientPrimaryCollectsSharedDropAndPasses() {
        FakeScenarioClient client = new FakeScenarioClient();
        Path screenshotsDir = Path.of("build/tmp/m94-06-two-client-pickup/screenshots");
        new M94SaveRestartVisibilityScenario().run(
            "m94-06-two-client-drop-break",
            screenshotsDir,
            client
        );
        client.operations.clear();

        ClientScenarioReport report = new M94SaveRestartVisibilityScenario().run(
            "m94-06-two-client-pickup-collect",
            screenshotsDir,
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "collect-drop:drop-marker:minecraft:dirt:1"
        ), client.operations);
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client shared pickup: passed")),
            "primary two-client pickup phase must record pickup convergence"
        );
    }

    @Test
    void twoClientSecondaryObservesSharedPickupRemovalAndPasses() {
        FakeScenarioClient client = new FakeScenarioClient();
        Path screenshotsDir = Path.of("build/tmp/m94-06-two-client-pickup-gone/screenshots");
        new M94SaveRestartVisibilityScenario().run(
            "m94-06-two-client-drop-break",
            screenshotsDir,
            client
        );
        client.operations.clear();

        ClientScenarioReport report = new M94SaveRestartVisibilityScenario().run(
            "m94-06-two-client-pickup-gone-observe",
            screenshotsDir,
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "wait-no-drop:drop-marker:minecraft:dirt"
        ), client.operations);
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client shared pickup removal: passed")),
            "secondary two-client pickup phase must record item removal visibility"
        );
    }

    @Test
    void afterRestartFailsWhenMarkerIsMissing() {
        ClientScenarioReport report = new M94SaveRestartVisibilityScenario().run(
            "m94-06-save-restart-after",
            Path.of("build/tmp/m94-06-missing/screenshots"),
            new FakeScenarioClient()
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("missing restart marker")),
            "post-restart phase must fail closed when the marker file is absent"
        );
    }

    @Test
    void afterRestartFailsWhenMarkerIsIncomplete() throws Exception {
        Path screenshotsDir = Path.of("build/tmp/m94-06-incomplete/screenshots");
        Path marker = screenshotsDir.getParent().resolve("m94-06-save-restart-marker.properties");
        Files.createDirectories(marker.getParent());
        Files.writeString(marker, "x=11\nz=10\nface=west\n");

        ClientScenarioReport report = new M94SaveRestartVisibilityScenario().run(
            "m94-06-save-restart-after",
            screenshotsDir,
            new FakeScenarioClient()
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("invalid restart marker")),
            "post-restart phase must fail closed when the marker file is incomplete"
        );
    }

    @Test
    void blocksUnknownScenarioIds() {
        ClientScenarioReport report = new M94SaveRestartVisibilityScenario().run(
            "m94-unknown",
            Path.of("build/tmp/m94-06-test/screenshots"),
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
            throw new UnsupportedOperationException("not used by save/restart scenario");
        }

        @Override
        public ScenarioBlockPair findPlaceablePair(ScenarioReach reach) {
            operations.add("find-placeable:" + reach.label());
            return new ScenarioBlockPair(
                new ScenarioBlockTarget(12, 64, 10, "east", "drop-clicked", "minecraft:grass_block"),
                new ScenarioBlockTarget(13, 64, 10, "west", "drop-target", "minecraft:air")
            );
        }

        @Override
        public ScenarioBlockPair findDryPlaceablePair(ScenarioReach reach) {
            operations.add("find-dry-placeable:" + reach.label());
            return new ScenarioBlockPair(
                new ScenarioBlockTarget(10, 64, 10, "east", "marker-clicked", "minecraft:grass_block"),
                new ScenarioBlockTarget(11, 64, 10, "west", "marker-target", "minecraft:air")
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
            operations.add("wait-block:" + target.label() + ":" + blockId);
            return true;
        }

        @Override
        public void sendCommand(String command) {
            operations.add("command:" + command);
        }

        @Override
        public boolean waitForStableBlocks(ScenarioBlockPair pair, Duration duration) {
            throw new UnsupportedOperationException("not used by save/restart scenario");
        }

        @Override
        public boolean waitForNoFluid(ScenarioBlockTarget target, Duration duration) {
            throw new UnsupportedOperationException("not used by save/restart scenario");
        }

        @Override
        public ScenarioBreakResult breakBlock(
            ScenarioBlockTarget target,
            String expectedDropItemId,
            int expectedSelectedCount,
            Duration timeout
        ) {
            throw new UnsupportedOperationException("not used by save/restart scenario");
        }

        @Override
        public ScenarioBreakResult breakBlockUntilDropVisible(
            ScenarioBlockTarget target,
            String expectedDropItemId,
            Duration timeout
        ) {
            operations.add("break-visible-drop:" + target.label() + ":" + expectedDropItemId);
            return new ScenarioBreakResult(true, true, true, false, selected);
        }

        @Override
        public boolean waitForVisibleItemDrop(String itemId, ScenarioBlockTarget near, Duration timeout) {
            operations.add("wait-drop:" + near.label() + ":" + itemId);
            return true;
        }

        public ScenarioBreakResult collectVisibleItemDrop(
            ScenarioBlockTarget near,
            String expectedDropItemId,
            int expectedSelectedCount,
            Duration timeout
        ) {
            operations.add(
                "collect-drop:" + near.label() + ":" + expectedDropItemId + ":" + expectedSelectedCount
            );
            selected = new ScenarioHeldItem(expectedDropItemId, expectedSelectedCount);
            return new ScenarioBreakResult(true, true, true, true, selected);
        }

        public boolean waitForNoVisibleItemDrop(String itemId, ScenarioBlockTarget near, Duration timeout) {
            operations.add("wait-no-drop:" + near.label() + ":" + itemId);
            return true;
        }

        @Override
        public ScenarioHeldItem selectedItem() {
            return selected;
        }
    }
}
