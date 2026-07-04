package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;

import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

final class M94SignScenario {
    static final String ID = "m94-04a-regular-sign-place-text";
    private static final Duration SETUP_TIMEOUT = Duration.ofSeconds(8);
    private static final Duration SIGN_TIMEOUT = Duration.ofSeconds(4);
    private static final List<String> SIGN_BLOCK_IDS = List.of(
        "minecraft:oak_sign",
        "minecraft:oak_wall_sign"
    );
    private static final List<String> SIGN_LINES = List.of(
        "Solaris",
        "M94",
        "real-client",
        "sign"
    );

    ClientScenarioReport run(String id, Path screenshotsDir, ScenarioClient client) {
        if (!ID.equals(id)) {
            return new ClientScenarioReport("blocked", id, List.of("unsupported scenario id: " + id));
        }

        List<String> observations = new ArrayList<>();
        try {
            ScenarioBlockPair pair = client.findDryPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
            if (pair == null) {
                observations.add("blocked: no loaded placeable sign target found within survival reach");
                return new ClientScenarioReport("blocked", id, observations);
            }
            observations.add(
                "sign target: clicked=" + pair.clicked().blockId()
                    + " at " + coordinates(pair.clicked())
                    + ", target=" + pair.target().blockId()
                    + " at " + coordinates(pair.target())
            );

            ScenarioHeldItem sign = client.giveAndSelect("minecraft:oak_sign", 1, 0, SETUP_TIMEOUT);
            if (!sign.matches("minecraft:oak_sign", 1)) {
                observations.add(
                    "blocked: expected held oak sign, saw "
                        + sign.itemId() + " x" + sign.count()
                );
                return new ClientScenarioReport("blocked", id, observations);
            }
            observations.add("held setup: minecraft:oak_sign x1 in hotbar slot 0");

            ScenarioUseResult placeUse = client.useItemOn(pair.clicked(), sign);
            boolean signVisible = client.waitForAnyBlock(pair.target(), SIGN_BLOCK_IDS, SIGN_TIMEOUT);
            ScenarioHeldItem afterPlace = client.selectedItem();
            boolean decremented = afterPlace.matches("minecraft:air", 0);
            observations.add(
                "sign placement: " + (signVisible && decremented ? "passed" : "failed")
                    + " use_result=" + placeUse.result()
                    + " target_is_regular_sign=" + signVisible
                    + " held=" + afterPlace.itemId() + " x" + afterPlace.count()
            );

            boolean editorOpen = client.waitForSignEditor(pair.target(), SIGN_TIMEOUT);
            observations.add("sign editor: " + (editorOpen ? "passed" : "failed"));
            if (editorOpen) {
                client.updateSignText(pair.target(), SIGN_LINES);
            }
            boolean textVisible = editorOpen
                && client.waitForSignText(pair.target(), SIGN_LINES, SIGN_TIMEOUT);
            observations.add(
                "sign text update: " + (textVisible ? "passed" : "failed")
                    + " lines=" + String.join("|", SIGN_LINES)
            );
            boolean editorClosed = textVisible && client.closeCurrentScreen(SIGN_TIMEOUT);
            observations.add("sign editor close: " + (editorClosed ? "passed" : "failed"));
            boolean textVisibleAfterClose = editorClosed
                && client.waitForSignText(pair.target(), SIGN_LINES, SIGN_TIMEOUT);
            observations.add(
                "sign text after close: " + (textVisibleAfterClose ? "passed" : "failed")
                    + " lines=" + String.join("|", SIGN_LINES)
            );
            observations.add(
                "degraded: hanging signs, waxed signs, styled/filtered/clickable text, "
                    + "bed sleep/respawn, campfires, restart persistence, and broad visual parity "
                    + "assertions are not exercised by " + ID
            );
            observations.add("screenshots directory available to driver: " + screenshotsDir);

            boolean passed = signVisible && decremented && editorOpen && textVisibleAfterClose;
            return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private static String coordinates(ScenarioBlockTarget target) {
        return target.x() + "," + target.y() + "," + target.z() + "/" + target.face();
    }
}
