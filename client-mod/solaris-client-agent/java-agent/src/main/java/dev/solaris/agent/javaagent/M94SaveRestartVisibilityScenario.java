package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

final class M94SaveRestartVisibilityScenario {
    static final String ID = "m94-06-save-restart-two-client-visibility";
    static final String BEFORE_ID = "m94-06-save-restart-before";
    static final String AFTER_ID = "m94-06-save-restart-after";
    static final String TWO_CLIENT_PLACE_ID = "m94-06-two-client-place";
    static final String TWO_CLIENT_OBSERVE_ID = "m94-06-two-client-observe";
    static final String TWO_CLIENT_DROP_BREAK_ID = "m94-06-two-client-drop-break";
    static final String TWO_CLIENT_DROP_OBSERVE_ID = "m94-06-two-client-drop-observe";
    static final String TWO_CLIENT_PICKUP_COLLECT_ID = "m94-06-two-client-pickup-collect";
    static final String TWO_CLIENT_PICKUP_GONE_OBSERVE_ID = "m94-06-two-client-pickup-gone-observe";
    private static final String MARKER_FILE = "m94-06-save-restart-marker.properties";
    private static final String DROP_MARKER_FILE = "m94-06-shared-drop-marker.properties";
    private static final Duration SETUP_TIMEOUT = Duration.ofSeconds(8);
    private static final Duration BLOCK_TIMEOUT = Duration.ofSeconds(3);
    private static final Duration BREAK_TIMEOUT = Duration.ofSeconds(6);
    private static final int HOTBAR_SLOT = 0;

    static boolean supports(String id) {
        return ID.equals(id)
            || BEFORE_ID.equals(id)
            || AFTER_ID.equals(id)
            || TWO_CLIENT_PLACE_ID.equals(id)
            || TWO_CLIENT_OBSERVE_ID.equals(id)
            || TWO_CLIENT_DROP_BREAK_ID.equals(id)
            || TWO_CLIENT_DROP_OBSERVE_ID.equals(id)
            || TWO_CLIENT_PICKUP_COLLECT_ID.equals(id)
            || TWO_CLIENT_PICKUP_GONE_OBSERVE_ID.equals(id);
    }

    ClientScenarioReport run(String id, Path screenshotsDir, ScenarioClient client) {
        if (ID.equals(id)) {
            return new ClientScenarioReport(
                "blocked",
                id,
                List.of(
                    "blocked: m94-06 requires runner-managed before/after phases with a real server restart"
                )
            );
        }
        if (BEFORE_ID.equals(id)) {
            return runBeforeRestart(id, screenshotsDir, client);
        }
        if (AFTER_ID.equals(id)) {
            return runAfterRestart(id, screenshotsDir, client);
        }
        if (TWO_CLIENT_PLACE_ID.equals(id)) {
            return runTwoClientPlace(id, screenshotsDir, client);
        }
        if (TWO_CLIENT_OBSERVE_ID.equals(id)) {
            return runTwoClientObserve(id, screenshotsDir, client);
        }
        if (TWO_CLIENT_DROP_BREAK_ID.equals(id)) {
            return runTwoClientDropBreak(id, screenshotsDir, client);
        }
        if (TWO_CLIENT_DROP_OBSERVE_ID.equals(id)) {
            return runTwoClientDropObserve(id, screenshotsDir, client);
        }
        if (TWO_CLIENT_PICKUP_COLLECT_ID.equals(id)) {
            return runTwoClientPickupCollect(id, screenshotsDir, client);
        }
        if (TWO_CLIENT_PICKUP_GONE_OBSERVE_ID.equals(id)) {
            return runTwoClientPickupGoneObserve(id, screenshotsDir, client);
        }
        return new ClientScenarioReport("blocked", id, List.of("unsupported scenario id: " + id));
    }

    private ClientScenarioReport runBeforeRestart(String id, Path screenshotsDir, ScenarioClient client) {
        List<String> observations = new ArrayList<>();
        try {
            ScenarioBlockPair pair = client.findDryPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
            if (pair == null) {
                observations.add("blocked: no loaded dry marker target found within survival reach");
                return new ClientScenarioReport("blocked", id, observations);
            }
            observations.add(
                "restart marker target: clicked=" + pair.clicked().blockId()
                    + " at " + coordinates(pair.clicked())
                    + ", target=" + pair.target().blockId()
                    + " at " + coordinates(pair.target())
            );

            ScenarioHeldItem dirt = client.giveAndSelect("minecraft:dirt", 1, HOTBAR_SLOT, SETUP_TIMEOUT);
            if (!dirt.matches("minecraft:dirt", 1)) {
                observations.add(
                    "blocked: expected held dirt marker item, saw " + dirt.itemId() + " x" + dirt.count()
                );
                return new ClientScenarioReport("blocked", id, observations);
            }
            ScenarioUseResult use = client.useItemOn(pair.clicked(), dirt);
            boolean placed = client.waitForBlock(pair.target(), "minecraft:dirt", BLOCK_TIMEOUT);
            observations.add(
                "restart marker placement: " + (placed ? "passed" : "failed")
                    + " use_result=" + use.result()
                    + " target=" + coordinates(pair.target())
            );
            if (!placed) {
                return new ClientScenarioReport("failed", id, observations);
            }

            writeMarker(markerPath(screenshotsDir), pair.target());
            client.sendCommand("save-all");
            observations.add("save-all command sent before runner-managed server restart");
            observations.add("screenshots directory available to driver: " + screenshotsDir);
            return new ClientScenarioReport("passed", id, observations);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private ClientScenarioReport runAfterRestart(String id, Path screenshotsDir, ScenarioClient client) {
        List<String> observations = new ArrayList<>();
        Path markerPath = markerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing restart marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        try {
            ScenarioBlockTarget marker = readMarker(markerPath);
            boolean persisted = client.waitForBlock(marker, "minecraft:dirt", BLOCK_TIMEOUT);
            observations.add(
                "restart marker persistence: " + (persisted ? "passed" : "failed")
                    + " target=" + coordinates(marker)
            );
            observations.add(
                "blocked: shared container convergence, edit contention, and broader two-client "
                    + "join/move coverage remain outside this restart marker phase"
            );
            observations.add("screenshots directory available to driver: " + screenshotsDir);
            return new ClientScenarioReport(persisted ? "blocked" : "failed", id, observations);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private ClientScenarioReport runTwoClientPlace(String id, Path screenshotsDir, ScenarioClient client) {
        List<String> observations = new ArrayList<>();
        try {
            ScenarioBlockPair pair = client.findDryPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
            if (pair == null) {
                observations.add("blocked: no loaded dry marker target found within survival reach");
                return new ClientScenarioReport("blocked", id, observations);
            }
            observations.add(
                "two-client marker target: clicked=" + pair.clicked().blockId()
                    + " at " + coordinates(pair.clicked())
                    + ", target=" + pair.target().blockId()
                    + " at " + coordinates(pair.target())
            );

            ScenarioHeldItem dirt = client.giveAndSelect("minecraft:dirt", 1, HOTBAR_SLOT, SETUP_TIMEOUT);
            if (!dirt.matches("minecraft:dirt", 1)) {
                observations.add(
                    "blocked: expected held dirt marker item, saw " + dirt.itemId() + " x" + dirt.count()
                );
                return new ClientScenarioReport("blocked", id, observations);
            }
            ScenarioUseResult use = client.useItemOn(pair.clicked(), dirt);
            boolean placed = client.waitForBlock(pair.target(), "minecraft:dirt", BLOCK_TIMEOUT);
            observations.add(
                "two-client marker placement: " + (placed ? "passed" : "failed")
                    + " use_result=" + use.result()
                    + " target=" + coordinates(pair.target())
            );
            if (!placed) {
                return new ClientScenarioReport("failed", id, observations);
            }

            writeMarker(markerPath(screenshotsDir), pair.target());
            observations.add("marker coordinates written for secondary real-client observer");
            return new ClientScenarioReport("passed", id, observations);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private ClientScenarioReport runTwoClientObserve(String id, Path screenshotsDir, ScenarioClient client) {
        List<String> observations = new ArrayList<>();
        Path markerPath = markerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing restart marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        try {
            ScenarioBlockTarget marker = readMarker(markerPath);
            boolean visible = client.waitForBlock(marker, "minecraft:dirt", BLOCK_TIMEOUT);
            observations.add(
                "two-client block visibility: " + (visible ? "passed" : "failed")
                    + " target=" + coordinates(marker)
            );
            return new ClientScenarioReport(visible ? "passed" : "failed", id, observations);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private ClientScenarioReport runTwoClientDropBreak(String id, Path screenshotsDir, ScenarioClient client) {
        List<String> observations = new ArrayList<>();
        try {
            ScenarioBlockPair pair = client.findPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
            if (pair == null) {
                observations.add("blocked: no loaded dirt-like drop target found within survival reach");
                return new ClientScenarioReport("blocked", id, observations);
            }
            observations.add(
                "two-client shared drop target: clicked=" + pair.clicked().blockId()
                    + " at " + coordinates(pair.clicked())
            );

            client.giveAndSelect("minecraft:dirt", 0, HOTBAR_SLOT, SETUP_TIMEOUT);
            ScenarioBreakResult broke = client.breakBlockUntilDropVisible(
                pair.clicked(),
                "minecraft:dirt",
                BREAK_TIMEOUT
            );
            boolean passed = broke.started() && broke.becameAir() && broke.sawDrop();
            observations.add(
                "two-client shared drop break: " + (passed ? "passed" : "failed")
                    + " started=" + broke.started()
                    + " became_air=" + broke.becameAir()
                    + " saw_drop=" + broke.sawDrop()
            );
            if (!passed) {
                return new ClientScenarioReport("failed", id, observations);
            }

            writeMarker(dropMarkerPath(screenshotsDir), pair.clicked());
            observations.add("drop coordinates written for secondary real-client observer");
            return new ClientScenarioReport("passed", id, observations);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private ClientScenarioReport runTwoClientDropObserve(String id, Path screenshotsDir, ScenarioClient client) {
        List<String> observations = new ArrayList<>();
        Path markerPath = dropMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing shared drop marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        try {
            ScenarioBlockTarget marker = readMarker(markerPath, "drop-marker");
            boolean visible = client.waitForVisibleItemDrop("minecraft:dirt", marker, BLOCK_TIMEOUT);
            observations.add(
                "two-client shared drop visibility: " + (visible ? "passed" : "failed")
                    + " target=" + coordinates(marker)
            );
            return new ClientScenarioReport(visible ? "passed" : "failed", id, observations);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private ClientScenarioReport runTwoClientPickupCollect(String id, Path screenshotsDir, ScenarioClient client) {
        List<String> observations = new ArrayList<>();
        Path markerPath = dropMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing shared drop marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        try {
            ScenarioBlockTarget marker = readMarker(markerPath, "drop-marker");
            ScenarioBreakResult pickup = client.collectVisibleItemDrop(
                marker,
                "minecraft:dirt",
                1,
                BREAK_TIMEOUT
            );
            boolean passed = pickup.becameAir() && pickup.pickupRestored();
            observations.add(
                "two-client shared pickup: " + (passed ? "passed" : "failed")
                    + " saw_drop=" + pickup.sawDrop()
                    + " drop_gone=" + pickup.becameAir()
                    + " pickup_restored=" + pickup.pickupRestored()
                    + " held=" + pickup.selectedItem().itemId() + " x" + pickup.selectedItem().count()
            );
            return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private ClientScenarioReport runTwoClientPickupGoneObserve(String id, Path screenshotsDir, ScenarioClient client) {
        List<String> observations = new ArrayList<>();
        Path markerPath = dropMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing shared drop marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        try {
            ScenarioBlockTarget marker = readMarker(markerPath, "drop-marker");
            boolean removed = client.waitForNoVisibleItemDrop("minecraft:dirt", marker, BLOCK_TIMEOUT);
            observations.add(
                "two-client shared pickup removal: " + (removed ? "passed" : "failed")
                    + " target=" + coordinates(marker)
            );
            return new ClientScenarioReport(removed ? "passed" : "failed", id, observations);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private static Path markerPath(Path screenshotsDir) {
        Path runDir = screenshotsDir.getParent();
        return (runDir == null ? Path.of(".") : runDir).resolve(MARKER_FILE);
    }

    private static Path dropMarkerPath(Path screenshotsDir) {
        Path runDir = screenshotsDir.getParent();
        return (runDir == null ? Path.of(".") : runDir).resolve(DROP_MARKER_FILE);
    }

    private static void writeMarker(Path path, ScenarioBlockTarget target) throws IOException {
        Files.createDirectories(path.getParent());
        Files.writeString(
            path,
            "x=" + target.x() + "\n"
                + "y=" + target.y() + "\n"
                + "z=" + target.z() + "\n"
                + "face=" + target.face() + "\n"
        );
    }

    private static ScenarioBlockTarget readMarker(Path path) throws IOException {
        return readMarker(path, "restart-marker");
    }

    private static ScenarioBlockTarget readMarker(Path path, String label) throws IOException {
        Integer x = null;
        Integer y = null;
        Integer z = null;
        String face = null;
        for (String line : Files.readAllLines(path)) {
            String[] parts = line.split("=", 2);
            if (parts.length != 2) {
                continue;
            }
            switch (parts[0]) {
                case "x" -> x = parseMarkerInt(path, "x", parts[1]);
                case "y" -> y = parseMarkerInt(path, "y", parts[1]);
                case "z" -> z = parseMarkerInt(path, "z", parts[1]);
                case "face" -> face = parts[1];
                default -> {
                }
            }
        }
        if (x == null || y == null || z == null || face == null || face.isBlank()) {
            throw new IOException("invalid restart marker: missing x, y, z, or face in " + path);
        }
        return new ScenarioBlockTarget(x, y, z, face, label, "minecraft:dirt");
    }

    private static int parseMarkerInt(Path path, String key, String value) throws IOException {
        try {
            return Integer.parseInt(value);
        } catch (NumberFormatException error) {
            throw new IOException("invalid restart marker: " + key + "=" + value + " in " + path, error);
        }
    }

    private static String coordinates(ScenarioBlockTarget target) {
        return target.x() + "," + target.y() + "," + target.z() + "/" + target.face();
    }
}
