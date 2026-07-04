package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;

import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

final class M94RejectedBlockScenario {
    private static final String ID = "m94-02b-rejected-block-resync";
    private static final Duration SETUP_TIMEOUT = Duration.ofSeconds(8);
    private static final Duration STABILITY_WINDOW = Duration.ofMillis(900);

    ClientScenarioReport run(String id, Path screenshotsDir, ScenarioClient client) {
        if (!ID.equals(id)) {
            return new ClientScenarioReport("blocked", id, List.of("unsupported scenario id: " + id));
        }

        List<String> observations = new ArrayList<>();
        try {
            ScenarioBlockPair nearPair = requirePair(
                client,
                ScenarioReach.WITHIN_SURVIVAL_REACH,
                observations
            );
            ScenarioBlockPair farPair = requirePair(
                client,
                ScenarioReach.OUTSIDE_SURVIVAL_REACH,
                observations
            );
            if (nearPair == null || farPair == null) {
                return new ClientScenarioReport("blocked", id, observations);
            }

            ScenarioHeldItem dirt = requireHeldItem(
                client,
                "minecraft:dirt",
                1,
                0,
                observations
            );
            if (dirt == null) {
                return new ClientScenarioReport("blocked", id, observations);
            }
            boolean passed = runProbe(client, "occupied solid placement", nearPair, dirt, false, observations);
            passed &= runProbe(client, "out-of-reach solid placement", farPair, dirt, false, observations);

            ScenarioHeldItem waterBucket = requireHeldItem(
                client,
                "minecraft:water_bucket",
                1,
                0,
                observations
            );
            if (waterBucket == null) {
                return new ClientScenarioReport("blocked", id, observations);
            }
            passed &= runProbe(client, "occupied water-bucket fallback", nearPair, waterBucket, true, observations);

            observations.add("screenshots directory available to driver: " + screenshotsDir);
            return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private static ScenarioBlockPair requirePair(
        ScenarioClient client,
        ScenarioReach reach,
        List<String> observations
    ) throws Exception {
        ScenarioBlockPair pair = client.findOccupiedPair(reach);
        if (pair == null) {
            observations.add("blocked: no loaded occupied block pair found for " + reach.label());
            return null;
        }
        observations.add(
            "target " + reach.label() + ": clicked=" + pair.clicked().blockId()
                + " at " + coordinates(pair.clicked())
                + ", target=" + pair.target().blockId()
                + " at " + coordinates(pair.target())
        );
        return pair;
    }

    private static ScenarioHeldItem requireHeldItem(
        ScenarioClient client,
        String itemId,
        int count,
        int hotbarSlot,
        List<String> observations
    ) throws Exception {
        ScenarioHeldItem held = client.giveAndSelect(itemId, count, hotbarSlot, SETUP_TIMEOUT);
        if (!held.matches(itemId, count)) {
            observations.add(
                "blocked: expected held item " + itemId + " x" + count
                    + ", saw " + held.itemId() + " x" + held.count()
            );
            return null;
        }
        observations.add("held setup: " + itemId + " x" + count + " in hotbar slot " + hotbarSlot);
        return held;
    }

    private static boolean runProbe(
        ScenarioClient client,
        String label,
        ScenarioBlockPair pair,
        ScenarioHeldItem held,
        boolean requireNoFluid,
        List<String> observations
    ) throws Exception {
        ScenarioUseResult use = client.useItemOn(pair.clicked(), held);
        boolean stableBlocks = client.waitForStableBlocks(pair, STABILITY_WINDOW);
        boolean noFluid = !requireNoFluid || client.waitForNoFluid(pair.target(), STABILITY_WINDOW);
        ScenarioHeldItem after = client.selectedItem();
        boolean heldUnchanged = after.matches(held.itemId(), held.count());
        boolean passed = stableBlocks && noFluid && heldUnchanged;
        observations.add(
            label + ": " + (passed ? "passed" : "failed")
                + " use_result=" + use.result()
                + " stable_blocks=" + stableBlocks
                + " no_fluid=" + noFluid
                + " held=" + after.itemId() + " x" + after.count()
        );
        return passed;
    }

    private static String coordinates(ScenarioBlockTarget target) {
        return target.x() + "," + target.y() + "," + target.z() + "/" + target.face();
    }
}
