package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.UUID;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class PlayableRealClientLoopScenarioTest {
    private static final String SUPPORTED_LOGS = String.join("|", List.of(
        "minecraft:oak_log",
        "minecraft:spruce_log",
        "minecraft:birch_log",
        "minecraft:jungle_log",
        "minecraft:acacia_log",
        "minecraft:dark_oak_log",
        "minecraft:mangrove_log",
        "minecraft:cherry_log",
        "minecraft:pale_oak_log"
    ));

    @Test
    void supportsPassiveLivestockMotionScenario() {
        assertTrue(
            PlayableRealClientLoopScenario.supports("playable-44-passive-livestock-motion"),
            "playable client must expose the reusable livestock movement regression"
        );
    }

    @Test
    void passiveLivestockMotionChecksAllSpeciesWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-44-passive-livestock-motion",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertFalse(client.usedDebugSetup(), "livestock movement must use natural spawned entities");
        assertFalse(
            client.operations.stream().anyMatch(operation -> operation.startsWith("approachEntity:")),
            "livestock observation must not move the player across unrelated chunks"
        );
        for (String entityTypeId : List.of("minecraft:cow", "minecraft:sheep", "minecraft:chicken")) {
            assertTrue(
                client.operations.stream().anyMatch(operation -> operation.startsWith("motion:" + entityTypeId)),
                "scenario must observe packet-driven motion for " + entityTypeId
            );
            assertTrue(
                report.observations().stream().anyMatch(
                    observation -> observation.contains("livestock motion: passed entity=" + entityTypeId)
                ),
                "scenario must record movement evidence for " + entityTypeId
            );
        }
        assertTrue(
            report.observations().stream().anyMatch(
                observation -> observation.contains("entity=minecraft:cow")
                    && observation.contains("vertical_rise=1.1")
            ),
            "cow observation must prove a vanilla-height step climb"
        );
    }

    @Test
    void joinGeneratedSpawnProbePassesWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-01-join-generated-spawn",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertEquals(List.of(
            "selected",
            "count:minecraft:oak_log",
            "count:minecraft:oak_planks",
            "count:minecraft:crafting_table",
            "count:minecraft:wooden_pickaxe"
        ), client.operations);
        assertFalse(client.usedDebugSetup(), "playable join probe must not use debug setup helpers");
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("join/play-state: passed")),
            "join probe must record that the real client reached playable state"
        );
    }

    @Test
    void fullLoopCraftsToolSoaksAndWritesRestartMarkerWithoutDebugSetup() throws Exception {
        FakeScenarioClient client = new FakeScenarioClient();
        client.visiblePassiveDuringSoak = true;
        client.visibleHostileDuringSoak = true;
        Path screenshotsDir = Path.of("build/tmp/playable-04-test/screenshots");
        Files.deleteIfExists(screenshotsDir.getParent().resolve("playable-03-save-restart-marker.properties"));

        ClientScenarioReport report = new PlayableRealClientLoopScenario(Duration.ofMillis(1_050)).run(
            "playable-04-twenty-minute-survival-loop",
            screenshotsDir,
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertFalse(client.usedDebugSetup(), "playable loop must not fall back to debug setup");
        assertTrue(
            screenshotsDir.getParent().resolve("playable-03-save-restart-marker.properties").toFile().isFile(),
            "full playable loop must write marker coordinates for the runner-managed restart phase"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("20-minute survival soak: passed")),
            "full playable loop must record the survival soak evidence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("natural spawn acceptance: passed")),
            "the full playable loop must fail closed unless passive and hostile natural spawn evidence was observed"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("survival resource work: passed")),
            "the long playable loop must perform useful work instead of standing AFK"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("wooden sword recipe: passed")),
            "the long survival loop must prepare a basic combat weapon before night"
        );
    }


    @Test
    void fullLoopFailsClosedWhenNaturalSpawnEvidenceNeverAppears() throws Exception {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario(Duration.ofMillis(1)).run(
            "playable-04-twenty-minute-survival-loop",
            Path.of("build/tmp/playable-04-natural-spawn-missing-test/screenshots"),
            client
        );

        assertEquals("failed", report.result(), () -> String.join("\n", report.observations()));
        assertTrue(
            report.observations().stream().anyMatch(entry ->
                entry.contains("natural spawn acceptance: failed")
                    && entry.contains("passive_observed=false")
                    && entry.contains("hostile_observed=false")
            ),
            () -> String.join("\n", report.observations())
        );
    }

    @Test
    void fullLoopDefendsAgainstVisibleHostileDuringSurvivalSoak() throws Exception {
        FakeScenarioClient client = new FakeScenarioClient();
        client.visiblePassiveDuringSoak = true;
        client.visibleHostileDuringSoak = true;

        ClientScenarioReport report = new PlayableRealClientLoopScenario(Duration.ofMillis(1)).run(
            "playable-04-twenty-minute-survival-loop",
            Path.of("build/tmp/playable-04-hostile-test/screenshots"),
            client
        );

        assertEquals("passed", report.result(), () -> String.join("\n", report.observations()));
        assertTrue(
            client.operations.stream().anyMatch(operation -> operation.startsWith("attackEntityUntilRemoved:")),
            "the long survival gate must react to a visible hostile instead of remaining AFK"
        );
    }

    @Test
    void fullLoopContinuesAfterLocalLogsAreExhaustedOnceUsefulWorkWasProven() throws Exception {
        FakeScenarioClient client = new FakeScenarioClient();
        client.visiblePassiveDuringSoak = true;
        client.visibleHostileDuringSoak = true;
        client.logsUnavailableDuringSoakAfterTick = 60L;

        ClientScenarioReport report = new PlayableRealClientLoopScenario(Duration.ofSeconds(40)).run(
            "playable-04-twenty-minute-survival-loop",
            Path.of("build/tmp/playable-04-resource-exhaustion-test/screenshots"),
            client
        );

        assertEquals("passed", report.result(), () -> String.join("\n", report.observations()));
        assertTrue(
            report.observations().stream().anyMatch(entry ->
                entry.contains("survival resource work: exhausted")
                    && entry.contains("continuing_soak=true")
            ),
            () -> String.join("\n", report.observations())
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("natural spawn acceptance: passed")),
            "resource exhaustion must not bypass the passive+hostile natural-spawn gate"
        );
        assertFalse(
            client.operations.stream().anyMatch(operation -> operation.contains("far-natural-log")),
            "periodic survival work must remain nearby-only after initial natural progression"
        );
    }

    @Test
    void fullLoopDefendsAgainstVisibleSpiderDuringSurvivalSoak() throws Exception {
        FakeScenarioClient client = new FakeScenarioClient();
        client.visiblePassiveDuringSoak = true;
        client.visibleHostileDuringSoak = true;
        client.visibleHostileTypeDuringSoak = "minecraft:spider";

        ClientScenarioReport report = new PlayableRealClientLoopScenario(Duration.ofMillis(1)).run(
            "playable-04-twenty-minute-survival-loop",
            Path.of("build/tmp/playable-04-spider-test/screenshots"),
            client
        );

        assertEquals("passed", report.result(), () -> String.join("\n", report.observations()));
        assertTrue(
            client.operations.contains("attackEntityUntilRemoved:minecraft:spider:199"),
            "the long survival gate must defend against the spider that attacks the player"
        );
    }

    @Test
    void fullLoopObservesDistantHostileAtNightWithoutChargingAcrossTheWorld() throws Exception {
        FakeScenarioClient client = new FakeScenarioClient();
        client.visiblePassiveDuringSoak = true;
        client.clientTicks = 12_522L;
        client.visibleHostileOutsideReachDuringSoak = true;

        ClientScenarioReport report = new PlayableRealClientLoopScenario(Duration.ofMillis(1)).run(
            "playable-04-twenty-minute-survival-loop",
            Path.of("build/tmp/playable-04-distant-hostile-test/screenshots"),
            client
        );

        assertEquals("passed", report.result(), () -> String.join("\n", report.observations()));
        assertTrue(
            client.operations.contains("visibleEntity:minecraft:zombie|minecraft:skeleton|minecraft:spider:outside-survival-reach"),
            "the long survival gate must scan beyond melee reach once night begins"
        );
        assertFalse(
            client.operations.stream().anyMatch(operation -> operation.startsWith("attackEntityUntilRemoved:")),
            "the long survival gate must observe distant natural hostiles without suicidal long-range pursuit"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry ->
                entry.contains("outside_reach=true") && entry.contains("engagement=deferred")
            ),
            () -> String.join("\n", report.observations())
        );
    }

    @Test
    void fullLoopDoesNotChaseDistantHostileWhileBadlyInjured() throws Exception {
        FakeScenarioClient client = new FakeScenarioClient();
        client.visiblePassiveDuringSoak = true;
        client.clientTicks = 12_522L;
        client.healthAfterHostileCombat = 6.0F;
        client.visibleHostileOutsideReachDuringSoak = true;

        ClientScenarioReport report = new PlayableRealClientLoopScenario(Duration.ofMillis(1)).run(
            "playable-04-twenty-minute-survival-loop",
            Path.of("build/tmp/playable-04-injured-test/screenshots"),
            client
        );

        assertEquals("passed", report.result(), () -> String.join("\n", report.observations()));
        assertFalse(
            client.operations.stream().anyMatch(operation -> operation.startsWith("attackEntityUntilRemoved:")),
            "an injured survival player must not charge a hostile outside melee reach"
        );
    }

    @Test
    void fullLoopDefersHostileAlreadyWithinReachWhenBadlyInjured() throws Exception {
        FakeScenarioClient client = new FakeScenarioClient();
        client.visiblePassiveDuringSoak = true;
        client.visibleHostileDuringSoak = true;
        client.healthAfterHostileCombat = 6.0F;

        ClientScenarioReport report = new PlayableRealClientLoopScenario(Duration.ofMillis(1)).run(
            "playable-04-twenty-minute-survival-loop",
            Path.of("build/tmp/playable-04-injured-within-reach-test/screenshots"),
            client
        );

        assertEquals("passed", report.result(), () -> String.join("\n", report.observations()));
        assertFalse(
            client.operations.stream().anyMatch(operation -> operation.startsWith("attackEntityUntilRemoved:")),
            "a badly injured survival player must not start a fresh melee exchange"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry ->
                entry.contains("within_reach=true") && entry.contains("engagement=deferred_low_health")
            ),
            () -> String.join("\n", report.observations())
        );
    }

    @Test
    void fullLoopEngagesHostileAtInclusiveFifteenHealthBoundary() throws Exception {
        FakeScenarioClient client = new FakeScenarioClient();
        client.visiblePassiveDuringSoak = true;
        client.visibleHostileDuringSoak = true;
        client.healthAfterHostileCombat = 15.0F;

        ClientScenarioReport report = new PlayableRealClientLoopScenario(Duration.ofMillis(1)).run(
            "playable-04-twenty-minute-survival-loop",
            Path.of("build/tmp/playable-04-fifteen-health-boundary-test/screenshots"),
            client
        );

        assertEquals("passed", report.result(), () -> String.join("\n", report.observations()));
        assertTrue(
            client.operations.stream().anyMatch(operation -> operation.startsWith("attackEntityUntilRemoved:")),
            "exactly 15 health must remain inside the active-defense boundary"
        );
        assertFalse(
            report.observations().stream().anyMatch(entry -> entry.contains("engagement=deferred_low_health")),
            () -> String.join("\n", report.observations())
        );
    }

    @Test
    void survivalSoakTreatsVanillaNightAsUnsafeForLongResourceWork() {
        assertFalse(PlayableRealClientLoopScenario.isNightTime(12_541L));
        assertTrue(PlayableRealClientLoopScenario.isNightTime(12_542L));
        assertTrue(PlayableRealClientLoopScenario.isNightTime(23_999L));
        assertFalse(PlayableRealClientLoopScenario.isNightTime(24_000L));
        assertTrue(PlayableRealClientLoopScenario.isNightTime(36_542L));
    }

    @Test
    void fullLoopRecoversNaturalCombatDeathAndContinuesServerTimeSoak() throws Exception {
        FakeScenarioClient client = new FakeScenarioClient();
        client.visiblePassiveDuringSoak = true;
        client.visibleHostileDuringSoak = true;
        client.dieOnNextHostileAttack = true;

        ClientScenarioReport report = new PlayableRealClientLoopScenario(Duration.ofMillis(1_050)).run(
            "playable-04-twenty-minute-survival-loop",
            Path.of("build/tmp/playable-04-death-recovery-test/screenshots"),
            client
        );

        assertEquals("passed", report.result(), () -> String.join("\n", report.observations()));
        assertTrue(client.operations.contains("waitDeathScreen"));
        assertTrue(client.operations.contains("respawn"));
        assertTrue(
            client.operations.stream().anyMatch(operation -> operation.startsWith("collectIdentity:minecraft:wooden_pickaxe:")),
            "natural combat death must recover the exact wooden pickaxe death drop"
        );
        assertTrue(
            client.operations.stream().anyMatch(operation -> operation.startsWith("collectIdentity:minecraft:wooden_sword:")),
            "natural combat death must recover the exact wooden sword death drop"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry ->
                entry.contains("survival death-drop recovery: passed")
                    && entry.contains("pickaxe_recovered=true")
                    && entry.contains("sword_recovered=true")
            ),
            () -> String.join("\n", report.observations())
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("recovered_deaths=1")),
            () -> String.join("\n", report.observations())
        );
    }

    @Test
    void fullLoopDoesNotFoldDeathDropsIntoThePreDeathIdentityBaseline() throws Exception {
        FakeScenarioClient client = new FakeScenarioClient();
        client.visiblePassiveDuringSoak = true;
        client.visibleHostileDuringSoak = true;
        client.dieDuringDropBaselineAfterTick = 20L;

        ClientScenarioReport report = new PlayableRealClientLoopScenario(Duration.ofMillis(1_050)).run(
            "playable-04-twenty-minute-survival-loop",
            Path.of("build/tmp/playable-04-between-iterations-death-test/screenshots"),
            client
        );

        assertEquals("passed", report.result(), () -> String.join("\n", report.observations()));
        assertTrue(client.deathMaterializedDuringDropBaseline);
        assertTrue(
            client.operations.stream().anyMatch(operation ->
                operation.startsWith("waitNewDropIdentity:minecraft:wooden_pickaxe:")
                    && !operation.contains("entityId=701")
            ),
            "the exact death drop must not be captured into the pre-death exclusion baseline"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("survival death-drop recovery: passed")),
            () -> String.join("\n", report.observations())
        );
    }

    @Test
    void fullLoopFailsWhenServerTimePacketsStop() throws Exception {
        FakeScenarioClient client = new FakeScenarioClient();
        client.tickProgressDuringSoak = false;

        ClientScenarioReport report = new PlayableRealClientLoopScenario(Duration.ofMillis(1)).run(
            "playable-04-twenty-minute-survival-loop",
            Path.of("build/tmp/playable-04-server-time-stall-test/screenshots"),
            client
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("server_time_progress=false")),
            () -> String.join("\n", report.observations())
        );
    }

    @Test
    void logToPlanksUsesNaturalLogDropAndInventoryRecipeWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-02a-natural-log-to-planks",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "selected",
            "count:minecraft:oak_log",
            "count:minecraft:oak_planks",
            "count:minecraft:crafting_table",
            "count:minecraft:wooden_pickaxe",
            "findBreakable:" + SUPPORTED_LOGS + ":within-survival-reach",
            "approach:minecraft:oak_log:natural-log",
            "breakVisible:minecraft:oak_log:minecraft:oak_log",
            "collect:minecraft:oak_log:minecraft:oak_log:1",
            "count:minecraft:oak_log",
            "count:minecraft:oak_planks",
            "recipe:0:18:false",
            "waitCount:minecraft:oak_log:0",
            "waitCount:minecraft:oak_planks:4"
        ), client.operations);
        assertFalse(client.usedDebugSetup(), "wood-to-tool playable probe must not use debug setup helpers");
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("natural log break/drop/pickup: passed")),
            "wood-to-tool probe must record natural log break/drop/pickup evidence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("inventory recipe: passed")),
            "wood-to-tool probe must record inventory recipe evidence"
        );
    }

    @Test
    void logToPlanksWalksTowardLoadedNaturalLogBeforeBreakingIt() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.nearLogsAvailable = false;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-02a-natural-log-to-planks",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "selected",
            "count:minecraft:oak_log",
            "count:minecraft:oak_planks",
            "count:minecraft:crafting_table",
            "count:minecraft:wooden_pickaxe",
            "findBreakable:" + SUPPORTED_LOGS + ":within-survival-reach",
            "findBreakable:" + SUPPORTED_LOGS + ":outside-survival-reach",
            "approach:minecraft:oak_log:far-natural-log",
            "findBreakable:" + SUPPORTED_LOGS + ":within-survival-reach",
            "approach:minecraft:oak_log:natural-log",
            "breakVisible:minecraft:oak_log:minecraft:oak_log",
            "collect:minecraft:oak_log:minecraft:oak_log:1",
            "count:minecraft:oak_log",
            "count:minecraft:oak_planks",
            "recipe:0:18:false",
            "waitCount:minecraft:oak_log:0",
            "waitCount:minecraft:oak_planks:4"
        ), client.operations);
        assertFalse(client.usedDebugSetup(), "walking to a natural log must not use debug setup helpers");
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("natural log approach: passed")),
            "wood-to-tool probe must record natural movement toward the loaded log"
        );
    }

    @Test
    void logToPlanksUsesDetectedGeneratedLogFamily() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.logItemId = "minecraft:spruce_log";
        client.planksItemId = "minecraft:spruce_planks";

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-02a-natural-log-to-planks",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertTrue(client.operations.contains("breakVisible:minecraft:spruce_log:minecraft:spruce_log"));
        assertTrue(client.operations.contains("recipe:0:20:false"));
        assertTrue(client.operations.contains("waitCount:minecraft:spruce_log:0"));
        assertTrue(client.operations.contains("waitCount:minecraft:spruce_planks:4"));
        assertFalse(client.usedDebugSetup(), "non-oak generated logs must not fall back to debug setup");
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("planks_item=minecraft:spruce_planks")),
            "scenario must record the detected planks family"
        );
    }

    @Test
    void renewableWheatBreadCompletesWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-43-renewable-wheat-bread",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertFalse(client.usedDebugSetup(), "renewable food progression must not use debug setup");
        assertTrue(client.operations.contains("recipe:7:30:false"), "scenario must craft an earned wooden hoe");
        assertEquals(
            3,
            client.operations.stream().filter(operation -> operation.startsWith("findTillable:")).count()
        );
        assertEquals(
            3,
            client.operations.stream().filter(operation -> operation.endsWith(":age:7")).count(),
            "scenario must observe three mature crops"
        );
        assertEquals(
            3,
            report.observations().stream()
                .filter(entry -> entry.contains("wheat client light plot=") && entry.contains("sky=15"))
                .count(),
            "scenario must prove mature wheat has client-visible growth light"
        );
        assertTrue(client.operations.contains("recipe:7:60:false"), "scenario must craft bread from earned wheat");
        assertFalse(
            client.operations.contains("drainHungerBySprinting"),
            "crop progression must not make the visible client perform synthetic hunger-drain movement"
        );
        assertFalse(
            client.operations.stream().anyMatch(operation -> operation.startsWith("eatSelectedFood:minecraft:bread:")),
            "food consumption remains covered by the dedicated eating scenario"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("renewable bread ready: passed")),
            "scenario must record the earned bread left ready for normal play"
        );
    }

    @Test
    void renewableWheatAcceptsVanillaGrowthLightBelowOpenSkyMaximum() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.cropSkyLight = 14;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-43-renewable-wheat-bread",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result(), () -> String.join("\n", report.observations()));
        assertEquals(
            3,
            report.observations().stream()
                .filter(entry -> entry.contains("wheat client light plot=") && entry.contains("sky=14"))
                .count()
        );
    }

    @Test
    void woodToToolCraftsWoodenPickaxeInOpenedCraftingTableWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-02-natural-wood-to-tool",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "selected",
            "count:minecraft:oak_log",
            "count:minecraft:oak_planks",
            "count:minecraft:crafting_table",
            "count:minecraft:wooden_pickaxe",
            "findBreakable:" + SUPPORTED_LOGS + ":within-survival-reach",
            "approach:minecraft:oak_log:natural-log",
            "breakVisible:minecraft:oak_log:minecraft:oak_log",
            "collect:minecraft:oak_log:minecraft:oak_log:1",
            "findBreakable:minecraft:oak_log:within-survival-reach",
            "approach:minecraft:oak_log:natural-log",
            "breakVisible:minecraft:oak_log:minecraft:oak_log",
            "collect:minecraft:oak_log:minecraft:oak_log:1",
            "findBreakable:minecraft:oak_log:within-survival-reach",
            "approach:minecraft:oak_log:natural-log",
            "breakVisible:minecraft:oak_log:minecraft:oak_log",
            "collect:minecraft:oak_log:minecraft:oak_log:1",
            "count:minecraft:oak_log",
            "count:minecraft:oak_planks",
            "recipe:0:18:true",
            "waitCount:minecraft:oak_log:0",
            "waitCount:minecraft:oak_planks:12",
            "count:minecraft:oak_planks",
            "count:minecraft:crafting_table",
            "recipe:0:10:false",
            "waitCount:minecraft:oak_planks:8",
            "waitCount:minecraft:crafting_table:1",
            "selectHotbar:minecraft:crafting_table:1",
            "findDry:within-survival-reach",
            "use:minecraft:crafting_table:table-clicked",
            "waitBlock:table-target:minecraft:crafting_table",
            "use:minecraft:crafting_table:table-target",
            "screen:net.minecraft.client.gui.screens.inventory.CraftingScreen",
            "containerId",
            "count:minecraft:oak_planks",
            "count:minecraft:stick",
            "recipe:7:21:false",
            "waitCount:minecraft:oak_planks:6",
            "waitCount:minecraft:stick:4",
            "count:minecraft:oak_planks",
            "count:minecraft:stick",
            "count:minecraft:wooden_pickaxe",
            "recipe:7:31:false",
            "waitCount:minecraft:oak_planks:3",
            "waitCount:minecraft:stick:2",
            "waitCount:minecraft:wooden_pickaxe:1",
            "closeScreen"
        ), client.operations);
        assertFalse(client.usedDebugSetup(), "wood-to-tool probe must not fall back to debug setup");
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("wooden pickaxe recipe: passed")),
            "wood-to-tool probe must record wooden pickaxe recipe evidence"
        );
    }

    @Test
    void stoneToolProgressionMinesCobblestoneAndCraftsStonePickaxeWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-05-stone-tool-progression",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertFalse(client.usedDebugSetup(), "stone progression must not fall back to debug setup");
        assertTrue(
            client.operations.contains("selectHotbar:minecraft:wooden_pickaxe:1"),
            "stone progression must use the earned wooden pickaxe"
        );
        assertTrue(
            client.operations.contains("approach:minecraft:crafting_table:table-target"),
            "stone progression must walk back into reach of the earned crafting table before reopening it"
        );
        assertTrue(
            client.operations.contains("breakVisible:minecraft:stone:minecraft:cobblestone"),
            "stone progression must break natural stone for cobblestone drops"
        );
        assertTrue(
            client.operations.contains("recipe:7:24:false"),
            "stone progression must use the embedded stone pickaxe recipe id"
        );
        assertTrue(
            client.operations.contains("waitCount:minecraft:stone_pickaxe:1"),
            "stone progression must wait for the crafted stone pickaxe"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("stone break/drop/pickup: passed")),
            "stone progression must record cobblestone mining evidence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("stone pickaxe recipe: passed")),
            "stone progression must record stone pickaxe crafting evidence"
        );
    }

    @Test
    void stoneToolProgressionRetriesMissedCobblestonePickupUntilInventoryTarget() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.failedCobblestonePickupsRemaining = 1;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-05-stone-tool-progression",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("stone break/drop/pickup: failed")),
            "stone progression should record a missed pickup before retrying"
        );
        assertTrue(
            client.operations.stream()
                    .filter("collect:minecraft:stone:minecraft:cobblestone:1"::equals)
                    .count() >= 4,
            "one missed cobblestone pickup should cause one extra natural stone mining attempt"
        );
    }

    @Test
    void stoneToolProgressionRetriesReachableStoneAfterFailedFarApproach() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.missingReachableStoneScansRemaining = 1;
        client.failFirstNaturalStoneFarApproach = true;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-05-stone-tool-progression",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("natural stone approach: failed")),
            "stone progression should record the failed far-stone approach"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry ->
                entry.contains("natural stone reachable fallback after failed approach: passed")
            ),
            "stone progression should retry a reachable stone after the failed far approach"
        );
        assertTrue(
            client.operations.contains("breakVisible:minecraft:stone:minecraft:cobblestone"),
            "stone progression should still mine cobblestone after the reachable fallback"
        );
    }

    @Test
    void stoneToolRestartBeforePersistsCraftedTableMarkerAndStonePickaxeWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();
        Path screenshotsDir = Path.of("build/tmp/playable-06-test/screenshots");

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-06-stone-tool-save-restart-before",
            screenshotsDir,
            client
        );

        assertEquals("passed", report.result());
        assertFalse(client.usedDebugSetup(), "stone restart before phase must not use debug setup");
        assertTrue(
            screenshotsDir.getParent().resolve("playable-03-save-restart-marker.properties").toFile().isFile(),
            "stone restart before phase must write the crafted table marker for after phase"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("stone pickaxe recipe: passed")),
            "stone restart before phase must craft the stone pickaxe before restart"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("restart marker placement: passed")),
            "stone restart before phase must record marker placement"
        );
    }

    @Test
    void stoneToolRestartAfterChecksPersistedMarkerAndStonePickaxeInventory() {
        FakeScenarioClient client = new FakeScenarioClient();
        Path screenshotsDir = Path.of("build/tmp/playable-06-test/screenshots");
        new PlayableRealClientLoopScenario().run(
            "playable-06-stone-tool-save-restart-before",
            screenshotsDir,
            client
        );
        client.operations.clear();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-06-stone-tool-save-restart-after",
            screenshotsDir,
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "selected",
            "count:minecraft:oak_log",
            "count:minecraft:oak_planks",
            "count:minecraft:crafting_table",
            "count:minecraft:wooden_pickaxe",
            "waitBlock:restart-marker:minecraft:crafting_table",
            "count:minecraft:stone_pickaxe"
        ), client.operations);
        assertFalse(client.usedDebugSetup(), "stone restart after phase must not use debug setup");
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("restart marker persistence: passed")),
            "stone restart after phase must record persisted marker observation"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("stone inventory persistence: passed")),
            "stone restart after phase must record persisted stone pickaxe inventory observation"
        );
    }

    @Test
    void furnacePlacementCraftsPlacesAndOpensEarnedFurnaceWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-07-furnace-placement-open",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertFalse(client.usedDebugSetup(), "furnace placement must not use debug setup");
        assertTrue(
            client.operations.contains("recipe:7:13:false"),
            "furnace placement must use the embedded furnace recipe id"
        );
        assertTrue(
            client.operations.contains("waitCount:minecraft:furnace:1"),
            "furnace placement must wait for the crafted furnace item"
        );
        assertTrue(
            client.operations.contains("use:minecraft:furnace:furnace-clicked"),
            "furnace placement must place the earned furnace item"
        );
        assertTrue(
            client.operations.contains("waitBlock:furnace-target:minecraft:furnace"),
            "furnace placement must wait for the furnace world block"
        );
        assertTrue(
            client.operations.contains("screen:net.minecraft.client.gui.screens.inventory.FurnaceScreen"),
            "furnace placement must open the furnace UI"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("furnace recipe: passed")),
            "furnace placement must record furnace recipe evidence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("furnace open: passed")),
            "furnace placement must record placed/opened furnace evidence"
        );
    }

    @Test
    void furnaceCharcoalSmeltsEarnedLogWithWoodFuelWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-08-furnace-charcoal-smelt",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertFalse(client.usedDebugSetup(), "furnace charcoal smelt must not use debug setup");
        assertTrue(
            client.operations.contains("breakVisible:minecraft:oak_log:minecraft:oak_log"),
            "charcoal smelt must collect an extra natural log as furnace input"
        );
        assertTrue(
            client.operations.contains("moveToContainer:0:minecraft:oak_log:1"),
            "charcoal smelt must move the earned log into furnace input slot 0"
        );
        assertTrue(
            client.operations.contains("moveToContainer:1:minecraft:oak_planks:1"),
            "charcoal smelt must move earned planks into furnace fuel slot 1"
        );
        assertTrue(
            client.operations.contains("moveFromContainer:1:minecraft:oak_planks:1"),
            "charcoal smelt must clear leftover planks from furnace fuel slot 1"
        );
        assertTrue(
            client.operations.contains("waitContainer:2:minecraft:charcoal:1"),
            "charcoal smelt must wait for furnace output slot 2"
        );
        assertTrue(
            client.operations.contains("moveFromContainer:2:minecraft:charcoal:1"),
            "charcoal smelt must move the cooked charcoal into inventory"
        );
        assertTrue(
            client.operations.contains("waitCount:minecraft:charcoal:1"),
            "charcoal smelt must wait for charcoal inventory convergence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("furnace input transfer: passed")),
            "charcoal smelt must record input transfer evidence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("furnace charcoal output: passed")),
            "charcoal smelt must record cooked output evidence"
        );
    }

    @Test
    void furnaceCharcoalSmeltStillBreaksReachableInputLogWhenCloseApproachFails() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.failPostFurnaceLogApproach = true;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-08-furnace-charcoal-smelt",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry ->
                entry.contains("furnace input log close approach: failed")
            ),
            "scenario must record that movement toward the already reachable input log did not improve position"
        );
        assertTrue(
            client.operations.contains("breakVisible:minecraft:oak_log:minecraft:oak_log"),
            "scenario must still attempt the reachable log break after close-approach failure"
        );
        assertFalse(client.usedDebugSetup(), "reachable fallback must not use debug setup");
    }

    @Test
    void logToPlanksRetriesReachableLogAfterFarApproachFailure() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.nearLogsAvailable = false;
        client.failFirstNaturalLogFarApproach = true;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-02a-natural-log-to-planks",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            report.observations().stream().anyMatch(entry ->
                entry.contains("natural log approach: failed")
            ),
            "scenario must record the failed far-log approach"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry ->
                entry.contains("natural log reachable fallback after failed approach: passed")
            ),
            "scenario must retry a reachable natural log after the failed far approach"
        );
        assertTrue(
            client.operations.contains("breakVisible:minecraft:oak_log:minecraft:oak_log"),
            "scenario must still break a natural log through the normal client path"
        );
        assertFalse(client.usedDebugSetup(), "reachable fallback must not use debug setup");
    }

    @Test
    void torchCraftPlaceUsesCookedCharcoalAndPlacesEarnedTorchWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-09-torch-craft-place",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertFalse(client.usedDebugSetup(), "torch craft/place must not use debug setup");
        assertTrue(
            client.operations.contains("waitCount:minecraft:charcoal:1"),
            "torch craft/place must reuse the cooked charcoal from the furnace path"
        );
        assertTrue(
            client.operations.contains("recipe:0:27:false"),
            "torch craft/place must use the embedded torch recipe id"
        );
        assertTrue(
            client.operations.contains("waitCount:minecraft:charcoal:0"),
            "torch craft/place must consume one charcoal"
        );
        assertTrue(
            client.operations.contains("waitCount:minecraft:stick:1"),
            "torch craft/place must consume one earned stick"
        );
        assertTrue(
            client.operations.contains("waitCount:minecraft:torch:4"),
            "torch craft/place must create four torches"
        );
        assertTrue(
            client.operations.contains("selectHotbar:minecraft:torch:4"),
            "torch craft/place must select the earned torches from hotbar"
        );
        assertTrue(
            client.operations.contains("use:minecraft:torch:torch-clicked"),
            "torch craft/place must place the earned torch item"
        );
        assertTrue(
            client.operations.contains("waitBlock:torch-target:minecraft:torch"),
            "torch craft/place must wait for the torch world block"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("torch recipe: passed")),
            "torch craft/place must record torch recipe evidence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("torch placement: passed")),
            "torch craft/place must record placed torch evidence"
        );
    }

    @Test
    void passiveFoodDropKillsNaturalAnimalAndCollectsFoodWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-10-passive-food-drop",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertFalse(client.usedDebugSetup(), "passive food path must not use debug setup");
        assertTrue(
            client.operations.contains("findEntity:minecraft:cow|minecraft:pig|minecraft:chicken:outside-survival-reach"),
            "passive food path must scan for a naturally loaded passive mob"
        );
        assertTrue(
            client.operations.contains("approachEntity:minecraft:cow:loaded-passive"),
            "passive food path must approach the natural mob through movement"
        );
        assertTrue(
            client.operations.contains("attackEntityDrop:minecraft:cow:entity_id=42:minecraft:beef:1"),
            "passive food path must attack the natural mob until its food drop is collected"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("passive food drop: passed")),
            "passive food path must record natural mob kill/drop/pickup evidence"
        );
    }

    @Test
    void eatPassiveFoodUsesEarnedDropAfterNaturalHungerDrainWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-11-eat-passive-food",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertFalse(client.usedDebugSetup(), "passive food eating path must not use debug setup");
        assertTrue(
            client.operations.contains("drainHungerBySprinting"),
            "passive food eating path must create a natural hunger deficit through movement"
        );
        assertTrue(
            client.operations.contains("selectHotbar:minecraft:beef:1"),
            "passive food eating path must select the earned food drop"
        );
        assertTrue(
            client.operations.contains("eatSelectedFood:minecraft:beef:1"),
            "passive food eating path must hold use until the earned food is consumed"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("passive food eating: passed")),
            "passive food eating path must record consumed earned food evidence"
        );
    }

    @Test
    void cookedPassiveFoodSmeltsRawDropAndEatsCookedMealWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-15-cooked-passive-food",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertFalse(client.usedDebugSetup(), "cooked passive food path must not use debug setup");
        assertTrue(
            client.operations.contains("waitCount:minecraft:charcoal:1"),
            "cooked passive food path must prepare earned charcoal fuel"
        );
        assertTrue(
            client.operations.indexOf("attackEntityDrop:minecraft:cow:entity_id=42:minecraft:beef:1")
                < client.operations.indexOf("waitCount:minecraft:charcoal:1"),
            "cooked passive food path must collect raw food before the long furnace/stone route moves away from animals"
        );
        assertTrue(
            client.operations.indexOf("waitCount:minecraft:wooden_pickaxe:1")
                < client.operations.indexOf("attackEntityDrop:minecraft:cow:entity_id=42:minecraft:beef:1"),
            "cooked passive food path must finish the initial wood/tool route before chasing animals"
        );
        assertTrue(
            client.operations.indexOf("attackEntityDrop:minecraft:cow:entity_id=42:minecraft:beef:1")
                < client.operations.indexOf("breakVisible:minecraft:stone:minecraft:cobblestone"),
            "cooked passive food path must collect raw food before stone mining moves away from nearby animals"
        );
        assertTrue(
            client.operations.contains("attackEntityDrop:minecraft:cow:entity_id=42:minecraft:beef:1"),
            "cooked passive food path must collect a raw passive food drop"
        );
        assertTrue(
            client.operations.contains("moveToContainer:0:minecraft:beef:1"),
            "cooked passive food path must move raw food into furnace input slot 0"
        );
        assertTrue(
            client.operations.contains("moveToContainer:1:minecraft:charcoal:1"),
            "cooked passive food path must move earned charcoal into furnace fuel slot 1"
        );
        assertTrue(
            client.operations.contains("waitContainer:2:minecraft:cooked_beef:1"),
            "cooked passive food path must wait for cooked food in furnace output slot 2"
        );
        assertTrue(
            client.operations.contains("moveFromContainer:2:minecraft:cooked_beef:1"),
            "cooked passive food path must move cooked food into inventory"
        );
        assertTrue(
            client.operations.contains("selectHotbar:minecraft:cooked_beef:1"),
            "cooked passive food path must select the cooked food from hotbar"
        );
        assertTrue(
            client.operations.contains("eatSelectedFood:minecraft:cooked_beef:1"),
            "cooked passive food path must eat the cooked food after natural hunger drain"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("cooked passive food eating: passed")),
            "cooked passive food path must record consumed cooked food evidence"
        );
    }

    @Test
    void cookedPassiveFoodCarriesSpareTableWhenOriginalTableIsUnreachableAfterStoneMining() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.failCraftingTableApproachForFurnace = true;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-15-cooked-passive-food",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertFalse(client.usedDebugSetup(), "spare crafting table fallback must not use debug setup");
        assertTrue(
            client.operations.stream().filter("recipe:0:10:false"::equals).count() >= 2,
            "fallback must craft a spare crafting table from carried planks"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry ->
                entry.contains("crafting table approach for furnace: failed")
            ),
            "scenario must record the failed old-table approach before using the spare table"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("furnace recipe: passed")),
            "spare table fallback must still craft the furnace"
        );
    }

    @Test
    void earnedChestStorageCraftsChestAndDepositsEarnedFoodWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-12-earned-chest-storage",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertFalse(client.usedDebugSetup(), "earned chest storage path must not use debug setup");
        assertTrue(
            client.operations.contains("recipe:7:5:false"),
            "earned chest storage path must craft chest from earned planks in the crafting table"
        );
        assertTrue(
            client.operations.contains("selectHotbar:minecraft:chest:1"),
            "earned chest storage path must select the earned chest"
        );
        assertTrue(
            client.operations.contains("use:minecraft:chest:chest-clicked"),
            "earned chest storage path must place/open the earned chest"
        );
        assertTrue(
            client.operations.contains("attackEntityDrop:minecraft:cow:entity_id=42:minecraft:beef:1"),
            "earned chest storage path must collect an earned passive food item for storage"
        );
        assertTrue(
            client.operations.contains("moveToContainer:0:minecraft:beef:1"),
            "earned chest storage path must deposit an earned item into the chest"
        );
        assertTrue(
            client.operations.contains("waitContainer:0:minecraft:beef:1"),
            "earned chest storage path must observe the deposited item in the chest slot"
        );
        assertFalse(
            client.operations.contains("approach:minecraft:chest:chest-target"),
            "earned chest storage path should place the chest near the earned item instead of returning across terrain"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("earned chest storage: passed")),
            "earned chest storage path must record container storage evidence"
        );
    }

    @Test
    void earnedChestStorageDoesNotRequireFourthUpperLog() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.fourthLogIsReachableDownFace = true;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-12-earned-chest-storage",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertFalse(
            client.operations.contains("approach:minecraft:oak_log:upper-natural-log"),
            "earned chest storage should not depend on a brittle fourth upper log"
        );
    }

    @Test
    void chestStorageSaveRestartBeforeWritesChestMarkerWithoutDebugSetup() throws Exception {
        Path runDir = Files.createTempDirectory("playable-13-before");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-13-chest-storage-save-restart-before",
            screenshotsDir,
            client
        );

        assertEquals("passed", report.result());
        assertFalse(client.usedDebugSetup(), "chest save/restart before phase must not use debug setup");
        assertTrue(
            Files.isRegularFile(runDir.resolve("playable-13-chest-storage-marker.properties")),
            "before phase must write a chest marker for the runner-managed after phase"
        );
        assertTrue(
            client.operations.contains("moveToContainer:0:minecraft:beef:1"),
            "before phase must store the earned item in the chest before restart"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("runner-managed restart: pending")),
            "before phase must record that the runner owns the restart boundary"
        );
    }

    @Test
    void chestStorageSaveRestartBeforeContinuesWhenStoredSlotIsConfirmedButScreenCloseIsPending()
        throws Exception {
        Path runDir = Files.createTempDirectory("playable-13-before-close-pending");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient client = new FakeScenarioClient();
        client.failCloseCurrentScreen = true;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-13-chest-storage-save-restart-before",
            screenshotsDir,
            client
        );

        assertEquals("passed", report.result());
        assertTrue(
            Files.isRegularFile(runDir.resolve("playable-13-chest-storage-marker.properties")),
            "before phase must write the marker once the stored chest slot is confirmed"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("slot_matched=true")),
            "before phase must keep the storage evidence even when close remains pending"
        );
    }

    @Test
    void chestStorageSaveRestartAfterVerifiesPersistedChestSlot() throws Exception {
        Path runDir = Files.createTempDirectory("playable-13-after");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient client = new FakeScenarioClient();
        ClientScenarioReport before = new PlayableRealClientLoopScenario().run(
            "playable-13-chest-storage-save-restart-before",
            screenshotsDir,
            client
        );
        assertEquals("passed", before.result());
        client.operations.clear();

        ClientScenarioReport after = new PlayableRealClientLoopScenario().run(
            "playable-13-chest-storage-save-restart-after",
            screenshotsDir,
            client
        );

        assertEquals("passed", after.result());
        assertTrue(
            client.operations.contains("waitBlock:chest-marker:minecraft:chest"),
            "after phase must verify the persisted chest block"
        );
        assertTrue(
            client.operations.contains("waitContainer:0:minecraft:beef:1"),
            "after phase must verify the persisted stored item in chest slot 0"
        );
        assertTrue(
            after.observations().stream().anyMatch(entry -> entry.contains("chest storage persistence: passed")),
            "after phase must record chest slot persistence evidence"
        );
    }

    @Test
    void chestStorageSaveRestartAfterFailsClosedWithoutMarker() {
        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-13-chest-storage-save-restart-after",
            Path.of("build/tmp/playable-13-missing/screenshots"),
            new FakeScenarioClient()
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("missing chest storage marker")),
            "after phase must fail closed when the chest marker is absent"
        );
    }

    @Test
    void earnedBedSleepCraftsWhiteBedAndSkipsNightWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-14-earned-bed-sleep",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertFalse(client.usedDebugSetup(), "earned bed sleep path must not use debug setup");
        assertTrue(
            client.operations.contains("findEntity:minecraft:sheep:outside-survival-reach"),
            "earned bed path must scan for natural sheep as the wool source"
        );
        assertTrue(
            client.operations.contains("attackEntityDrop:minecraft:sheep:entity_id=43:minecraft:white_wool:1"),
            "earned bed path must collect wool through a natural sheep drop"
        );
        assertTrue(
            client.operations.contains("recipe:7:34:false"),
            "earned bed path must use the appended embedded white-bed recipe display id"
        );
        assertTrue(
            client.operations.contains("selectHotbar:minecraft:white_bed:1"),
            "earned bed path must select the crafted bed"
        );
        assertTrue(
            client.operations.contains("use:minecraft:white_bed:bed-clicked"),
            "earned bed path must place the crafted bed"
        );
        assertTrue(
            client.operations.contains("waitNight"),
            "earned bed path must wait for natural night instead of setting time by command"
        );
        assertTrue(
            client.operations.contains("waitMorning"),
            "earned bed path must observe the server time skip back to morning"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("bed sleep skip: passed")),
            "earned bed path must record night-to-morning sleep evidence"
        );
    }

    @Test
    void earnedBedSleepSkipsObservedBlackSheepForMatchingWhiteBedWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.firstObservedSheepWoolItemId = "minecraft:black_wool";

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-14-earned-bed-sleep",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result(), () -> String.join("\n", report.observations()));
        assertFalse(client.usedDebugSetup(), "black sheep fallback must not use debug setup");
        assertTrue(
            client.operations.contains("findSheepWool:minecraft:white_wool:outside-survival-reach"),
            "earned bed path must select only sheep with white wool after observing another color"
        );
        assertTrue(
            client.operations.contains("attackEntityDrop:minecraft:sheep:entity_id=100:minecraft:white_wool:1"),
            "earned bed path must collect three matching white wool drops"
        );
        assertEquals(
            3L,
            client.operations.stream()
                .filter(operation -> operation.startsWith("attackEntityDrop:minecraft:sheep:"))
                .count(),
            "earned bed path must collect exactly three matching white wool drops"
        );
        assertTrue(
            client.operations.contains("recipe:7:34:false"),
            "earned bed path must use the normal recipe for the matching white bed"
        );
        assertTrue(
            client.operations.contains("selectHotbar:minecraft:white_bed:1"),
            "earned bed path must select the crafted matching bed"
        );
        assertFalse(
            client.operations.stream().anyMatch(operation ->
                operation.startsWith("attackEntityDrop:") && operation.contains(":entity_id=43:")
            ),
            "earned bed path must not attack sheep whose wool cannot craft the selected bed"
        );
        for (int entityId : List.of(100, 101, 102)) {
            assertTrue(
                client.operations.contains(
                    "attackEntityDrop:minecraft:sheep:entity_id="
                        + entityId
                        + ":minecraft:white_wool:1"
                ),
                "earned bed path must attack selected white sheep entity_id=" + entityId
            );
        }
        int thirdWhiteAttack = client.operations.indexOf(
            "attackEntityDrop:minecraft:sheep:entity_id=102:minecraft:white_wool:1"
        );
        int stableRecipeSubmission = client.operations.indexOf("recipe:7:34:false");
        assertTrue(
            thirdWhiteAttack < stableRecipeSubmission,
            "stable white-bed recipe display id must be submitted only after three white wool drops"
        );
        assertFalse(
            client.operations.stream().anyMatch(operation -> operation.startsWith("findBedRecipe:")),
            "earned bed path must not depend on dynamic recipe-book lookup"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry ->
                entry.contains("sheep wool scan: skipped") && entry.contains("wool_item=minecraft:black_wool")
            ),
            "earned bed path must record the observed black sheep metadata before selecting white wool"
        );
        assertFalse(
            client.operations.stream().anyMatch(operation -> operation.startsWith("wait_ticks:")),
            "earned bed color handling must not use tick guesses"
        );
    }

    @Test
    void earnedDoorCraftsPlacesAndTogglesMatchingWoodenDoorWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.logItemId = "minecraft:birch_log";
        client.planksItemId = "minecraft:birch_planks";

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-16-earned-door-place-toggle",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertFalse(client.usedDebugSetup(), "earned door path must not use debug setup");
        assertTrue(
            client.operations.contains("recipe:7:36:false"),
            "earned door path must use the matching birch door recipe display id"
        );
        assertTrue(
            client.operations.contains("waitCount:minecraft:birch_door:3"),
            "earned door path must wait for the crafted birch door stack"
        );
        assertTrue(
            client.operations.contains("selectHotbar:minecraft:birch_door:1"),
            "earned door path must select the matching crafted door"
        );
        assertTrue(
            client.operations.contains("use:minecraft:birch_door:door-clicked"),
            "earned door path must place the crafted door"
        );
        assertTrue(
            client.operations.contains("waitBlock:door-target:minecraft:birch_door"),
            "earned door path must wait for the lower door block"
        );
        assertTrue(
            client.operations.stream().filter("use:minecraft:birch_door:door-target"::equals).count() >= 2,
            "earned door path must use the placed door twice to prove open and close interactions"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("door recipe: passed")),
            "earned door path must record matching door recipe evidence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("door toggle close: passed")),
            "earned door path must record open/close toggle evidence"
        );
    }

    @Test
    void earnedSignCraftsPlacesAndEditsMatchingWoodenSignWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.logItemId = "minecraft:birch_log";
        client.planksItemId = "minecraft:birch_planks";

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-17-earned-sign-place-edit",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertFalse(client.usedDebugSetup(), "earned sign path must not use debug setup");
        assertTrue(
            client.operations.contains("recipe:7:21:false"),
            "earned sign path must craft sticks from earned planks in the opened table"
        );
        assertTrue(
            client.operations.contains("recipe:7:45:false"),
            "earned sign path must use the matching birch sign recipe display id"
        );
        assertTrue(
            client.operations.contains("waitCount:minecraft:birch_sign:3"),
            "earned sign path must wait for the crafted birch sign stack"
        );
        assertTrue(
            client.operations.contains("selectHotbar:minecraft:birch_sign:1"),
            "earned sign path must select the matching crafted sign"
        );
        assertTrue(
            client.operations.contains("use:minecraft:birch_sign:sign-clicked"),
            "earned sign path must place the crafted sign"
        );
        assertTrue(
            client.operations.contains("waitBlock:sign-target:minecraft:birch_sign"),
            "earned sign path must wait for the placed sign block"
        );
        assertTrue(
            client.operations.contains("signEditor:sign-target"),
            "earned sign path must observe the vanilla sign editor"
        );
        assertTrue(
            client.operations.contains("signText:sign-target:Solaris|P17|NoDebug|OK"),
            "earned sign path must update the placed sign text through the client packet"
        );
        assertTrue(
            client.operations.contains("waitSignText:sign-target:Solaris|P17|NoDebug|OK"),
            "earned sign path must observe the edited sign text"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("sign recipe: passed")),
            "earned sign path must record matching sign recipe evidence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("sign text update: passed")),
            "earned sign path must record sign text update evidence"
        );
    }

    @Test
    void earnedCampfireCooksPassiveFoodWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.logItemId = "minecraft:birch_log";
        client.planksItemId = "minecraft:birch_planks";

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-18-earned-campfire-cooking",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertFalse(client.usedDebugSetup(), "earned campfire path must not use debug setup");
        assertTrue(
            client.operations.contains("recipe:7:53:false"),
            "earned campfire path must craft campfire with the appended embedded recipe display id"
        );
        assertTrue(
            client.operations.contains("moveFromContainer:0:minecraft:birch_log:1"),
            "earned campfire path must return furnace input log remainder before crafting the campfire"
        );
        assertTrue(
            client.operations.contains("waitCount:minecraft:campfire:1"),
            "earned campfire path must wait for crafted campfire inventory"
        );
        assertTrue(
            client.operations.contains("selectHotbar:minecraft:campfire:1"),
            "earned campfire path must select the crafted campfire"
        );
        assertTrue(
            client.operations.contains("use:minecraft:campfire:campfire-clicked"),
            "earned campfire path must place the crafted campfire"
        );
        assertTrue(
            client.operations.contains("waitBlock:campfire-target:minecraft:campfire"),
            "earned campfire path must observe the placed campfire block"
        );
        assertTrue(
            client.operations.contains("selectHotbar:minecraft:beef:1"),
            "earned campfire path must select the raw passive food drop"
        );
        assertTrue(
            client.operations.contains("use:minecraft:beef:campfire-target"),
            "earned campfire path must use raw food on the placed campfire"
        );
        assertTrue(
            client.operations.contains("waitDrop:minecraft:cooked_beef:campfire-target"),
            "earned campfire path must wait for cooked item entity output"
        );
        assertTrue(
            client.operations.contains("collect:minecraft:campfire:minecraft:cooked_beef:1"),
            "earned campfire path must collect the campfire cooked output"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("campfire recipe: passed")),
            "earned campfire path must record campfire recipe evidence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("campfire cooking output: passed")),
            "earned campfire path must record cooked item output evidence"
        );
    }

    @Test
    void earnedCampfirePlacesSpareCraftingTableWhenOriginalTableIsOutOfReach() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.logItemId = "minecraft:birch_log";
        client.planksItemId = "minecraft:birch_planks";
        client.failCraftingTableApproachForCampfire = true;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-18-earned-campfire-cooking",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            client.operations.stream().filter("recipe:0:10:false"::equals).count() >= 2,
            "campfire path must craft a spare table from earned planks when the original table cannot be reached"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("crafting table approach for campfire: failed")),
            "campfire path must record the failed original-table approach"
        );
        assertTrue(
            client.operations.contains("recipe:7:53:false"),
            "campfire path must still craft the campfire after opening the spare table"
        );
    }

    @Test
    void earnedCampfireRetriesReachableReserveLogAfterFarApproachFailure() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.logItemId = "minecraft:birch_log";
        client.planksItemId = "minecraft:birch_planks";
        client.failFirstCampfireReserveLogApproach = true;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-18-earned-campfire-cooking",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            report.observations().stream().anyMatch(entry ->
                entry.contains("campfire reserve log approach: failed")
            ),
            "scenario must record the failed far-log approach"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry ->
                entry.contains("campfire reserve log reachable fallback after failed approach: passed")
            ),
            "scenario must retry a reachable log after the failed far approach"
        );
    }

    @Test
    void earnedCampfireDeathRespawnsWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.logItemId = "minecraft:birch_log";
        client.planksItemId = "minecraft:birch_planks";

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-19-earned-campfire-death-respawn",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertFalse(client.usedDebugSetup(), "campfire death/respawn path must not use debug setup");
        assertTrue(
            client.operations.contains("standOnBlockUntilDeath:campfire-target:minecraft:campfire"),
            "campfire death/respawn path must wait for natural campfire contact damage"
        );
        assertTrue(
            client.operations.contains("respawn"),
            "campfire death/respawn path must perform a vanilla respawn packet"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("campfire hazard death: passed")),
            "campfire death/respawn path must record death-screen evidence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("campfire respawn: passed")),
            "campfire death/respawn path must record respawn evidence"
        );
    }

    @Test
    void campfireDeathDropRecoveryReturnsEarnedItemAfterRespawn() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.logItemId = "minecraft:birch_log";
        client.planksItemId = "minecraft:birch_planks";

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-20-campfire-death-drop-recovery",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertFalse(client.usedDebugSetup(), "campfire death-drop recovery must not use debug setup");
        assertTrue(
            client.operations.contains("standOnBlockUntilDeath:campfire-target:minecraft:campfire"),
            "death-drop recovery must still use natural campfire contact damage"
        );
        assertTrue(
            client.operations.contains("respawn"),
            "death-drop recovery must perform vanilla respawn before pickup"
        );
        assertTrue(
            client.operations.contains("approach:minecraft:campfire:campfire-target"),
            "death-drop recovery must walk back toward the death site after respawn"
        );
        assertTrue(
            client.operations.contains(
                "collectIdentity:minecraft:wooden_pickaxe:701:00000000-0000-0000-0000-000000000701"
            ),
            "death-drop recovery must collect the wooden-pickaxe entity observed after death"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("campfire death-drop recovery: passed")),
            "death-drop recovery must record pickup evidence"
        );
    }

    @Test
    void campfireDeathDropRecoveryRejectsInventoryGainWithoutPickupVisibility() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.logItemId = "minecraft:birch_log";
        client.planksItemId = "minecraft:birch_planks";
        client.woodenPickaxePickupObserved = false;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-20-campfire-death-drop-recovery",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry ->
                entry.contains("campfire death-drop recovery: failed")
                    && entry.contains("pickup_visible=false")
            ),
            "recovery must require visible wooden-pickaxe pickup evidence, not just an inventory increase"
        );
    }

    @Test
    void campfireDeathDropRecoveryFailsWithoutPostDeathItemEntityVisibility() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.logItemId = "minecraft:birch_log";
        client.planksItemId = "minecraft:birch_planks";
        client.postDeathWoodenPickaxeEntityVisible = false;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-20-campfire-death-drop-recovery",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry ->
                entry.contains("campfire wooden pickaxe death drop: failed")
                    && entry.contains("identity=missing")
            ),
            "recovery must fail when no new wooden-pickaxe entity is observed after death"
        );
    }

    @Test
    void campfireDeathDropRecoveryFailsWhenMatchedItemEntityDoesNotDisappear() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.logItemId = "minecraft:birch_log";
        client.planksItemId = "minecraft:birch_planks";
        client.woodenPickaxeEntityDisappeared = false;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-20-campfire-death-drop-recovery",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry ->
                entry.contains("campfire death-drop recovery: failed")
                    && entry.contains("pickup_disappeared=false")
            ),
            "recovery must require disappearance of the matched post-death entity"
        );
    }

    @Test
    void campfireDeathDropRecoveryRejectsPreexistingItemEntityIdentity() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.logItemId = "minecraft:birch_log";
        client.planksItemId = "minecraft:birch_planks";
        client.preexistingWoodenPickaxeEntityId = 700;
        client.returnPreexistingWoodenPickaxeEntity = true;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-20-campfire-death-drop-recovery",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry ->
                entry.contains("campfire wooden pickaxe death drop: failed")
                    && entry.contains("entityId=700")
                    && entry.contains("uuid=00000000-0000-0000-0000-000000000700")
            ),
            "recovery must not bind to a wooden-pickaxe entity visible before death"
        );
    }

    @Test
    void campfireDeathDropRecoveryDistinguishesReusedEntityIdByUuid() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.logItemId = "minecraft:birch_log";
        client.planksItemId = "minecraft:birch_planks";
        client.preexistingWoodenPickaxeEntityId = 701;
        client.deathDropWoodenPickaxeEntityId = 701;
        client.preexistingWoodenPickaxeEntityUuid = UUID.fromString("00000000-0000-0000-0000-000000000700");
        client.deathDropWoodenPickaxeEntityUuid = UUID.fromString("00000000-0000-0000-0000-000000000701");

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-20-campfire-death-drop-recovery",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            client.operations.contains(
                "collectIdentity:minecraft:wooden_pickaxe:701:00000000-0000-0000-0000-000000000701"
            ),
            "collection must bind to the post-death UUID when the numeric entity id is reused"
        );
    }

    @Test
    void campfireDeathDropRecoveryRejectsIdentityLossPlusUnrelatedPickup() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.logItemId = "minecraft:birch_log";
        client.planksItemId = "minecraft:birch_planks";
        client.woodenPickaxeIdentityLostBeforePickup = true;
        client.unrelatedWoodenPickaxePickedUpAfterIdentityLoss = true;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-20-campfire-death-drop-recovery",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry ->
                entry.contains("campfire death-drop recovery: failed")
                    && entry.contains("pickup_disappeared=true")
                    && entry.contains("pickup_restored=false")
            ),
            "an unrelated inventory gain after identity loss must not confirm pickup"
        );
    }

    @Test
    void itemDropIdentitySnapshotIsGlobalUuidBoundAndHasNoFallback() throws Exception {
        String source = Files.readString(Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftScenarioClient.java"
        ));
        String mixinSource = Files.readString(Path.of(
            "../fabric-agent/src/main/java/dev/solaris/agent/mixin/ClientPacketListenerMixin.java"
        ));
        String snapshotSignature =
            "private static List<ScenarioItemDropIdentity> itemDropIdentitiesOnClientThread(";
        String resolverSignature =
            "private static Vec3 itemDropPositionOnClientThread(String itemId, ScenarioItemDropIdentity identity)";

        assertTrue(source.contains(snapshotSignature), "item-drop snapshots must use immutable identities");
        assertTrue(source.contains(resolverSignature), "identity collection must have an exact UUID-bound resolver");

        String snapshot = source.substring(
            source.indexOf(snapshotSignature),
            source.indexOf("private static ScenarioItemDropIdentity newItemDropIdentityOnClientThread(")
        );
        assertTrue(snapshot.contains("entitiesForRendering()"));
        assertFalse(snapshot.contains("distanceToSqr"), "snapshot must include every rendered matching item");

        String resolver = source.substring(
            source.indexOf(resolverSignature),
            source.indexOf("private static ScenarioEntityObservation visibleEntityOnClientThread(")
        );
        assertTrue(resolver.contains("entity.getUUID()"), "numeric entity ids must be verified by UUID");
        assertFalse(resolver.contains("center"), "identity loss must not fall back to the campfire center");
        assertTrue(source.contains("ClientStateEvents.consumeItemTakenBy(expectedIdentity"));
        assertTrue(
            source.contains("expectedIdentity != null && dropGone && !itemTakeObserved"),
            "identity disappearance without its take packet must fail"
        );
        assertTrue(
            source.contains("awaitClientStateChange(observedStateVersion, deadlineNanos)"),
            "inventory confirmation after a take packet must wait for the exact next state event"
        );
        assertTrue(mixinSource.contains("handleTakeItemEntity"));
        assertTrue(mixinSource.contains("ClientStateEvents.publishItemTaken"));
    }

    @Test
    void itemDropIdentityRemainsPresentAfterMovingOutsideFormerRadius() throws Exception {
        String source = Files.readString(Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftScenarioClient.java"
        ));
        String resolverSignature =
            "private static Vec3 itemDropPositionOnClientThread(String itemId, ScenarioItemDropIdentity identity)";
        String resolver = source.substring(
            source.indexOf(resolverSignature),
            source.indexOf("private static ScenarioEntityObservation visibleEntityOnClientThread(")
        );

        assertTrue(resolver.contains("minecraft.level.getEntity(identity.entityId())"));
        assertFalse(
            resolver.contains("distanceToSqr"),
            "a rendered exact identity outside the former four-block radius must remain present"
        );
    }

    @Test
    void earnedToolZombieCombatKillsNaturalHostileWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-21-earned-tool-zombie-combat",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertFalse(client.usedDebugSetup(), "earned hostile combat path must not use debug setup");
        assertTrue(
            client.operations.contains("waitNight"),
            "earned hostile combat path must wait for natural night instead of summoning"
        );
        assertTrue(
            client.operations.contains("findEntity:minecraft:zombie:outside-survival-reach"),
            "earned hostile combat path must scan for a naturally loaded zombie"
        );
        assertTrue(
            client.operations.contains("selectHotbar:minecraft:wooden_pickaxe:1"),
            "earned hostile combat path must fight with the crafted tool"
        );
        assertTrue(
            client.operations.contains("attackEntityDrop:minecraft:zombie:entity_id=99:minecraft:rotten_flesh:1"),
            "earned hostile combat path must kill the zombie and collect its drop"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("zombie combat drop: passed")),
            "earned hostile combat path must record hostile kill/drop evidence"
        );
    }

    @Test
    void earnedToolZombieCombatFailsClosedIfPlayerDiesDuringCombat() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.healthAfterHostileCombat = 0.0F;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-21-earned-tool-zombie-combat",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "failed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("zombie combat survival: failed")),
            "earned hostile combat path must not pass when the server leaves the player dead"
        );
    }

    @Test
    void stoneSwordZombieCombatCraftsWeaponAndKillsNaturalHostileWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-22-stone-sword-zombie-combat",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertFalse(client.usedDebugSetup(), "stone sword hostile combat path must not use debug setup");
        assertTrue(
            client.operations.contains("recipe:7:26:false"),
            "stone sword combat must craft the embedded stone sword recipe"
        );
        assertTrue(
            client.operations.contains("waitCount:minecraft:stone_sword:1"),
            "stone sword combat must wait for the crafted stone sword"
        );
        assertTrue(
            client.operations.contains("selectHotbar:minecraft:stone_sword:1"),
            "stone sword combat must fight with the crafted weapon"
        );
        assertTrue(
            client.operations.contains("findEntity:minecraft:zombie:outside-survival-reach"),
            "stone sword combat path must scan for a naturally loaded zombie"
        );
        assertTrue(
            client.operations.contains("attackEntityDrop:minecraft:zombie:entity_id=99:minecraft:rotten_flesh:1"),
            "stone sword combat path must kill the zombie and collect its drop"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("stone sword recipe: passed")),
            "stone sword combat must record stone sword recipe evidence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("stone sword zombie combat: passed")),
            "stone sword combat must record hostile kill/drop evidence"
        );
    }

    @Test
    void ironIngotProgressionMinesNaturalOreAndSmeltsWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-23-iron-ingot-progression",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertFalse(client.usedDebugSetup(), "iron progression path must not use debug setup");
        assertTrue(
            client.operations.contains("recipe:7:24:false"),
            "iron progression must craft an earned stone pickaxe"
        );
        assertTrue(
            client.operations.contains("recipe:7:13:false"),
            "iron progression must craft an earned furnace"
        );
        assertTrue(
            client.operations.contains("selectHotbar:minecraft:stone_pickaxe:1"),
            "iron progression must mine ore with the earned stone pickaxe"
        );
        assertTrue(
            client.operations.stream().anyMatch(operation -> operation.startsWith(
                "findBreakable:minecraft:iron_ore|minecraft:deepslate_iron_ore:"
            )),
            "iron progression must scan for natural iron ore blocks"
        );
        assertTrue(
            client.operations.contains("breakVisible:minecraft:iron_ore:minecraft:raw_iron"),
            "iron progression must break natural iron ore into raw iron"
        );
        assertTrue(
            client.operations.contains("moveToContainer:0:minecraft:raw_iron:2"),
            "iron progression must move two earned raw iron into furnace input"
        );
        assertTrue(
            client.operations.contains("waitContainer:2:minecraft:iron_ingot:2"),
            "iron progression must wait for two ingots so vanilla 0.7 XP rounding cannot yield zero"
        );
        assertTrue(
            client.operations.contains("moveFromContainer:2:minecraft:iron_ingot:2"),
            "iron progression must move both iron ingots into inventory"
        );
        assertTrue(
            client.operations.contains("waitExperienceAbove:0"),
            "iron progression must wait for furnace experience to reach the client"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("natural iron ore break/drop/pickup: passed")),
            "iron progression must record natural ore break/drop evidence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("furnace iron ingot inventory: passed")),
            "iron progression must record smelted ingot inventory evidence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("furnace iron experience: passed")),
            "iron progression must record received furnace experience"
        );
    }

    @Test
    void ironSwordZombieCombatMinesSmeltsCraftsAndFightsWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-24-iron-sword-zombie-combat",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertFalse(client.usedDebugSetup(), "iron sword combat path must not use debug setup");
        assertTrue(
            client.operations.contains("recipe:7:24:false"),
            "iron sword combat must craft an earned stone pickaxe"
        );
        assertTrue(
            client.operations.contains("recipe:7:13:false"),
            "iron sword combat must craft an earned furnace"
        );
        assertTrue(
            client.operations.stream()
                    .filter("breakVisible:minecraft:iron_ore:minecraft:raw_iron"::equals)
                    .count() >= 2,
            "iron sword combat must mine two natural iron ore blocks"
        );
        assertTrue(
            client.operations.stream()
                    .filter("waitContainer:2:minecraft:iron_ingot:1"::equals)
                    .count() >= 2,
            "iron sword combat must smelt two earned iron ingots"
        );
        assertTrue(
            client.operations.contains("recipe:7:57:false"),
            "iron sword combat must place the earned iron sword recipe"
        );
        assertTrue(
            client.operations.contains("waitCount:minecraft:iron_sword:1"),
            "iron sword combat must wait for the crafted iron sword"
        );
        assertTrue(
            client.operations.contains("selectHotbar:minecraft:iron_sword:1"),
            "iron sword combat must fight with the crafted iron sword"
        );
        assertTrue(
            client.operations.contains("attackEntityDrop:minecraft:zombie:entity_id=99:minecraft:rotten_flesh:1"),
            "iron sword combat path must kill a natural zombie and collect its drop"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("iron sword recipe: passed")),
            "iron sword combat must record iron sword recipe evidence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("iron sword zombie combat: passed")),
            "iron sword combat must record hostile kill/drop evidence"
        );
    }

    @Test
    void shieldZombieBlockCraftsEarnedShieldAndHoldsUseWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-26-earned-shield-zombie-block",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertFalse(client.usedDebugSetup(), "shield block path must not use debug setup");
        assertTrue(
            client.operations.contains("recipe:7:58:false"),
            "shield block path must craft the earned shield recipe"
        );
        assertTrue(
            client.operations.contains("waitCount:minecraft:shield:1"),
            "shield block path must wait for the crafted shield"
        );
        assertTrue(
            client.operations.contains("selectHotbar:minecraft:shield:1"),
            "shield block path must select the earned shield"
        );
        assertTrue(
            client.operations.contains("findEntity:minecraft:zombie:outside-survival-reach"),
            "shield block path must scan for a naturally loaded zombie"
        );
        assertTrue(
            client.operations.contains("blockAttackWithSelectedShield:minecraft:shield"),
            "shield block path must observe a blocked attack with the earned shield"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("shield recipe: passed")),
            "shield block path must record shield recipe evidence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("shield zombie block: passed")),
            "shield block path must record shield use survival evidence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("blocked_attack_observed=true")),
            "shield block path must record a positive blocked-attack event"
        );
    }

    @Test
    void shieldZombieBlockFailsWhenNoBlockedAttackWasObserved() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.shieldBlockedAttackObserved = false;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-26-earned-shield-zombie-block",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "failed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
    }

    @Test
    void ironChestplateEquipCraftsEarnedChestplateAndQuickEquipsWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-27-earned-iron-chestplate-equip",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertFalse(client.usedDebugSetup(), "iron chestplate equip path must not use debug setup");
        assertTrue(
            client.operations.contains("recipe:7:59:false"),
            "iron chestplate equip path must craft the earned chestplate recipe"
        );
        assertTrue(
            client.operations.contains("waitCount:minecraft:iron_chestplate:1"),
            "iron chestplate equip path must wait for the crafted chestplate"
        );
        assertTrue(
            client.operations.contains("selectHotbar:minecraft:iron_chestplate:1"),
            "iron chestplate equip path must select the crafted chestplate"
        );
        assertTrue(
            client.operations.contains("quickEquip:minecraft:iron_chestplate:chest"),
            "iron chestplate equip path must equip through the normal inventory quick-move path"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("iron chestplate recipe: passed")),
            "iron chestplate equip path must record chestplate recipe evidence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("iron chestplate equip: passed")),
            "iron chestplate equip path must record equipped armor evidence"
        );
    }

    @Test
    void ironChestplateZombieMitigationEquipsEarnedChestplateAndMeasuresNaturalHitWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-28-earned-iron-chestplate-zombie-mitigation",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertFalse(client.usedDebugSetup(), "iron chestplate mitigation path must not use debug setup");
        assertTrue(
            client.operations.contains("quickEquip:minecraft:iron_chestplate:chest"),
            "iron chestplate mitigation path must equip through the normal inventory quick-move path"
        );
        assertTrue(
            client.operations.contains("findEntity:minecraft:zombie:outside-survival-reach"),
            "iron chestplate mitigation path must scan for a naturally loaded zombie"
        );
        assertTrue(
            client.operations.contains("waitHealthBelow:20.0"),
            "iron chestplate mitigation path must wait for natural zombie damage"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("iron chestplate zombie mitigation: passed")),
            "iron chestplate mitigation path must record reduced natural zombie damage"
        );
    }

    @Test
    void ironChestplateRestartBeforeWritesMarkerAndEquipsChestplateWithoutDebugSetup() throws Exception {
        Path runDir = Files.createTempDirectory("playable-29-before");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-29-iron-chestplate-save-restart-mitigation-before",
            screenshotsDir,
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertFalse(client.usedDebugSetup(), "iron chestplate restart before phase must not use debug setup");
        assertTrue(
            Files.isRegularFile(runDir.resolve("playable-03-save-restart-marker.properties")),
            "iron chestplate restart before phase must write the crafted table marker for after phase"
        );
        assertTrue(
            client.operations.contains("quickEquip:minecraft:iron_chestplate:chest"),
            "before phase must equip the earned iron chestplate before restart"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("iron chestplate equip: passed")),
            "before phase must record chestplate equip evidence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("runner-managed restart: pending")),
            "before phase must record that the runner owns the restart boundary"
        );
    }

    @Test
    void ironChestplateRestartAfterChecksPersistedArmorAndMitigationWithoutDebugSetup()
        throws Exception {
        Path runDir = Files.createTempDirectory("playable-29-after");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient client = new FakeScenarioClient();
        ClientScenarioReport before = new PlayableRealClientLoopScenario().run(
            "playable-29-iron-chestplate-save-restart-mitigation-before",
            screenshotsDir,
            client
        );
        assertEquals("passed", before.result());
        client.operations.clear();

        ClientScenarioReport after = new PlayableRealClientLoopScenario().run(
            "playable-29-iron-chestplate-save-restart-mitigation-after",
            screenshotsDir,
            client
        );

        assertEquals(
            "passed",
            after.result(),
            () -> String.join("\n", after.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            client.operations.contains("waitBlock:restart-marker:minecraft:crafting_table"),
            "after phase must verify the persisted crafting table marker"
        );
        assertTrue(
            client.operations.contains("equippedArmor:chest"),
            "after phase must read the persisted chest armor slot"
        );
        assertTrue(
            client.operations.contains("findEntity:minecraft:zombie:outside-survival-reach"),
            "after phase must scan for a naturally loaded zombie after restart"
        );
        assertTrue(
            client.operations.contains("waitHealthBelow:20.0"),
            "after phase must wait for natural zombie damage after restart"
        );
        assertFalse(client.usedDebugSetup(), "iron chestplate restart after phase must not use debug setup");
        assertTrue(
            after.observations().stream().anyMatch(entry -> entry.contains("restart marker persistence: passed")),
            "after phase must record persisted marker observation"
        );
        assertTrue(
            after.observations().stream().anyMatch(entry -> entry.contains("iron chestplate armor persistence: passed")),
            "after phase must record persisted equipped armor observation"
        );
        assertTrue(
            after.observations().stream().anyMatch(entry -> entry.contains("iron chestplate restarted zombie mitigation: passed")),
            "after phase must record post-restart armor mitigation evidence"
        );
    }

    @Test
    void ironChestplateRestartAfterFailsClosedWithoutMarker() {
        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-29-iron-chestplate-save-restart-mitigation-after",
            Path.of("build/tmp/playable-29-missing/screenshots"),
            new FakeScenarioClient()
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("missing restart marker")),
            "after phase must fail closed when marker file is absent"
        );
    }

    @Test
    void twoClientSharedLogDropBreakWritesNaturalDropMarkerWithoutDebugSetup() throws Exception {
        Path runDir = Files.createTempDirectory("playable-30-drop");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-30-two-client-shared-log-drop-break",
            screenshotsDir,
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertFalse(client.usedDebugSetup(), "two-client shared drop break must not use debug setup");
        assertTrue(
            Files.isRegularFile(runDir.resolve("playable-30-shared-log-drop-marker.properties")),
            "primary drop phase must write the shared natural log drop marker"
        );
        assertTrue(
            client.operations.contains("breakVisible:minecraft:oak_log:minecraft:oak_log"),
            "primary drop phase must break a natural generated log through the normal break path"
        );
        assertFalse(
            client.operations.stream().anyMatch(operation -> operation.startsWith("collect:")),
            "primary drop phase must leave the natural log drop visible for the secondary client"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client shared log drop break: passed")),
            "primary drop phase must record shared natural log drop evidence"
        );
    }

    @Test
    void twoClientSharedLogDropObserveSeesPrimaryDropWithoutDebugSetup() throws Exception {
        Path runDir = Files.createTempDirectory("playable-30-observe");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient primary = new FakeScenarioClient();
        ClientScenarioReport before = new PlayableRealClientLoopScenario().run(
            "playable-30-two-client-shared-log-drop-break",
            screenshotsDir,
            primary
        );
        assertEquals("passed", before.result());

        FakeScenarioClient secondary = new FakeScenarioClient();
        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-30-two-client-shared-log-drop-observe",
            screenshotsDir,
            secondary
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + secondary.operations
        );
        assertFalse(secondary.usedDebugSetup(), "secondary drop observe must not use debug setup");
        assertTrue(
            secondary.operations.contains("waitDrop:minecraft:oak_log:playable-two-client-drop-marker"),
            "secondary must wait for the shared natural log item entity at the primary marker"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client shared log drop visibility: passed")),
            "secondary drop observe must record shared drop visibility"
        );
    }

    @Test
    void twoClientSharedLogPickupCollectAndGoneObserveUseSharedMarker() throws Exception {
        Path runDir = Files.createTempDirectory("playable-30-pickup");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient primary = new FakeScenarioClient();
        ClientScenarioReport before = new PlayableRealClientLoopScenario().run(
            "playable-30-two-client-shared-log-drop-break",
            screenshotsDir,
            primary
        );
        assertEquals("passed", before.result());
        primary.operations.clear();

        ClientScenarioReport pickup = new PlayableRealClientLoopScenario().run(
            "playable-30-two-client-shared-log-pickup-collect",
            screenshotsDir,
            primary
        );

        assertEquals(
            "passed",
            pickup.result(),
            () -> String.join("\n", pickup.observations()) + "\noperations=" + primary.operations
        );
        assertTrue(
            primary.operations.contains("collect:minecraft:oak_log:minecraft:oak_log:1"),
            "primary must collect the shared natural log drop through normal pickup"
        );
        assertTrue(
            pickup.observations().stream().anyMatch(entry -> entry.contains("two-client shared log pickup: passed")),
            "primary pickup phase must record pickup convergence"
        );

        FakeScenarioClient secondary = new FakeScenarioClient();
        ClientScenarioReport gone = new PlayableRealClientLoopScenario().run(
            "playable-30-two-client-shared-log-pickup-gone-observe",
            screenshotsDir,
            secondary
        );

        assertEquals(
            "passed",
            gone.result(),
            () -> String.join("\n", gone.observations()) + "\noperations=" + secondary.operations
        );
        assertTrue(
            secondary.operations.contains("waitNoDrop:minecraft:oak_log:playable-two-client-drop-marker"),
            "secondary must observe the shared natural log item entity disappear after primary pickup"
        );
        assertFalse(secondary.usedDebugSetup(), "secondary pickup removal observe must not use debug setup");
        assertTrue(
            gone.observations().stream().anyMatch(entry -> entry.contains("two-client shared log pickup removal: passed")),
            "secondary removal phase must record shared pickup removal"
        );
    }

    @Test
    void twoClientSharedLogDropObserveFailsClosedWithoutMarker() {
        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-30-two-client-shared-log-drop-observe",
            Path.of("build/tmp/playable-30-missing/screenshots"),
            new FakeScenarioClient()
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("missing shared log drop marker")),
            "secondary observe phase must fail closed when marker file is absent"
        );
    }

    @Test
    void twoClientEarnedSharedChestDepositWritesEarnedPlankMarkerWithoutDebugSetup() throws Exception {
        Path runDir = Files.createTempDirectory("playable-31-deposit");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-31-two-client-earned-shared-chest-deposit",
            screenshotsDir,
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertFalse(client.usedDebugSetup(), "earned shared chest deposit must not use debug setup");
        assertTrue(
            Files.isRegularFile(runDir.resolve("playable-31-shared-chest-marker.properties")),
            "primary deposit phase must write the shared earned chest marker"
        );
        assertTrue(
            client.operations.contains("recipe:7:5:false"),
            "primary deposit phase must craft the shared chest from earned planks"
        );
        assertTrue(
            client.operations.contains("moveToContainer:0:minecraft:oak_planks:1"),
            "primary deposit phase must deposit leftover earned planks from the chest-crafting route"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client shared chest deposit: passed")),
            "primary deposit phase must record shared chest deposit evidence"
        );
    }

    @Test
    void twoClientEarnedSharedChestWithdrawTakesPrimaryItemWithoutDebugSetup() throws Exception {
        Path runDir = Files.createTempDirectory("playable-31-withdraw");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient primary = new FakeScenarioClient();
        ClientScenarioReport deposit = new PlayableRealClientLoopScenario().run(
            "playable-31-two-client-earned-shared-chest-deposit",
            screenshotsDir,
            primary
        );
        assertEquals("passed", deposit.result());

        FakeScenarioClient secondary = new FakeScenarioClient();
        secondary.containerItemIds[0] = "minecraft:oak_planks";
        secondary.containerCounts[0] = 1;
        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-31-two-client-earned-shared-chest-withdraw",
            screenshotsDir,
            secondary
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + secondary.operations
        );
        assertFalse(secondary.usedDebugSetup(), "secondary shared chest withdraw must not use debug setup");
        assertTrue(
            secondary.operations.contains("waitBlock:playable-two-client-chest-marker:minecraft:chest"),
            "secondary must wait for the primary placed chest block"
        );
        assertTrue(
            secondary.operations.contains("approach:minecraft:chest:playable-two-client-chest-marker"),
            "secondary must walk to the primary placed chest before opening it"
        );
        assertTrue(
            secondary.operations.contains("waitContainer:0:minecraft:oak_planks:1"),
            "secondary must observe the primary deposited item"
        );
        assertTrue(
            secondary.operations.contains("moveFromContainer:0:minecraft:oak_planks:1"),
            "secondary must withdraw the shared item through the container quick-move path"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client shared chest withdraw: passed")),
            "secondary withdraw phase must record shared chest transfer evidence"
        );
    }

    @Test
    void twoClientEarnedSharedChestObserveEmptyFailsClosedWithoutMarker() {
        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-31-two-client-earned-shared-chest-observe-empty",
            Path.of("build/tmp/playable-31-missing/screenshots"),
            new FakeScenarioClient()
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("missing shared chest marker")),
            "primary observe-empty phase must fail closed when marker file is absent"
        );
    }

    @Test
    void twoClientEarnedSharedChestObserveEmptyUsesMarkerAfterSecondaryWithdraw() throws Exception {
        Path runDir = Files.createTempDirectory("playable-31-empty");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient primary = new FakeScenarioClient();
        ClientScenarioReport deposit = new PlayableRealClientLoopScenario().run(
            "playable-31-two-client-earned-shared-chest-deposit",
            screenshotsDir,
            primary
        );
        assertEquals("passed", deposit.result());

        primary.containerItemIds[0] = null;
        primary.containerCounts[0] = 0;
        primary.operations.clear();
        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-31-two-client-earned-shared-chest-observe-empty",
            screenshotsDir,
            primary
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + primary.operations
        );
        assertTrue(
            primary.operations.contains("waitContainerEmpty:0"),
            "primary must observe the shared chest slot empty after secondary withdraw"
        );
        assertTrue(
            primary.operations.contains("approach:minecraft:chest:playable-two-client-chest-marker"),
            "primary empty-observe phase must walk to the shared chest marker before opening it"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client shared chest empty observe: passed")),
            "primary empty-observe phase must record shared chest removal evidence"
        );
    }

    @Test
    void twoClientEarnedTorchPlaceWritesBlockMarkerWithoutDebugSetup() throws Exception {
        Path runDir = Files.createTempDirectory("playable-32-place");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-32-two-client-earned-torch-place",
            screenshotsDir,
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertFalse(client.usedDebugSetup(), "earned shared torch placement must not use debug setup");
        assertTrue(
            Files.isRegularFile(runDir.resolve("playable-32-shared-block-edit-marker.properties")),
            "primary place phase must write the shared block edit marker"
        );
        assertTrue(
            client.operations.contains("recipe:0:27:false"),
            "primary place phase must craft torches through the earned charcoal route"
        );
        assertTrue(
            client.operations.contains("selectHotbar:minecraft:torch:4"),
            "primary place phase must select the earned torches from hotbar"
        );
        assertTrue(
            client.operations.contains("use:minecraft:torch:torch-clicked"),
            "primary place phase must place the earned torch item"
        );
        assertTrue(
            client.operations.contains("waitBlock:torch-target:minecraft:torch"),
            "primary place phase must wait for the placed torch block"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client shared torch placement: passed")),
            "primary place phase must record shared torch placement evidence"
        );
    }

    @Test
    void twoClientEarnedTorchObserveSeesPrimaryPlacedBlockWithoutDebugSetup() throws Exception {
        Path runDir = Files.createTempDirectory("playable-32-observe");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient primary = new FakeScenarioClient();
        ClientScenarioReport place = new PlayableRealClientLoopScenario().run(
            "playable-32-two-client-earned-torch-place",
            screenshotsDir,
            primary
        );
        assertEquals("passed", place.result());

        FakeScenarioClient secondary = new FakeScenarioClient();
        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-32-two-client-earned-torch-observe",
            screenshotsDir,
            secondary
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + secondary.operations
        );
        assertFalse(secondary.usedDebugSetup(), "secondary shared torch observe must not use debug setup");
        assertTrue(
            secondary.operations.contains("approach:minecraft:dirt:playable-two-client-block-approach"),
            "secondary must walk to the solid support block for the primary placed torch before observing it"
        );
        assertTrue(
            secondary.operations.contains("waitBlock:playable-two-client-block-marker:minecraft:torch"),
            "secondary must wait for the primary placed torch block"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client shared torch visibility: passed")),
            "secondary observe phase must record shared torch block visibility"
        );
    }

    @Test
    void twoClientEarnedTorchBreakAndGoneObserveUseSharedMarker() throws Exception {
        Path runDir = Files.createTempDirectory("playable-32-break");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient primary = new FakeScenarioClient();
        ClientScenarioReport place = new PlayableRealClientLoopScenario().run(
            "playable-32-two-client-earned-torch-place",
            screenshotsDir,
            primary
        );
        assertEquals("passed", place.result());
        primary.operations.clear();
        primary.failedTorchPickupsRemaining = 1;

        ClientScenarioReport broke = new PlayableRealClientLoopScenario().run(
            "playable-32-two-client-earned-torch-break",
            screenshotsDir,
            primary
        );

        assertEquals(
            "passed",
            broke.result(),
            () -> String.join("\n", broke.observations()) + "\noperations=" + primary.operations
        );
        assertTrue(
            primary.operations.contains("approach:minecraft:dirt:playable-two-client-block-approach"),
            "primary break phase must walk back to the solid support block for the shared torch marker"
        );
        assertTrue(
            primary.operations.contains("breakVisible:minecraft:torch:minecraft:torch"),
            "primary break phase must break the placed torch through the normal block edit path"
        );
        assertTrue(
            primary.operations.contains("collect:minecraft:torch:minecraft:torch:1"),
            "primary break phase should attempt to collect the torch drop"
        );
        assertTrue(
            primary.operations.contains("waitBlock:playable-two-client-block-marker:minecraft:air"),
            "primary break phase must confirm the torch block became air"
        );
        assertTrue(
            broke.observations().stream().anyMatch(entry -> entry.contains("two-client shared torch break: passed")),
            "primary break phase must record shared torch break evidence"
        );
        assertTrue(
            broke.observations().stream().anyMatch(entry -> entry.contains("collected_by_breaker=false")),
            "a peer pickup must not invalidate otherwise complete shared block-edit evidence"
        );

        FakeScenarioClient secondary = new FakeScenarioClient();
        ClientScenarioReport gone = new PlayableRealClientLoopScenario().run(
            "playable-32-two-client-earned-torch-gone-observe",
            screenshotsDir,
            secondary
        );

        assertEquals(
            "passed",
            gone.result(),
            () -> String.join("\n", gone.observations()) + "\noperations=" + secondary.operations
        );
        assertTrue(
            secondary.operations.contains("approach:minecraft:dirt:playable-two-client-block-approach"),
            "secondary gone-observe phase must walk to the solid support block for the shared torch marker"
        );
        assertTrue(
            secondary.operations.contains("waitBlock:playable-two-client-block-marker:minecraft:air"),
            "secondary must observe the placed torch block disappear"
        );
        assertFalse(secondary.usedDebugSetup(), "secondary block removal observe must not use debug setup");
        assertTrue(
            gone.observations().stream().anyMatch(entry -> entry.contains("two-client shared torch removal visibility: passed")),
            "secondary gone-observe phase must record shared torch removal visibility"
        );
    }

    @Test
    void twoClientEarnedTorchObserveFailsClosedWithoutMarker() {
        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-32-two-client-earned-torch-observe",
            Path.of("build/tmp/playable-32-missing/screenshots"),
            new FakeScenarioClient()
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("missing shared block edit marker")),
            "secondary observe phase must fail closed when marker file is absent"
        );
    }

    @Test
    void twoClientPlayerObserveWritesVisibilityMarker() throws Exception {
        Path runDir = Files.createTempDirectory("playable-33-observe");
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-33-two-client-player-observe",
            runDir.resolve("screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            Files.isRegularFile(runDir.resolve("playable-33-player-visibility-marker.properties")),
            "secondary observe phase must write the shared player visibility marker"
        );
        assertTrue(
            client.operations.contains("waitPlayer:SolarisPrimary"),
            "secondary observe phase must wait for the primary player through client-visible entities"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client player visibility: passed")),
            "secondary observe phase must record player visibility evidence"
        );
    }

    @Test
    void twoClientPlayerMovedObserveUsesVisibilityMarker() throws Exception {
        Path runDir = Files.createTempDirectory("playable-33-moved");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient client = new FakeScenarioClient();
        ClientScenarioReport observe = new PlayableRealClientLoopScenario().run(
            "playable-33-two-client-player-observe",
            screenshotsDir,
            client
        );
        assertEquals("passed", observe.result());
        client.operations.clear();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-33-two-client-player-moved-observe",
            screenshotsDir,
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            client.operations.contains("waitPlayerMoved:SolarisPrimary:0.05"),
            "secondary moved-observe phase must wait for a visible movement delta from the primary player"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client player movement visibility: passed")),
            "secondary moved-observe phase must record player movement visibility evidence"
        );
    }

    @Test
    void twoClientPlayerMovedObserveFailsClosedWithoutMarker() {
        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-33-two-client-player-moved-observe",
            Path.of("build/tmp/playable-33-missing/screenshots"),
            new FakeScenarioClient()
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("missing player visibility marker")),
            "moved-observe phase must fail closed when marker file is absent"
        );
    }

    @Test
    void twoClientChatSendUsesNormalChatMessageWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-34-two-client-chat-send",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            client.operations.contains("sendChat:p34 hello from primary"),
            "primary send phase must use normal chat, not slash commands"
        );
        assertFalse(client.usedDebugSetup(), "chat send phase must not use debug command setup");
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client chat send: passed")),
            "primary send phase must record chat send evidence"
        );
    }

    @Test
    void twoClientChatObserveSeesPrimaryMessageWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-34-two-client-chat-observe",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            client.operations.contains("waitChat:<SolarisPrimary> p34 hello from primary"),
            "secondary observe phase must wait for the rendered primary chat line"
        );
        assertFalse(client.usedDebugSetup(), "chat observe phase must not use debug command setup");
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client chat observe: passed")),
            "secondary observe phase must record visible chat evidence"
        );
    }

    @Test
    void twoClientPlayerDisconnectVisiblePhaseRecordsPrimaryBeforeDisconnect() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-35-two-client-player-disconnect-visible",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            client.operations.contains("waitPlayer:SolarisPrimary"),
            "secondary visible phase must prove the primary player was visible before disconnect"
        );
        assertFalse(client.usedDebugSetup(), "player disconnect visible phase must not use debug setup");
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client player pre-disconnect visibility: passed")),
            "visible phase must record pre-disconnect player visibility evidence"
        );
    }

    @Test
    void twoClientPlayerDisconnectGoneObserveWaitsForPrimaryRemoval() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-35-two-client-player-gone-observe",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            client.operations.contains("waitNoPlayer:SolarisPrimary"),
            "secondary gone-observe phase must wait until the primary player entity disappears"
        );
        assertFalse(client.usedDebugSetup(), "player disconnect gone-observe phase must not use debug setup");
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client player disconnect removal: passed")),
            "gone-observe phase must record player removal evidence"
        );
    }

    @Test
    void twoClientPlayerReconnectVisiblePhaseWritesBaselineMarker() throws Exception {
        Path runDir = Files.createTempDirectory("playable-36-visible");
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-36-two-client-player-reconnect-visible",
            runDir.resolve("screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            Files.isRegularFile(runDir.resolve("playable-33-player-visibility-marker.properties")),
            "secondary reconnect-visible phase must write the baseline player visibility marker"
        );
        assertTrue(
            client.operations.contains("waitPlayer:SolarisPrimary"),
            "secondary reconnect-visible phase must prove the primary player was visible before reconnect"
        );
        assertFalse(client.usedDebugSetup(), "player reconnect visible phase must not use debug setup");
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client player pre-reconnect visibility: passed")),
            "visible phase must record pre-reconnect player visibility evidence"
        );
    }

    @Test
    void twoClientPlayerReconnectedObserveRequiresNewVisibleEntity() throws Exception {
        Path runDir = Files.createTempDirectory("playable-36-reconnected");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient client = new FakeScenarioClient();
        client.visiblePlayerEntityId = 777;
        ClientScenarioReport visible = new PlayableRealClientLoopScenario().run(
            "playable-36-two-client-player-reconnect-visible",
            screenshotsDir,
            client
        );
        assertEquals("passed", visible.result());
        client.operations.clear();
        client.visiblePlayerEntityId = 778;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-36-two-client-player-reconnected-observe",
            screenshotsDir,
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            client.operations.contains("waitPlayer:SolarisPrimary"),
            "secondary reconnected-observe phase must wait for the reconnected primary player"
        );
        assertFalse(client.usedDebugSetup(), "player reconnect observe phase must not use debug setup");
        assertTrue(
            report.observations().stream().anyMatch(entry ->
                entry.contains("two-client player reconnect visibility: passed")
                    && entry.contains("old_entity_id=777")
                    && entry.contains("new_entity_id=778")
            ),
            "reconnected-observe phase must record old and new primary player entity ids"
        );
    }

    @Test
    void twoClientPlayerReconnectGoneObserveWaitsForPrimaryRemoval() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-36-two-client-player-reconnect-gone-observe",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            client.operations.contains("waitNoPlayer:SolarisPrimary"),
            "secondary reconnect gone-observe phase must wait until the old primary player entity disappears"
        );
        assertFalse(client.usedDebugSetup(), "player reconnect gone-observe phase must not use debug setup");
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client player reconnect removal: passed")),
            "gone-observe phase must record reconnect removal evidence"
        );
    }

    @Test
    void twoClientPlayerReconnectedObserveFailsClosedWithoutMarker() {
        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-36-two-client-player-reconnected-observe",
            Path.of("build/tmp/playable-36-missing/screenshots"),
            new FakeScenarioClient()
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("missing player visibility marker")),
            "reconnected-observe phase must fail closed when the baseline marker is absent"
        );
    }

    @Test
    void twoClientPlayerDeathBaselineWritesVisibilityMarker() throws Exception {
        Path runDir = Files.createTempDirectory("playable-37-baseline");
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-37-two-client-player-death-baseline",
            runDir.resolve("screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            Files.isRegularFile(runDir.resolve("playable-33-player-visibility-marker.properties")),
            "secondary death baseline phase must write the player visibility marker"
        );
        assertTrue(
            client.operations.contains("waitPlayer:SolarisPrimary"),
            "secondary death baseline phase must prove the primary player was visible before death"
        );
        assertFalse(client.usedDebugSetup(), "player death baseline phase must not use debug setup");
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client player pre-death visibility: passed")),
            "death baseline phase must record pre-death player visibility evidence"
        );
    }

    @Test
    void twoClientCampfireDeathRespawnUsesNaturalDeathPathWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.logItemId = "minecraft:birch_log";
        client.planksItemId = "minecraft:birch_planks";

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-37-two-client-campfire-death-respawn",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            client.operations.contains("standOnBlockUntilDeath:campfire-target:minecraft:campfire"),
            "primary death phase must use natural campfire contact damage"
        );
        assertTrue(
            client.operations.contains("respawn"),
            "primary death phase must perform the vanilla respawn packet"
        );
        assertFalse(
            client.operations.stream().anyMatch(operation ->
                operation.startsWith("findBreakable:minecraft:campfire|minecraft:soul_campfire")
            ),
            "primary death phase must reuse the campfire placement target instead of rediscovering it as a solid block"
        );
        assertFalse(client.usedDebugSetup(), "primary death/respawn phase must not use debug setup");
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client campfire death/respawn: passed")),
            "primary death phase must record death/respawn evidence for the two-client scenario"
        );
    }

    @Test
    void twoClientPlayerPostRespawnMovedObserveUsesBaselineMarker() throws Exception {
        Path runDir = Files.createTempDirectory("playable-37-post-respawn");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient client = new FakeScenarioClient();
        ClientScenarioReport baseline = new PlayableRealClientLoopScenario().run(
            "playable-37-two-client-player-death-baseline",
            screenshotsDir,
            client
        );
        assertEquals("passed", baseline.result());
        client.operations.clear();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-37-two-client-player-post-respawn-moved-observe",
            screenshotsDir,
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            client.operations.contains("waitPlayerMoved:SolarisPrimary:0.05"),
            "secondary post-respawn phase must wait for visible movement from the primary player"
        );
        assertFalse(client.usedDebugSetup(), "post-respawn moved-observe phase must not use debug setup");
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client player post-respawn movement visibility: passed")),
            "post-respawn moved-observe phase must record visible movement evidence"
        );
    }

    @Test
    void twoClientPlayerPostRespawnMovedObserveFailsClosedWithoutMarker() {
        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-37-two-client-player-post-respawn-moved-observe",
            Path.of("build/tmp/playable-37-missing/screenshots"),
            new FakeScenarioClient()
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("missing player visibility marker")),
            "post-respawn moved-observe phase must fail closed when the baseline marker is absent"
        );
    }

    @Test
    void twoClientInventoryDropPrimaryDropsEarnedNaturalLogAndWritesMarker() throws Exception {
        Path runDir = Files.createTempDirectory("playable-38-drop-primary");
        FakeScenarioClient client = new FakeScenarioClient();
        client.logItemId = "minecraft:birch_log";
        client.planksItemId = "minecraft:birch_planks";

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-38-two-client-inventory-drop-primary",
            runDir.resolve("screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            Files.isRegularFile(runDir.resolve("playable-38-inventory-drop-marker.properties")),
            "primary inventory-drop phase must persist the dropped item marker for the observer client"
        );
        assertTrue(
            client.operations.contains("dropSelected:minecraft:birch_log:1"),
            "primary inventory-drop phase must drop the earned log through the selected-item drop primitive"
        );
        assertFalse(client.usedDebugSetup(), "inventory drop handoff primary phase must not use debug setup");
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("two-client inventory drop: passed")),
            "primary inventory-drop phase must record selected-item drop evidence"
        );
    }

    @Test
    void twoClientInventoryDropObserveUsesMarker() throws Exception {
        Path runDir = Files.createTempDirectory("playable-38-drop-observe");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient client = new FakeScenarioClient();
        client.logItemId = "minecraft:birch_log";
        client.planksItemId = "minecraft:birch_planks";
        ClientScenarioReport primary = new PlayableRealClientLoopScenario().run(
            "playable-38-two-client-inventory-drop-primary",
            screenshotsDir,
            client
        );
        assertEquals("passed", primary.result());
        client.operations.clear();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-38-two-client-inventory-drop-observe",
            screenshotsDir,
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            client.operations.contains("waitDrop:minecraft:birch_log:playable-two-client-inventory-drop-marker"),
            "secondary observe phase must wait for the primary client's dropped item entity"
        );
        assertFalse(client.usedDebugSetup(), "inventory drop observe phase must not use debug setup");
    }

    @Test
    void twoClientInventoryDropSecondaryPickupUsesMarker() throws Exception {
        Path runDir = Files.createTempDirectory("playable-38-drop-pickup");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient client = new FakeScenarioClient();
        client.logItemId = "minecraft:birch_log";
        client.planksItemId = "minecraft:birch_planks";
        ClientScenarioReport primary = new PlayableRealClientLoopScenario().run(
            "playable-38-two-client-inventory-drop-primary",
            screenshotsDir,
            client
        );
        assertEquals("passed", primary.result());
        client.operations.clear();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-38-two-client-inventory-drop-secondary-pickup",
            screenshotsDir,
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            client.operations.contains("collect:minecraft:birch_log:minecraft:birch_log:1"),
            "secondary pickup phase must collect the primary client's dropped item"
        );
        assertFalse(client.usedDebugSetup(), "inventory drop pickup phase must not use debug setup");
    }

    @Test
    void twoClientInventoryDropGoneObserveFailsClosedWithoutMarker() {
        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-38-two-client-inventory-drop-gone-observe",
            Path.of("build/tmp/playable-38-missing/screenshots"),
            new FakeScenarioClient()
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("missing inventory drop marker")),
            "inventory drop gone-observe phase must fail closed when the marker is absent"
        );
    }

    @Test
    void ironChestplateEquipCarriesEnoughPlanksForEightSmeltsWithoutPostFurnaceLogTrip() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.failPostFurnaceLogApproach = true;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-27-earned-iron-chestplate-equip",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("iron chestplate fuel planks: passed")),
            "iron chestplate progression should carry enough planks for eight smelts"
        );
        assertFalse(
            report.observations().stream().anyMatch(entry -> entry.contains("natural log approach: failed")),
            "iron chestplate progression should not need a post-furnace log trip for fuel"
        );
        assertEquals(
            4,
            client.operations.stream()
                    .filter("moveToContainer:1:minecraft:oak_planks:1"::equals)
                    .count(),
            "iron chestplate progression should refresh fuel every other ingot and avoid a post-furnace log trip"
        );
    }

    @Test
    void naturalLogPickupRetryContinuesAfterLeafPickupNoise() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.failedLogPickupsRemaining = 1;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-02a-natural-log-to-planks",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("held=minecraft:birch_leaves x5")),
            "log retry should record the noisy leaf pickup that did not satisfy log inventory truth"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("natural log break/drop/pickup: passed")),
            "log retry should continue until a real log pickup restores inventory"
        );
        assertEquals(
            2,
            client.operations.stream()
                    .filter("collect:minecraft:oak_log:minecraft:oak_log:1"::equals)
                    .count(),
            "log retry should collect again after the failed pickup"
        );
    }

    @Test
    void ironChestplateEquipRetriesMissedRawIronPickupUntilIngotTarget() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.failedRawIronPickupsRemaining = 1;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-27-earned-iron-chestplate-equip",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("natural iron ore break/drop/pickup: failed")),
            "iron chestplate progression should record a missed raw iron pickup before retrying"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains(
                "pickup_detail=player=(10.0,79.0,7.0)"
            )),
            "a missed pickup must retain the final player/drop diagnostic from the client"
        );
        assertTrue(
            client.operations.stream()
                    .filter("breakVisible:minecraft:iron_ore:minecraft:raw_iron"::equals)
                    .count() >= 9,
            "one missed raw iron pickup should cause one extra natural iron ore mining attempt"
        );
    }

    @Test
    void ironChestplateEquipHandlesStackedRawIronPickupDuringSmelting() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.rawIronPickupBatchSize = 4;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-27-earned-iron-chestplate-equip",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("raw_iron_after=4")),
            "regression setup must prove one pickup can restore a stacked raw iron count"
        );
        assertFalse(
            report.observations().stream().anyMatch(entry -> entry.contains("furnace raw iron input remainder clear: failed")),
            "stacked raw iron in the furnace input must not fail a one-ingot smelt"
        );
        assertTrue(
            client.operations.contains("moveFromContainer:0:minecraft:raw_iron:1"),
            "stacked raw iron remainder should be restored from the furnace input before taking the ingot"
        );
    }

    @Test
    void ironSwordRestartBeforeWritesMarkerAndCraftsIronSwordWithoutDebugSetup() throws Exception {
        Path runDir = Files.createTempDirectory("playable-25-before");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-25-iron-sword-save-restart-before",
            screenshotsDir,
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertFalse(client.usedDebugSetup(), "iron sword restart before phase must not use debug setup");
        assertTrue(
            Files.isRegularFile(runDir.resolve("playable-03-save-restart-marker.properties")),
            "iron sword restart before phase must write the crafted table marker for after phase"
        );
        assertTrue(
            client.operations.contains("recipe:7:57:false"),
            "before phase must craft the earned iron sword before restart"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("iron sword recipe: passed")),
            "before phase must record iron sword recipe evidence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("runner-managed restart: pending")),
            "before phase must record that the runner owns the restart boundary"
        );
    }

    @Test
    void ironSwordRestartAfterChecksPersistedMarkerAndIronSwordInventory() throws Exception {
        Path runDir = Files.createTempDirectory("playable-25-after");
        Path screenshotsDir = runDir.resolve("screenshots");
        FakeScenarioClient client = new FakeScenarioClient();
        ClientScenarioReport before = new PlayableRealClientLoopScenario().run(
            "playable-25-iron-sword-save-restart-before",
            screenshotsDir,
            client
        );
        assertEquals("passed", before.result());
        client.operations.clear();

        ClientScenarioReport after = new PlayableRealClientLoopScenario().run(
            "playable-25-iron-sword-save-restart-after",
            screenshotsDir,
            client
        );

        assertEquals(
            "passed",
            after.result(),
            () -> String.join("\n", after.observations()) + "\noperations=" + client.operations
        );
        assertTrue(
            client.operations.contains("waitBlock:restart-marker:minecraft:crafting_table"),
            "after phase must verify the persisted crafting table marker"
        );
        assertTrue(
            client.operations.contains("count:minecraft:iron_sword"),
            "after phase must read the persisted iron sword inventory count"
        );
        assertFalse(client.usedDebugSetup(), "iron sword restart after phase must not use debug setup");
        assertTrue(
            after.observations().stream().anyMatch(entry -> entry.contains("restart marker persistence: passed")),
            "after phase must record persisted marker observation"
        );
        assertTrue(
            after.observations().stream().anyMatch(entry -> entry.contains("iron sword inventory persistence: passed")),
            "after phase must record persisted iron sword inventory observation"
        );
    }

    @Test
    void ironSwordRestartAfterFailsClosedWithoutMarker() {
        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-25-iron-sword-save-restart-after",
            Path.of("build/tmp/playable-25-missing/screenshots"),
            new FakeScenarioClient()
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("missing restart marker")),
            "after phase must fail closed when marker file is absent"
        );
    }

    @Test
    void visibleItemDropCollectionClearsFoliageObstaclesWhileWalkingToDrop() throws Exception {
        String source = Files.readString(Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftScenarioClient.java"
        ));
        String method = source.substring(
            source.indexOf("public ScenarioBreakResult collectVisibleItemDrop("),
            source.indexOf("public boolean waitForNoVisibleItemDrop(")
        );

        assertTrue(
            method.contains("clearFoliageObstacleTowardOnClientThread"),
            "visible item pickup must clear foliage obstacles like approachBlock; real-client log drops can be visible but unreachable"
        );
    }

    @Test
    void visibleItemDropCollectionUsesInventoryCountAsPickupTruth() throws Exception {
        String source = Files.readString(Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftScenarioClient.java"
        ));
        String method = source.substring(
            source.indexOf("public ScenarioBreakResult collectVisibleItemDrop("),
            source.indexOf("public boolean waitForNoVisibleItemDrop(")
        );

        assertTrue(
            method.contains("sample.inventoryCount() >= initialCount + expectedSelectedCount"),
            "visible item pickup must prove total inventory count increased"
        );
        assertFalse(
            method.contains("selected.matches(expectedDropItemId, expectedSelectedCount)"),
            "selected stack alone can be stale from a previous pickup and must not prove a new pickup"
        );
    }

    @Test
    void entityApproachClosesCurrentScreenBeforeWalkingToTarget() throws Exception {
        String source = Files.readString(Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftScenarioClient.java"
        ));
        String method = source.substring(
            source.indexOf("public boolean approachEntity("),
            source.indexOf("public ScenarioBreakResult attackEntityUntilDropCollected(")
        );

        assertTrue(
            method.contains("minecraft.screen != null") && method.contains("minecraft.setScreen(null)"),
            "entity approach must clear pause/container screens before driving movement keys"
        );
    }

    @Test
    void blockApproachClosesCurrentScreenBeforeWalkingToTarget() throws Exception {
        String source = Files.readString(Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftScenarioClient.java"
        ));
        String method = source.substring(
            source.indexOf("public boolean approachBlock("),
            source.indexOf("public boolean standOnBlockUntilDeath(")
        );

        assertTrue(
            method.contains("minecraft.screen != null") && method.contains("minecraft.setScreen(null)"),
            "block approach must clear pause/container screens before driving movement keys"
        );
    }

    @Test
    void realClientBlockBreakingAdvancesOnceFromTheClientTickHook() throws Exception {
        String source = Files.readString(Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftScenarioClient.java"
        ));
        String preTick = source.substring(
            source.indexOf("public static void runPreTickActions()"),
            source.indexOf("    @Override", source.indexOf("public static void runPreTickActions()"))
        );

        assertTrue(preTick.contains("minecraft.gameMode.startDestroyBlock"));
        assertTrue(preTick.contains("minecraft.gameMode.continueDestroyBlock"));
        assertTrue(preTick.contains("minecraft.options.keyAttack.setDown(true)"));
        assertFalse(preTick.contains("SERVER_DESTROY_TICKS"));
        assertFalse(preTick.contains("new ServerboundPlayerActionPacket"));
    }

    @Test
    void realClientBlockBreakingFlushesAbortBeforeStartingTheNextTarget() throws Exception {
        String source = Files.readString(Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftScenarioClient.java"
        ));
        String helper = source.substring(
            source.indexOf("private BlockBreakAutomation startBlockBreakAfterReset("),
            source.indexOf("public ScenarioBreakResult breakBlock(")
        );

        int release = helper.indexOf("minecraft.options.keyAttack.setDown(false)");
        int stop = helper.indexOf("minecraft.gameMode.stopDestroyBlock()");
        int install = helper.indexOf("ACTIVE_BLOCK_BREAK.set(action)", stop);
        int awaitStart = helper.indexOf("awaitBlockBreakStarted(action, deadlineNanos)", install);

        assertTrue(release >= 0 && release < stop);
        assertTrue(stop < install && install < awaitStart);
    }

    @Test
    void realClientBlockBreakingIsDrivenBeforeVanillaHandlesAttackInput() throws Exception {
        String clientSource = Files.readString(Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftScenarioClient.java"
        ));
        String modSource = Files.readString(Path.of(
            "../fabric-agent/src/main/java/dev/solaris/agent/neoforge/SolarisClientAgentMod.java"
        ));
        String mixinSource = Files.readString(Path.of(
            "../fabric-agent/src/main/java/dev/solaris/agent/mixin/MinecraftBlockBreakMixin.java"
        ));
        String mixinConfig = Files.readString(Path.of(
            "../fabric-agent/src/main/resources/solaris-client-agent.mixins.json"
        ));
        String breakMethods = clientSource.substring(
            clientSource.indexOf("private BlockBreakAutomation startBlockBreakAfterReset("),
            clientSource.indexOf("public boolean waitForVisibleItemDrop(")
        );

        assertTrue(modSource.contains("ClientTickEvent.Pre"));
        assertTrue(modSource.contains("MinecraftScenarioClient.runPreTickActions()"));
        assertTrue(mixinSource.contains("@Inject"));
        assertTrue(mixinSource.contains("MinecraftScenarioClient.hasActiveBlockBreak()"));
        assertTrue(mixinSource.contains("callbackInfo.cancel()"));
        assertTrue(mixinConfig.contains("MinecraftBlockBreakMixin"));
        assertFalse(clientSource.contains("minecraft.mouseHandler.grabMouse()"));
        assertTrue(clientSource.contains("started.complete(null)"));
        assertTrue(breakMethods.contains("awaitBlockBreakStarted"));
        assertFalse(breakMethods.contains("minecraft.gameMode.continueDestroyBlock"));
    }

    @Test
    void realClientBlockBreakingReleasesAttackAsSoonAsTheTargetIsAir() throws Exception {
        String source = Files.readString(Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftScenarioClient.java"
        ));
        String method = source.substring(
            source.indexOf("public ScenarioBreakResult breakBlockUntilDropVisible("),
            source.indexOf("public boolean waitForVisibleItemDrop(")
        );

        int airObserved = method.indexOf("becameAir |= sample.becameAir()");
        int earlyStop = method.indexOf("stopBlockBreak(action)", airObserved);
        int dropSuccess = method.indexOf("if (becameAir && sawDrop)", earlyStop);

        assertTrue(airObserved >= 0 && airObserved < earlyStop);
        assertTrue(earlyStop < dropSuccess);
    }

    @Test
    void realClientBreakableScanIncludesNonSolidPlants() throws Exception {
        String source = Files.readString(Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftScenarioClient.java"
        ));
        String helper = source.substring(
            source.indexOf("private static ScenarioBlockTarget breakableBlockTarget("),
            source.indexOf("private static boolean isBreakFaceAccessible(")
        );

        assertTrue(helper.contains("isNonAirLoaded(target)"));
        assertFalse(helper.contains("isSolidLoaded(target)"));
    }

    @Test
    void realClientBreakableScanTargetsTheNearestAccessibleFace() throws Exception {
        String source = Files.readString(Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftScenarioClient.java"
        ));
        String helper = source.substring(
            source.indexOf("private static ScenarioBlockTarget breakableBlockTarget("),
            source.indexOf("private static boolean isBreakFaceAccessible(")
        );

        assertTrue(
            helper.contains("closestDirection")
                && helper.contains("distance < closestDistance"),
            "breakable scan must target the accessible face nearest to the player's eyes"
        );
    }

    @Test
    void minecraftScenarioPlayerVisibilityFiltersByRequestedPlayerName() throws Exception {
        String source = Files.readString(Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftScenarioClient.java"
        ));
        String method = source.substring(
            source.indexOf("private static ScenarioPlayerObservation visiblePlayerOnClientThread("),
            source.indexOf("private static Entity entityByIdOnClientThread(")
        );

        assertTrue(
            method.contains("playerName.equals(player.getPlainTextName())"),
            "player visibility scans must filter by requested player name, not return the first remote player"
        );
    }

    @Test
    void closeCurrentScreenDisablesLostFocusPauseBeforeClearingScreen() throws Exception {
        String scenarioClientSource = Files.readString(Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftScenarioClient.java"
        ));
        String facadeSource = Files.readString(Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftClientFacade.java"
        ));
        String helper = scenarioClientSource.substring(
            scenarioClientSource.indexOf("static boolean closeCurrentScreenOnClientThread("),
            scenarioClientSource.indexOf("public int activeContainerId()")
        );

        assertTrue(
            helper.contains("minecraft.options.pauseOnLostFocus = false")
                && helper.contains("minecraft.setScreen(null)"),
            "real-client automation must disable lost-focus pause before clearing PauseScreen"
        );
        assertTrue(
            facadeSource.contains("MinecraftScenarioClient.closeCurrentScreenOnClientThread(minecraft)"),
            "bridge close_screen must use the same pause-safe close helper as scenarios"
        );
    }

    @Test
    void containerMutationRequiresAuthoritativeServerStateAdvance() {
        assertFalse(
            ScenarioClient.authoritativeContainerUpdateMatches(4, 7, 4, 7, true),
            "the locally predicted slot mutation must not count as a server commit"
        );
        assertFalse(
            ScenarioClient.authoritativeContainerUpdateMatches(4, 7, 5, 8, true),
            "an update for a different open container must not confirm the click"
        );
        assertFalse(
            ScenarioClient.authoritativeContainerUpdateMatches(4, 7, 4, 8, false),
            "a server response with the wrong slot contents must fail closed"
        );
        assertTrue(
            ScenarioClient.authoritativeContainerUpdateMatches(4, 7, 4, 8, true),
            "the expected slot after a server state advance confirms the click"
        );
    }

    @Test
    void containerMoveMethodsWaitForAuthoritativeServerUpdate() throws Exception {
        String source = Files.readString(Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftScenarioClient.java"
        ));
        String deposit = source.substring(
            source.indexOf("public boolean moveSelectedItemToContainerSlot("),
            source.indexOf("public boolean waitForContainerSlot(")
        );
        String withdraw = source.substring(
            source.indexOf("public boolean moveContainerSlotToInventory("),
            source.indexOf("public boolean waitForContainerSlotEmpty(")
        );

        assertTrue(deposit.contains("waitForAuthoritativeContainerUpdate"));
        assertTrue(withdraw.contains("waitForAuthoritativeContainerUpdate"));
    }

    @Test
    void hotbarSwapWaitsForAuthoritativeServerUpdate() throws Exception {
        String source = Files.readString(Path.of(
            "src/main/java/dev/solaris/agent/javaagent/MinecraftScenarioClient.java"
        ));
        String selection = source.substring(
            source.indexOf("public ScenarioHeldItem selectHotbarItem("),
            source.indexOf("public ScenarioBlockTarget dropSelectedItem(")
        );

        assertTrue(
            selection.contains("waitForAuthoritativeHotbarSelection"),
            "a predicted inventory SWAP must wait for the server container response"
        );
    }

    @Test
    void craftingTableOpenUsesEarnedInventoryItemWithoutDebugSetup() {
        FakeScenarioClient client = new FakeScenarioClient();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-02b-natural-crafting-table-open",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "selected",
            "count:minecraft:oak_log",
            "count:minecraft:oak_planks",
            "count:minecraft:crafting_table",
            "count:minecraft:wooden_pickaxe",
            "findBreakable:" + SUPPORTED_LOGS + ":within-survival-reach",
            "approach:minecraft:oak_log:natural-log",
            "breakVisible:minecraft:oak_log:minecraft:oak_log",
            "collect:minecraft:oak_log:minecraft:oak_log:1",
            "count:minecraft:oak_log",
            "count:minecraft:oak_planks",
            "recipe:0:18:false",
            "waitCount:minecraft:oak_log:0",
            "waitCount:minecraft:oak_planks:4",
            "count:minecraft:oak_planks",
            "count:minecraft:crafting_table",
            "recipe:0:10:false",
            "waitCount:minecraft:oak_planks:0",
            "waitCount:minecraft:crafting_table:1",
            "selectHotbar:minecraft:crafting_table:1",
            "findDry:within-survival-reach",
            "use:minecraft:crafting_table:table-clicked",
            "waitBlock:table-target:minecraft:crafting_table",
            "use:minecraft:crafting_table:table-target",
            "screen:net.minecraft.client.gui.screens.inventory.CraftingScreen",
            "closeScreen"
        ), client.operations);
        assertFalse(client.usedDebugSetup(), "crafting-table playable probe must not use debug setup helpers");
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("crafting table recipe: passed")),
            "crafting-table probe must record recipe evidence"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("crafting table open: passed")),
            "crafting-table probe must record placed/opened table evidence"
        );
    }

    @Test
    void craftingTablePlacementFailureDoesNotIssueASecondInteraction() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.craftingTablePlacementObserved = false;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-02b-natural-crafting-table-open",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("failed", report.result());
        assertTrue(client.operations.contains("use:minecraft:crafting_table:table-clicked"));
        assertFalse(client.operations.contains("use:minecraft:crafting_table:table-target"));
        assertFalse(client.operations.contains("screen:net.minecraft.client.gui.screens.inventory.CraftingScreen"));
        assertFalse(client.operations.contains("closeScreen"));
    }

    @Test
    void craftingTablePlacementWaitsForObservedBlockAfterLocalFailureResult() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.craftingTablePlaceUseResult = "fail";

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-02b-natural-crafting-table-open",
            Path.of("run/screenshots"),
            client
        );

        assertEquals("passed", report.result());
        assertTrue(client.operations.contains("waitBlock:table-target:minecraft:crafting_table"));
        assertTrue(client.operations.contains("use:minecraft:crafting_table:table-target"));
    }

    @Test
    void blocksUnknownScenarioIds() {
        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-unknown",
            Path.of("run/screenshots"),
            new FakeScenarioClient()
        );

        assertEquals("blocked", report.result());
        assertTrue(report.observations().get(0).contains("unsupported scenario"));
    }

    @Test
    void saveRestartBeforeUsesNaturalCraftedTableAsPersistenceMarker() {
        FakeScenarioClient client = new FakeScenarioClient();
        Path screenshotsDir = Path.of("build/tmp/playable-03-test/screenshots");

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-03-save-restart-before",
            screenshotsDir,
            client
        );

        assertEquals("passed", report.result());
        assertFalse(client.usedDebugSetup(), "playable save/restart before phase must not use debug setup");
        assertTrue(
            screenshotsDir.getParent().resolve("playable-03-save-restart-marker.properties").toFile().isFile(),
            "before phase must write marker coordinates for after phase"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("restart marker placement: passed")),
            "before phase must record marker placement"
        );
    }

    @Test
    void saveRestartAfterChecksPersistedMarkerAndWoodenPickaxeInventory() {
        FakeScenarioClient client = new FakeScenarioClient();
        Path screenshotsDir = Path.of("build/tmp/playable-03-test/screenshots");
        new PlayableRealClientLoopScenario().run(
            "playable-03-save-restart-before",
            screenshotsDir,
            client
        );
        client.operations.clear();

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-03-save-restart-after",
            screenshotsDir,
            client
        );

        assertEquals("passed", report.result());
        assertEquals(List.of(
            "selected",
            "count:minecraft:oak_log",
            "count:minecraft:oak_planks",
            "count:minecraft:crafting_table",
            "count:minecraft:wooden_pickaxe",
            "waitBlock:restart-marker:minecraft:crafting_table",
            "count:minecraft:wooden_pickaxe"
        ), client.operations);
        assertFalse(client.usedDebugSetup(), "playable save/restart after phase must not use debug setup");
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("restart marker persistence: passed")),
            "after phase must record persisted marker observation"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("inventory persistence: passed")),
            "after phase must record persisted wooden pickaxe inventory observation"
        );
    }

    @Test
    void saveRestartAfterFailsClosedWithoutMarker() {
        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-03-save-restart-after",
            Path.of("build/tmp/playable-03-missing/screenshots"),
            new FakeScenarioClient()
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("missing restart marker")),
            "after phase must fail closed when marker file is absent"
        );
    }

    @Test
    void stonecutterConservesNormalPickupAndQuickMoveAcrossReopen() {
        assertStonecutterFakeScreenAndContainerFollowUseCloseAndReopenActions();
        FakeScenarioClient client = new FakeScenarioClient();
        client.stonecutterSlabOfferId = 2;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-47-stonecutter-conservation",
            Path.of("run/screenshots"),
            client
        );

        assertEquals(
            "passed",
            report.result(),
            () -> String.join("\n", report.observations()) + "\noperations=" + client.operations
        );
        assertEquals(0, client.cobblestones, "all three stonecutter inputs must be consumed exactly once");
        assertEquals(6, client.cobblestoneSlabs, "one normal pickup plus max quick-move must produce six slabs");
        assertEquals(
            2,
            client.operations.stream()
                .filter("screen:net.minecraft.client.gui.screens.inventory.StonecutterScreen"::equals)
                .count(),
            "the menu must be observed through client state before and after close"
        );
        int normalPickup = client.operations.indexOf("moveFromContainer:1:minecraft:cobblestone_slab:2");
        int close = client.operations.indexOf("closeScreen");
        int reopen = client.operations.lastIndexOf(
            "screen:net.minecraft.client.gui.screens.inventory.StonecutterScreen"
        );
        int quickMove = client.operations.indexOf("quickMoveContainer:1");
        assertTrue(
            normalPickup >= 0 && normalPickup < close && close < reopen && reopen < quickMove,
            "normal pickup must precede the close/reopen boundary and quick-move must follow it"
        );
        for (int offerId = 0; offerId <= client.stonecutterSlabOfferId; offerId++) {
            String operation = "containerButton:" + offerId;
            assertEquals(
                2,
                client.operations.stream().filter(operation::equals).count(),
                "the matching offer must be discovered from observed output after each menu open"
            );
        }
        assertFalse(
            client.operations.stream().anyMatch(operation -> operation.startsWith("wait_ticks:")),
            "stonecutter synchronization must remain event-driven"
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("stonecutter normal pickup: passed"))
        );
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("stonecutter conservation: passed"))
        );

        FakeScenarioClient staleReopen = new FakeScenarioClient();
        staleReopen.stonecutterSlabOfferId = 2;
        staleReopen.reuseStonecutterContainerIdOnReopen = true;
        ClientScenarioReport staleReport = new PlayableRealClientLoopScenario().run(
            "playable-47-stonecutter-conservation",
            Path.of("run/screenshots"),
            staleReopen
        );
        assertEquals("failed", staleReport.result(), "same-ID reopen must not prove a new menu lifecycle");
        assertTrue(
            staleReport.observations().stream().anyMatch(entry ->
                entry.contains("stonecutter close/reopen conservation: failed")
            )
        );
    }

    private static void assertStonecutterFakeScreenAndContainerFollowUseCloseAndReopenActions() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.stonecutterSlabOfferId = 2;
        String screen = "net.minecraft.client.gui.screens.inventory.StonecutterScreen";
        ScenarioBlockPair pair = client.findUnobstructedPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);

        assertFalse(client.waitForScreenClassName(screen, Duration.ZERO));
        client.useItemOn(pair.clicked(), new ScenarioHeldItem("minecraft:stonecutter", 1));
        assertFalse(client.waitForScreenClassName(screen, Duration.ZERO), "placing the block must not open its menu");

        ScenarioBlockTarget stonecutter = new ScenarioBlockTarget(
            pair.target().x(),
            pair.target().y(),
            pair.target().z(),
            "up",
            "stonecutter-target",
            "minecraft:stonecutter"
        );
        client.useItemOn(stonecutter, new ScenarioHeldItem("minecraft:cobblestone", 1));
        assertTrue(client.waitForScreenClassName(screen, Duration.ZERO));
        int firstContainerId = client.activeContainerId();
        client.giveAndSelect("minecraft:cobblestone", 1, 1, Duration.ZERO);
        assertTrue(client.moveSelectedItemToContainerSlot(0, "minecraft:cobblestone", 1, Duration.ZERO));
        assertFalse(
            client.waitForContainerSlot(1, "minecraft:cobblestone_slab", 2, Duration.ZERO),
            "waiting must not materialize stonecutter output before an offer action"
        );
        assertTrue(client.clickContainerButton(0, Duration.ZERO));
        assertFalse(client.waitForContainerSlot(1, "minecraft:cobblestone_slab", 2, Duration.ZERO));
        assertTrue(client.clickContainerButton(2, Duration.ZERO));
        assertTrue(client.waitForContainerSlot(1, "minecraft:cobblestone_slab", 2, Duration.ZERO));

        assertTrue(client.closeCurrentScreen(Duration.ZERO));
        assertFalse(client.waitForScreenClassName(screen, Duration.ZERO));

        client.useItemOn(stonecutter, new ScenarioHeldItem("minecraft:air", 0));
        assertTrue(client.waitForScreenClassName(screen, Duration.ZERO));
        assertTrue(client.activeContainerId() > firstContainerId, "reopen must create a new container state");
    }

    @Test
    void generatedRuinCacheBeforeQuickMovesExactLootWithoutDebugSetup() throws Exception {
        FakeScenarioClient client = new FakeScenarioClient();
        client.configureGeneratedRuinCache();
        Path screenshotsDir = Path.of("build/tmp/playable-46-before/screenshots");
        Files.deleteIfExists(screenshotsDir.getParent().resolve("playable-46-generated-ruin-cache-marker.properties"));

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-46-generated-ruin-cache-before",
            screenshotsDir,
            client
        );

        assertEquals("passed", report.result(), () -> String.join("\n", report.observations()));
        assertFalse(client.usedDebugSetup(), "generated ruin cache must not use debug setup");
        assertEquals(1, client.diamonds);
        assertEquals(4, client.lapisLazuli);
        assertEquals(2, client.breads);
        assertTrue(
            Files.isRegularFile(screenshotsDir.getParent().resolve("playable-46-generated-ruin-cache-marker.properties")),
            "before phase must persist the exact chest target and cleared loot slots"
        );
        assertTrue(client.operations.contains("approachPosition:72:8"));
        assertTrue(client.operations.contains("quickMoveContainer:0"));
        assertTrue(client.operations.contains("quickMoveContainer:1"));
        assertTrue(client.operations.contains("quickMoveContainer:2"));
    }

    @Test
    void generatedRuinCacheBeforeFailsWhenExactChestIsAbsent() throws Exception {
        FakeScenarioClient client = new FakeScenarioClient();
        client.configureGeneratedRuinCache();
        client.generatedRuinChestPresent = false;

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-46-generated-ruin-cache-before",
            Path.of("build/tmp/playable-46-before-missing/screenshots"),
            client
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("exact generated chest Y: failed")),
            "before phase must fail closed when client world state does not expose the chest"
        );
    }

    @Test
    void generatedRuinCacheAfterProvesInventoryAndChestPersistence() throws Exception {
        FakeScenarioClient client = new FakeScenarioClient();
        client.configureGeneratedRuinCache();
        Path screenshotsDir = Path.of("build/tmp/playable-46-after/screenshots");
        Files.deleteIfExists(screenshotsDir.getParent().resolve("playable-46-generated-ruin-cache-marker.properties"));

        ClientScenarioReport before = new PlayableRealClientLoopScenario().run(
            "playable-46-generated-ruin-cache-before",
            screenshotsDir,
            client
        );
        ClientScenarioReport after = new PlayableRealClientLoopScenario().run(
            "playable-46-generated-ruin-cache-after",
            screenshotsDir,
            client
        );

        assertEquals("passed", before.result(), () -> String.join("\n", before.observations()));
        assertEquals("passed", after.result(), () -> String.join("\n", after.observations()));
        assertEquals(1, client.diamonds);
        assertEquals(4, client.lapisLazuli);
        assertEquals(2, client.breads);
        assertTrue(
            after.observations().stream().anyMatch(entry -> entry.contains("generated ruin cache persistence: passed")),
            "after phase must report both exact inventory counts and empty persisted slots"
        );
    }

    @Test
    void generatedRuinCacheAfterFailsWhenPersistedChestSlotIsNotEmpty() throws Exception {
        FakeScenarioClient client = new FakeScenarioClient();
        client.configureGeneratedRuinCache();
        Path screenshotsDir = Path.of("build/tmp/playable-46-after-nonempty/screenshots");
        Files.deleteIfExists(screenshotsDir.getParent().resolve("playable-46-generated-ruin-cache-marker.properties"));
        ClientScenarioReport before = new PlayableRealClientLoopScenario().run(
            "playable-46-generated-ruin-cache-before",
            screenshotsDir,
            client
        );
        client.containerItemIds[1] = "minecraft:lapis_lazuli";
        client.containerCounts[1] = 4;

        ClientScenarioReport after = new PlayableRealClientLoopScenario().run(
            "playable-46-generated-ruin-cache-after",
            screenshotsDir,
            client
        );

        assertEquals("passed", before.result(), () -> String.join("\n", before.observations()));
        assertEquals("failed", after.result());
        assertTrue(
            after.observations().stream().anyMatch(entry -> entry.contains("chest_slots_empty=false")),
            "after phase must fail closed when an expected loot slot remains populated"
        );
    }

    @Test
    void generatedRuinCacheAfterFailsWhenAnUnrecordedChestSlotIsNotEmpty() throws Exception {
        FakeScenarioClient client = new FakeScenarioClient();
        client.configureGeneratedRuinCache();
        Path screenshotsDir = Path.of("build/tmp/playable-46-after-unrecorded-slot/screenshots");
        Files.deleteIfExists(screenshotsDir.getParent().resolve("playable-46-generated-ruin-cache-marker.properties"));
        ClientScenarioReport before = new PlayableRealClientLoopScenario().run(
            "playable-46-generated-ruin-cache-before",
            screenshotsDir,
            client
        );
        client.containerItemIds[26] = "minecraft:diamond";
        client.containerCounts[26] = 1;

        ClientScenarioReport after = new PlayableRealClientLoopScenario().run(
            "playable-46-generated-ruin-cache-after",
            screenshotsDir,
            client
        );

        assertEquals("passed", before.result(), () -> String.join("\n", before.observations()));
        assertEquals("failed", after.result());
        assertTrue(
            after.observations().stream().anyMatch(entry -> entry.contains("chest_slots_empty=false")),
            "after phase must inspect all 27 chest slots, not just marker-listed loot slots"
        );
    }

    @Test
    void generatedRuinCacheAfterRejectsMarkerOutsideFixedRuinColumn() throws Exception {
        Path screenshotsDir = preparedGeneratedRuinCacheMarkerPath("marker-outside-column");
        writeGeneratedRuinCacheMarker(screenshotsDir, 73, 8, "up", 0, 1, 2);

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-46-generated-ruin-cache-after",
            screenshotsDir,
            persistedGeneratedRuinCacheClient()
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("invalid generated ruin cache marker")),
            "after phase must reject markers outside the fixed generated ruin column before interacting"
        );
    }

    @Test
    void generatedRuinCacheAfterRejectsMarkerWithUnsupportedFace() throws Exception {
        Path screenshotsDir = preparedGeneratedRuinCacheMarkerPath("marker-unsupported-face");
        writeGeneratedRuinCacheMarker(screenshotsDir, 72, 8, "northwest", 0, 1, 2);

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-46-generated-ruin-cache-after",
            screenshotsDir,
            persistedGeneratedRuinCacheClient()
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("invalid generated ruin cache marker")),
            "after phase must reject unsupported marker faces before interacting"
        );
    }

    @Test
    void generatedRuinCacheAfterRejectsMarkerSlotOutsideChestBounds() throws Exception {
        Path screenshotsDir = preparedGeneratedRuinCacheMarkerPath("marker-slot-outside-chest");
        writeGeneratedRuinCacheMarker(screenshotsDir, 72, 8, "up", 0, 1, 27);

        ClientScenarioReport report = new PlayableRealClientLoopScenario().run(
            "playable-46-generated-ruin-cache-after",
            screenshotsDir,
            persistedGeneratedRuinCacheClient()
        );

        assertEquals("failed", report.result());
        assertTrue(
            report.observations().stream().anyMatch(entry -> entry.contains("invalid generated ruin")),
            "after phase must reject marker slots outside the 27 chest slots before interacting"
        );
    }

    @Test
    void generatedRuinCacheRunnerForcesAnIsolatedWorldAndRestartPrerequisites() throws Exception {
        String runner = Files.readString(Path.of("../../../tools/run-real-client-regression.sh"));

        assertTrue(runner.contains("playable-46-generated-ruin-cache"));
        assertTrue(runner.contains("fresh_world_dir=\"$run_dir/world\""));
        assertTrue(
            runner.contains("if [[ \"$AGENT_SCENARIO\" == \"playable-46-generated-ruin-cache\" ]]; then")
                && runner.contains("printf '[]\\n'")
                && runner.contains("server_op_users=NONE")
                && runner.contains("playable-46 observations require server_op_users=NONE"),
            "generated ruin cache must run its real client without operator access"
        );
        assertTrue(runner.contains("playable-46-generated-ruin-cache-before"));
        assertTrue(runner.contains("playable-46-generated-ruin-cache-after"));
    }

    private static Path preparedGeneratedRuinCacheMarkerPath(String label) throws Exception {
        Path screenshotsDir = Path.of("build/tmp/playable-46-" + label + "/screenshots");
        Files.createDirectories(screenshotsDir.getParent());
        Files.deleteIfExists(screenshotsDir.getParent().resolve("playable-46-generated-ruin-cache-marker.properties"));
        return screenshotsDir;
    }

    private static FakeScenarioClient persistedGeneratedRuinCacheClient() {
        FakeScenarioClient client = new FakeScenarioClient();
        client.configureGeneratedRuinCache();
        client.diamonds = 1;
        client.lapisLazuli = 4;
        client.breads = 2;
        Arrays.fill(client.containerItemIds, null);
        Arrays.fill(client.containerCounts, 0);
        return client;
    }

    private static void writeGeneratedRuinCacheMarker(
        Path screenshotsDir,
        int x,
        int z,
        String face,
        int diamondSlot,
        int lapisSlot,
        int breadSlot
    ) throws Exception {
        Files.createDirectories(screenshotsDir.getParent());
        Files.writeString(
            screenshotsDir.getParent().resolve("playable-46-generated-ruin-cache-marker.properties"),
            "x=" + x + "\n"
                + "y=61\n"
                + "z=" + z + "\n"
                + "face=" + face + "\n"
                + "diamond_count=1\n"
                + "diamond_slot=" + diamondSlot + "\n"
                + "lapis_lazuli_count=4\n"
                + "lapis_lazuli_slot=" + lapisSlot + "\n"
                + "bread_count=2\n"
                + "bread_slot=" + breadSlot + "\n"
        );
    }

    private static final class FakeScenarioClient implements ScenarioClient {
        final List<String> operations = new ArrayList<>();
        String logItemId = "minecraft:oak_log";
        String planksItemId = "minecraft:oak_planks";
        int oakLogs = 0;
        int oakPlanks = 0;
        int sticks = 0;
        int craftingTables = 0;
        int woodenPickaxes = 0;
        int woodenSwords = 0;
        int woodenHoes = 0;
        int cobblestones = 0;
        int cobblestoneSlabs = 0;
        int stonePickaxes = 0;
        int stoneSwords = 0;
        int furnaces = 0;
        int rawIron = 0;
        int ironIngots = 0;
        int totalExperience = 0;
        int ironSwords = 0;
        int shields = 0;
        boolean shieldBlockedAttackObserved = true;
        int ironChestplates = 0;
        int chests = 0;
        int charcoals = 0;
        int torches = 0;
        int beef = 0;
        int porkchops = 0;
        int chickens = 0;
        int cookedBeef = 0;
        int cookedPorkchops = 0;
        int cookedChickens = 0;
        String sheepWoolItemId = "minecraft:white_wool";
        String firstObservedSheepWoolItemId = "minecraft:white_wool";
        int sheepWool = 0;
        int sheepBeds = 0;
        int rottenFlesh = 0;
        int doors = 0;
        int signs = 0;
        int campfires = 0;
        int wheatSeeds = 0;
        int wheat = 0;
        int breads = 0;
        int diamonds = 0;
        int lapisLazuli = 0;
        int farmPlots = 0;
        int shortGrassScans = 0;
        int foodLevel = 20;
        float healthAfterHostileCombat = 20.0F;
        boolean visiblePassiveDuringSoak = false;
        boolean visibleHostileDuringSoak = false;
        boolean visibleHostileOutsideReachDuringSoak = false;
        String visibleHostileTypeDuringSoak = "minecraft:zombie";
        boolean tickProgressDuringSoak = true;
        long clientTicks = 0L;
        final String[] containerItemIds = new String[27];
        final int[] containerCounts = new int[27];
        boolean nearLogsAvailable = true;
        long logsUnavailableDuringSoakAfterTick = Long.MAX_VALUE;
        boolean nearStoneAvailable = true;
        boolean fourthLogIsReachableDownFace = false;
        boolean failPostFurnaceLogApproach = false;
        boolean failCloseCurrentScreen = false;
        boolean failCraftingTableApproachForFurnace = false;
        boolean failCraftingTableApproachForCampfire = false;
        boolean failFirstCampfireReserveLogApproach = false;
        boolean failFirstNaturalLogFarApproach = false;
        boolean failFirstNaturalStoneFarApproach = false;
        boolean failedNaturalLogFarApproach = false;
        boolean failedCampfireReserveLogApproach = false;
        boolean failedNaturalStoneFarApproach = false;
        boolean equippedIronChestplate = false;
        int failedCobblestonePickupsRemaining = 0;
        int failedRawIronPickupsRemaining = 0;
        int failedLogPickupsRemaining = 0;
        int failedTorchPickupsRemaining = 0;
        int missingReachableStoneScansRemaining = 0;
        int rawIronPickupBatchSize = 1;
        int droppedWoodenPickaxes = 0;
        int droppedWoodenSwords = 0;
        boolean dieOnNextHostileAttack = false;
        long dieDuringDropBaselineAfterTick = Long.MAX_VALUE;
        boolean deathMaterializedDuringDropBaseline = false;
        boolean woodenPickaxePickupObserved = true;
        boolean woodenPickaxeEntityDisappeared = true;
        boolean postDeathWoodenPickaxeEntityVisible = true;
        boolean postDeathWoodenSwordEntityVisible = true;
        boolean returnPreexistingWoodenPickaxeEntity = false;
        boolean woodenPickaxeIdentityLostBeforePickup = false;
        boolean unrelatedWoodenPickaxePickedUpAfterIdentityLoss = false;
        int preexistingWoodenPickaxeEntityId = -1;
        int preexistingWoodenSwordEntityId = -1;
        int deathDropWoodenPickaxeEntityId = 701;
        int deathDropWoodenSwordEntityId = 702;
        boolean craftingTablePlacementObserved = true;
        String craftingTablePlaceUseResult = "success";
        UUID preexistingWoodenPickaxeEntityUuid =
            UUID.fromString("00000000-0000-0000-0000-000000000700");
        UUID preexistingWoodenSwordEntityUuid =
            UUID.fromString("00000000-0000-0000-0000-000000000710");
        UUID deathDropWoodenPickaxeEntityUuid =
            UUID.fromString("00000000-0000-0000-0000-000000000701");
        UUID deathDropWoodenSwordEntityUuid =
            UUID.fromString("00000000-0000-0000-0000-000000000702");
        int visiblePlayerEntityId = 777;
        int cropSkyLight = 15;
        boolean generatedRuinChestPresent;
        boolean stonecutterPlaced;
        boolean stonecutterMenuOpen;
        boolean stonecutterOfferSelected;
        int stonecutterSlabOfferId;
        String stonecutterScreenClassName;
        int activeContainerId = 7;
        int nextStonecutterContainerId = 8;
        boolean reuseStonecutterContainerIdOnReopen;

        void configureGeneratedRuinCache() {
            generatedRuinChestPresent = true;
            containerItemIds[0] = "minecraft:diamond";
            containerCounts[0] = 1;
            containerItemIds[1] = "minecraft:lapis_lazuli";
            containerCounts[1] = 4;
            containerItemIds[2] = "minecraft:bread";
            containerCounts[2] = 2;
        }

        boolean usedDebugSetup() {
            return operations.stream().anyMatch(operation -> operation.startsWith("debug:"));
        }

        @Override
        public ScenarioHeldItem selectedItem() {
            operations.add("selected");
            return new ScenarioHeldItem("minecraft:air", 0);
        }

        @Override
        public boolean waitForTicks(long ticks, Duration timeout) {
            operations.add("wait_ticks:" + ticks);
            if (tickProgressDuringSoak) {
                clientTicks += ticks;
            }
            return tickProgressDuringSoak;
        }

        @Override
        public long serverGameTime() {
            operations.add("serverGameTime");
            return clientTicks;
        }

        @Override
        public long waitForServerTimeAfter(long baseline, Duration timeout) {
            operations.add("waitForServerTimeAfter:" + baseline);
            if (tickProgressDuringSoak) {
                clientTicks = Math.max(clientTicks, baseline) + 20L;
            }
            return clientTicks;
        }

        @Override
        public int inventoryCount(String itemId) {
            operations.add("count:" + itemId);
            if (logItemId.equals(itemId)) {
                return oakLogs;
            }
            if (planksItemId.equals(itemId)) {
                return oakPlanks;
            }
            if ("minecraft:stick".equals(itemId)) {
                return sticks;
            }
            if ("minecraft:crafting_table".equals(itemId)) {
                return craftingTables;
            }
            if ("minecraft:wooden_pickaxe".equals(itemId)) {
                return woodenPickaxes;
            }
            if ("minecraft:wooden_sword".equals(itemId)) {
                return woodenSwords;
            }
            if ("minecraft:wooden_hoe".equals(itemId)) {
                return woodenHoes;
            }
            if ("minecraft:cobblestone".equals(itemId)) {
                return cobblestones;
            }
            if ("minecraft:cobblestone_slab".equals(itemId)) {
                return cobblestoneSlabs;
            }
            if ("minecraft:stone_pickaxe".equals(itemId)) {
                return stonePickaxes;
            }
            if ("minecraft:stone_sword".equals(itemId)) {
                return stoneSwords;
            }
            if ("minecraft:furnace".equals(itemId)) {
                return furnaces;
            }
            if ("minecraft:raw_iron".equals(itemId)) {
                return rawIron;
            }
            if ("minecraft:iron_ingot".equals(itemId)) {
                return ironIngots;
            }
            if ("minecraft:iron_sword".equals(itemId)) {
                return ironSwords;
            }
            if ("minecraft:shield".equals(itemId)) {
                return shields;
            }
            if ("minecraft:iron_chestplate".equals(itemId)) {
                return ironChestplates;
            }
            if ("minecraft:chest".equals(itemId)) {
                return chests;
            }
            if ("minecraft:charcoal".equals(itemId)) {
                return charcoals;
            }
            if ("minecraft:torch".equals(itemId)) {
                return torches;
            }
            if ("minecraft:beef".equals(itemId)) {
                return beef;
            }
            if ("minecraft:porkchop".equals(itemId)) {
                return porkchops;
            }
            if ("minecraft:chicken".equals(itemId)) {
                return chickens;
            }
            if ("minecraft:cooked_beef".equals(itemId)) {
                return cookedBeef;
            }
            if ("minecraft:cooked_porkchop".equals(itemId)) {
                return cookedPorkchops;
            }
            if ("minecraft:cooked_chicken".equals(itemId)) {
                return cookedChickens;
            }
            if (sheepWoolItemId.equals(itemId)) {
                return sheepWool;
            }
            if (sheepBedItemId().equals(itemId)) {
                return sheepBeds;
            }
            if ("minecraft:rotten_flesh".equals(itemId)) {
                return rottenFlesh;
            }
            if (doorItemId().equals(itemId)) {
                return doors;
            }
            if (signItemId().equals(itemId)) {
                return signs;
            }
            if ("minecraft:campfire".equals(itemId)) {
                return campfires;
            }
            if ("minecraft:wheat_seeds".equals(itemId)) {
                return wheatSeeds;
            }
            if ("minecraft:wheat".equals(itemId)) {
                return wheat;
            }
            if ("minecraft:bread".equals(itemId)) {
                return breads;
            }
            if ("minecraft:diamond".equals(itemId)) {
                return diamonds;
            }
            if ("minecraft:lapis_lazuli".equals(itemId)) {
                return lapisLazuli;
            }
            return 0;
        }

        @Override
        public ScenarioBlockTarget findBreakableBlock(List<String> blockIds, ScenarioReach reach) {
            operations.add("findBreakable:" + String.join("|", blockIds) + ":" + reach.label());
            if (blockIds.contains("minecraft:short_grass")) {
                int offset = shortGrassScans++;
                return new ScenarioBlockTarget(
                    2 + offset,
                    65,
                    2,
                    "up",
                    "natural-short-grass-" + offset,
                    "minecraft:short_grass"
                );
            }
            if (blockIds.contains("minecraft:stone")) {
                if (reach == ScenarioReach.WITHIN_SURVIVAL_REACH && missingReachableStoneScansRemaining > 0) {
                    missingReachableStoneScansRemaining -= 1;
                    return null;
                }
                if (reach == ScenarioReach.WITHIN_SURVIVAL_REACH && !nearStoneAvailable) {
                    return null;
                }
                if (reach == ScenarioReach.OUTSIDE_SURVIVAL_REACH) {
                    return new ScenarioBlockTarget(9, 62, 6, "up", "far-natural-stone", "minecraft:stone");
                }
                return new ScenarioBlockTarget(5, 62, 6, "up", "natural-stone", "minecraft:stone");
            }
            if (blockIds.contains("minecraft:campfire") && campfires > 0) {
                return new ScenarioBlockTarget(1, 65, 1, "up", "campfire-target", "minecraft:campfire");
            }
            if (blockIds.contains("minecraft:iron_ore")) {
                if (reach == ScenarioReach.OUTSIDE_SURVIVAL_REACH) {
                    return new ScenarioBlockTarget(11, 61, 7, "up", "far-natural-iron-ore", "minecraft:iron_ore");
                }
                return new ScenarioBlockTarget(6, 61, 7, "up", "natural-iron-ore", "minecraft:iron_ore");
            }
            if (!blockIds.contains(logItemId)) {
                return null;
            }
            if (woodenSwords > 0 && clientTicks >= logsUnavailableDuringSoakAfterTick) {
                return null;
            }
            if (
                failFirstCampfireReserveLogApproach
                    && !failedCampfireReserveLogApproach
                    && reach == ScenarioReach.WITHIN_SURVIVAL_REACH
                    && woodenPickaxes > 0
            ) {
                return null;
            }
            if (reach == ScenarioReach.WITHIN_SURVIVAL_REACH && !nearLogsAvailable) {
                return null;
            }
            if (reach == ScenarioReach.OUTSIDE_SURVIVAL_REACH) {
                return new ScenarioBlockTarget(8, 64, 5, "up", "far-natural-log", logItemId);
            }
            if (fourthLogIsReachableDownFace && oakLogs >= 3) {
                return new ScenarioBlockTarget(8, 82, 6, "down", "upper-natural-log", logItemId);
            }
            return new ScenarioBlockTarget(4, 64, 5, "up", "natural-log", logItemId);
        }

        @Override
        public boolean approachBlock(ScenarioBlockTarget target, Duration timeout) {
            operations.add("approach:" + target.blockId() + ":" + target.label());
            nearLogsAvailable = true;
            nearStoneAvailable = true;
            if (failPostFurnaceLogApproach && furnaces > 0 && logItemId.equals(target.blockId())) {
                return false;
            }
            if (failCraftingTableApproachForFurnace && "table-target".equals(target.label()) && cobblestones >= 8) {
                return false;
            }
            if (
                failCraftingTableApproachForCampfire
                    && "table-target".equals(target.label())
                    && charcoals > 0
                    && oakLogs >= 3
            ) {
                return false;
            }
            if (
                failFirstNaturalLogFarApproach
                    && !failedNaturalLogFarApproach
                    && "far-natural-log".equals(target.label())
            ) {
                failedNaturalLogFarApproach = true;
                return false;
            }
            if (
                failFirstNaturalStoneFarApproach
                    && !failedNaturalStoneFarApproach
                    && "far-natural-stone".equals(target.label())
            ) {
                failedNaturalStoneFarApproach = true;
                return false;
            }
            if (
                failFirstCampfireReserveLogApproach
                    && !failedCampfireReserveLogApproach
                    && "far-natural-log".equals(target.label())
                    && woodenPickaxes > 0
            ) {
                failedCampfireReserveLogApproach = true;
                return false;
            }
            if ("upper-natural-log".equals(target.label())) {
                return false;
            }
            return true;
        }

        @Override
        public boolean approachPosition(int x, int z, Duration timeout) {
            operations.add("approachPosition:" + x + ":" + z);
            return x == 72 && z == 8;
        }

        @Override
        public ScenarioBlockTarget findLoadedBlockInColumn(int x, int z, List<String> blockIds) {
            operations.add("findColumn:" + x + ":" + z + ":" + String.join("|", blockIds));
            if (generatedRuinChestPresent && x == 72 && z == 8 && blockIds.contains("minecraft:chest")) {
                return new ScenarioBlockTarget(72, 61, 8, "up", "generated-ruin-cache", "minecraft:chest");
            }
            return null;
        }

        @Override
        public ScenarioBreakResult breakBlockUntilDropVisible(
            ScenarioBlockTarget target,
            String expectedDropItemId,
            Duration timeout
        ) {
            operations.add("breakVisible:" + target.blockId() + ":" + expectedDropItemId);
            return new ScenarioBreakResult(true, true, true, false, new ScenarioHeldItem("minecraft:air", 0));
        }

        @Override
        public ScenarioBreakResult collectVisibleItemDrop(
            ScenarioBlockTarget near,
            String expectedDropItemId,
            int expectedSelectedCount,
            Duration timeout
        ) {
            operations.add("collect:" + near.blockId() + ":" + expectedDropItemId + ":" + expectedSelectedCount);
            if (logItemId.equals(expectedDropItemId)) {
                if (failedLogPickupsRemaining > 0) {
                    failedLogPickupsRemaining -= 1;
                    return new ScenarioBreakResult(
                        true,
                        true,
                        true,
                        false,
                        new ScenarioHeldItem("minecraft:birch_leaves", 5)
                    );
                }
                oakLogs += expectedSelectedCount;
            } else if ("minecraft:cobblestone".equals(expectedDropItemId)) {
                if (failedCobblestonePickupsRemaining > 0) {
                    failedCobblestonePickupsRemaining -= 1;
                    return new ScenarioBreakResult(
                        true,
                        true,
                        true,
                        false,
                        new ScenarioHeldItem("minecraft:wooden_pickaxe", 1)
                    );
                }
                cobblestones += expectedSelectedCount;
            } else if ("minecraft:raw_iron".equals(expectedDropItemId)) {
                if (failedRawIronPickupsRemaining > 0) {
                    failedRawIronPickupsRemaining -= 1;
                    return new ScenarioBreakResult(
                        true,
                        true,
                        true,
                        false,
                        new ScenarioHeldItem("minecraft:stone_pickaxe", 1),
                        "player=(10.0,79.0,7.0) drop=(12.5,77.0,7.5) distance_squared=10.5"
                    );
                }
                rawIron += Math.max(expectedSelectedCount, rawIronPickupBatchSize);
            } else if ("minecraft:torch".equals(expectedDropItemId) && failedTorchPickupsRemaining > 0) {
                failedTorchPickupsRemaining -= 1;
                return new ScenarioBreakResult(
                    true,
                    true,
                    true,
                    false,
                    new ScenarioHeldItem("minecraft:torch", 3)
                );
            } else if ("minecraft:cooked_beef".equals(expectedDropItemId)) {
                cookedBeef += expectedSelectedCount;
            } else if ("minecraft:cooked_porkchop".equals(expectedDropItemId)) {
                cookedPorkchops += expectedSelectedCount;
            } else if ("minecraft:cooked_chicken".equals(expectedDropItemId)) {
                cookedChickens += expectedSelectedCount;
            } else if ("minecraft:wooden_pickaxe".equals(expectedDropItemId)) {
                if (droppedWoodenPickaxes < expectedSelectedCount) {
                    return new ScenarioBreakResult(false, false, true, false, new ScenarioHeldItem("minecraft:air", 0));
                }
                droppedWoodenPickaxes -= expectedSelectedCount;
                woodenPickaxes += expectedSelectedCount;
                return new ScenarioBreakResult(
                    true,
                    woodenPickaxeEntityDisappeared,
                    woodenPickaxePickupObserved,
                    true,
                    new ScenarioHeldItem(expectedDropItemId, 1)
                );
            } else if ("minecraft:wooden_sword".equals(expectedDropItemId)) {
                if (droppedWoodenSwords < expectedSelectedCount) {
                    return new ScenarioBreakResult(false, false, true, false, new ScenarioHeldItem("minecraft:air", 0));
                }
                droppedWoodenSwords -= expectedSelectedCount;
                woodenSwords += expectedSelectedCount;
                return new ScenarioBreakResult(
                    true,
                    true,
                    true,
                    true,
                    new ScenarioHeldItem(expectedDropItemId, 1)
                );
            } else if ("minecraft:wheat_seeds".equals(expectedDropItemId)) {
                wheatSeeds += expectedSelectedCount;
            } else if ("minecraft:wheat".equals(expectedDropItemId)) {
                wheat += expectedSelectedCount;
            }
            return new ScenarioBreakResult(true, true, true, true, new ScenarioHeldItem(expectedDropItemId, 1));
        }

        @Override
        public List<ScenarioItemDropIdentity> visibleItemDropIdentities(String itemId) {
            operations.add("visibleDropIdentities:" + itemId);
            if (
                "minecraft:wooden_pickaxe".equals(itemId)
                    && !deathMaterializedDuringDropBaseline
                    && healthAfterHostileCombat > 0.0F
                    && clientTicks >= dieDuringDropBaselineAfterTick
            ) {
                healthAfterHostileCombat = 0.0F;
                materializeSurvivalDeathDrops();
                deathMaterializedDuringDropBaseline = true;
            }
            List<ScenarioItemDropIdentity> identities = new ArrayList<>();
            if ("minecraft:wooden_pickaxe".equals(itemId) && preexistingWoodenPickaxeEntityId >= 0) {
                identities.add(new ScenarioItemDropIdentity(
                    preexistingWoodenPickaxeEntityId,
                    preexistingWoodenPickaxeEntityUuid
                ));
            }
            if ("minecraft:wooden_sword".equals(itemId) && preexistingWoodenSwordEntityId >= 0) {
                identities.add(new ScenarioItemDropIdentity(
                    preexistingWoodenSwordEntityId,
                    preexistingWoodenSwordEntityUuid
                ));
            }
            if (
                "minecraft:wooden_pickaxe".equals(itemId)
                    && deathMaterializedDuringDropBaseline
                    && droppedWoodenPickaxes > 0
                    && postDeathWoodenPickaxeEntityVisible
            ) {
                identities.add(new ScenarioItemDropIdentity(
                    deathDropWoodenPickaxeEntityId,
                    deathDropWoodenPickaxeEntityUuid
                ));
            }
            if (
                "minecraft:wooden_sword".equals(itemId)
                    && deathMaterializedDuringDropBaseline
                    && droppedWoodenSwords > 0
                    && postDeathWoodenSwordEntityVisible
            ) {
                identities.add(new ScenarioItemDropIdentity(
                    deathDropWoodenSwordEntityId,
                    deathDropWoodenSwordEntityUuid
                ));
            }
            return List.copyOf(identities);
        }

        @Override
        public ScenarioItemDropIdentity waitForNewVisibleItemDropIdentity(
            String itemId,
            List<ScenarioItemDropIdentity> excludedIdentities,
            Duration timeout
        ) {
            operations.add(
                "waitNewDropIdentity:" + itemId + ":excluded=" + excludedIdentities
            );
            if (!postDeathWoodenPickaxeEntityVisible) {
                return null;
            }
            if ("minecraft:wooden_pickaxe".equals(itemId)) {
                ScenarioItemDropIdentity identity = returnPreexistingWoodenPickaxeEntity
                    ? new ScenarioItemDropIdentity(
                        preexistingWoodenPickaxeEntityId,
                        preexistingWoodenPickaxeEntityUuid
                    )
                    : new ScenarioItemDropIdentity(
                        deathDropWoodenPickaxeEntityId,
                        deathDropWoodenPickaxeEntityUuid
                    );
                if (deathMaterializedDuringDropBaseline && excludedIdentities.contains(identity)) {
                    return null;
                }
                return identity;
            }
            if ("minecraft:wooden_sword".equals(itemId)) {
                ScenarioItemDropIdentity identity = new ScenarioItemDropIdentity(
                    deathDropWoodenSwordEntityId,
                    deathDropWoodenSwordEntityUuid
                );
                if (deathMaterializedDuringDropBaseline && excludedIdentities.contains(identity)) {
                    return null;
                }
                return identity;
            }
            return null;
        }

        @Override
        public ScenarioBreakResult collectVisibleItemDropByIdentity(
            ScenarioBlockTarget near,
            String expectedDropItemId,
            ScenarioItemDropIdentity expectedIdentity,
            int expectedSelectedCount,
            Duration timeout
        ) {
            operations.add(
                "collectIdentity:" + expectedDropItemId + ":" + expectedIdentity.entityId()
                    + ":" + expectedIdentity.uuid()
            );
            ScenarioItemDropIdentity deathDropIdentity = switch (expectedDropItemId) {
                case "minecraft:wooden_pickaxe" -> new ScenarioItemDropIdentity(
                    deathDropWoodenPickaxeEntityId,
                    deathDropWoodenPickaxeEntityUuid
                );
                case "minecraft:wooden_sword" -> new ScenarioItemDropIdentity(
                    deathDropWoodenSwordEntityId,
                    deathDropWoodenSwordEntityUuid
                );
                default -> null;
            };
            if (deathDropIdentity == null || !deathDropIdentity.equals(expectedIdentity)) {
                return new ScenarioBreakResult(true, true, false, false, new ScenarioHeldItem("minecraft:air", 0));
            }
            if ("minecraft:wooden_pickaxe".equals(expectedDropItemId) && woodenPickaxeIdentityLostBeforePickup) {
                if (unrelatedWoodenPickaxePickedUpAfterIdentityLoss) {
                    woodenPickaxes += expectedSelectedCount;
                }
                return new ScenarioBreakResult(
                    true,
                    true,
                    true,
                    false,
                    new ScenarioHeldItem("minecraft:air", 0)
                );
            }
            return collectVisibleItemDrop(near, expectedDropItemId, expectedSelectedCount, timeout);
        }

        @Override
        public int recipeDisplayIdForResult(String itemId) {
            operations.add("recipeForResult:" + itemId);
            if ("minecraft:wooden_sword".equals(itemId)) {
                return 32;
            }
            throw new IllegalArgumentException("unsupported test recipe result " + itemId);
        }

        @Override
        public void placeRecipe(int containerId, int recipeDisplayId, boolean useMaxItems) {
            operations.add("recipe:" + containerId + ":" + recipeDisplayId + ":" + useMaxItems);
            if (recipeDisplayId == planksRecipeDisplayId()) {
                int logsToCraft = useMaxItems ? oakLogs : Math.min(oakLogs, 1);
                oakLogs -= logsToCraft;
                oakPlanks += logsToCraft * 4;
            } else if (recipeDisplayId == 10) {
                if (oakPlanks >= 4) {
                    oakPlanks -= 4;
                    craftingTables += 1;
                }
            } else if (recipeDisplayId == 21) {
                if (oakPlanks >= 2) {
                    oakPlanks -= 2;
                    sticks += 4;
                }
            } else if (recipeDisplayId == 31) {
                if (oakPlanks >= 3 && sticks >= 2) {
                    oakPlanks -= 3;
                    sticks -= 2;
                    woodenPickaxes += 1;
                }
            } else if (recipeDisplayId == 32) {
                if (oakPlanks >= 2 && sticks >= 1) {
                    oakPlanks -= 2;
                    sticks -= 1;
                    woodenSwords += 1;
                }
            } else if (recipeDisplayId == 30) {
                if (oakPlanks >= 2 && sticks >= 2) {
                    oakPlanks -= 2;
                    sticks -= 2;
                    woodenHoes += 1;
                }
            } else if (recipeDisplayId == 24) {
                if (cobblestones >= 3 && sticks >= 2) {
                    cobblestones -= 3;
                    sticks -= 2;
                    stonePickaxes += 1;
                }
            } else if (recipeDisplayId == 26) {
                if (cobblestones >= 2 && sticks >= 1) {
                    cobblestones -= 2;
                    sticks -= 1;
                    stoneSwords += 1;
                }
            } else if (recipeDisplayId == 13) {
                if (cobblestones >= 8) {
                    cobblestones -= 8;
                    furnaces += 1;
                }
            } else if (recipeDisplayId == 57) {
                if (ironIngots >= 2 && sticks >= 1) {
                    ironIngots -= 2;
                    sticks -= 1;
                    ironSwords += 1;
                }
            } else if (recipeDisplayId == 58) {
                if (oakPlanks >= 6 && ironIngots >= 1) {
                    oakPlanks -= 6;
                    ironIngots -= 1;
                    shields += 1;
                }
            } else if (recipeDisplayId == 59) {
                if (ironIngots >= 8) {
                    ironIngots -= 8;
                    ironChestplates += 1;
                }
            } else if (recipeDisplayId == 5) {
                if (oakPlanks >= 8) {
                    oakPlanks -= 8;
                    chests += 1;
                }
            } else if (recipeDisplayId == 27) {
                if (charcoals >= 1 && sticks >= 1) {
                    charcoals -= 1;
                    sticks -= 1;
                    torches += 4;
                }
            } else if (recipeDisplayId == sheepBedRecipeDisplayId()) {
                if (oakPlanks >= 3 && sheepWool >= 3) {
                    oakPlanks -= 3;
                    sheepWool -= 3;
                    sheepBeds += 1;
                }
            } else if (recipeDisplayId == doorRecipeDisplayId()) {
                if (oakPlanks >= 6) {
                    oakPlanks -= 6;
                    doors += 3;
                }
            } else if (recipeDisplayId == signRecipeDisplayId()) {
                if (oakPlanks >= 6 && sticks >= 1) {
                    oakPlanks -= 6;
                    sticks -= 1;
                    signs += 3;
                }
            } else if (recipeDisplayId == 53) {
                if (oakLogs >= 3 && sticks >= 3 && charcoals >= 1) {
                    oakLogs -= 3;
                    sticks -= 3;
                    charcoals -= 1;
                    campfires += 1;
                }
            } else if (recipeDisplayId == 60) {
                if (wheat >= 3) {
                    wheat -= 3;
                    breads += 1;
                }
            }
        }

        @Override
        public boolean waitForInventoryCount(String itemId, int count, Duration duration) {
            operations.add("waitCount:" + itemId + ":" + count);
            return inventoryCountWithoutRecording(itemId) == count;
        }

        @Override
        public int totalExperience() {
            operations.add("totalExperience");
            return totalExperience;
        }

        @Override
        public int waitForTotalExperienceAbove(int experience, Duration duration) {
            operations.add("waitExperienceAbove:" + experience);
            return totalExperience;
        }

        private int inventoryCountWithoutRecording(String itemId) {
            if (logItemId.equals(itemId)) {
                return oakLogs;
            }
            if (planksItemId.equals(itemId)) {
                return oakPlanks;
            }
            if ("minecraft:stick".equals(itemId)) {
                return sticks;
            }
            if ("minecraft:crafting_table".equals(itemId)) {
                return craftingTables;
            }
            if ("minecraft:wooden_pickaxe".equals(itemId)) {
                return woodenPickaxes;
            }
            if ("minecraft:wooden_sword".equals(itemId)) {
                return woodenSwords;
            }
            if ("minecraft:wooden_hoe".equals(itemId)) {
                return woodenHoes;
            }
            if ("minecraft:cobblestone".equals(itemId)) {
                return cobblestones;
            }
            if ("minecraft:cobblestone_slab".equals(itemId)) {
                return cobblestoneSlabs;
            }
            if ("minecraft:stone_pickaxe".equals(itemId)) {
                return stonePickaxes;
            }
            if ("minecraft:stone_sword".equals(itemId)) {
                return stoneSwords;
            }
            if ("minecraft:furnace".equals(itemId)) {
                return furnaces;
            }
            if ("minecraft:raw_iron".equals(itemId)) {
                return rawIron;
            }
            if ("minecraft:iron_ingot".equals(itemId)) {
                return ironIngots;
            }
            if ("minecraft:iron_sword".equals(itemId)) {
                return ironSwords;
            }
            if ("minecraft:shield".equals(itemId)) {
                return shields;
            }
            if ("minecraft:iron_chestplate".equals(itemId)) {
                return ironChestplates;
            }
            if ("minecraft:chest".equals(itemId)) {
                return chests;
            }
            if ("minecraft:charcoal".equals(itemId)) {
                return charcoals;
            }
            if ("minecraft:torch".equals(itemId)) {
                return torches;
            }
            if ("minecraft:beef".equals(itemId)) {
                return beef;
            }
            if ("minecraft:porkchop".equals(itemId)) {
                return porkchops;
            }
            if ("minecraft:chicken".equals(itemId)) {
                return chickens;
            }
            if ("minecraft:cooked_beef".equals(itemId)) {
                return cookedBeef;
            }
            if ("minecraft:cooked_porkchop".equals(itemId)) {
                return cookedPorkchops;
            }
            if ("minecraft:cooked_chicken".equals(itemId)) {
                return cookedChickens;
            }
            if (sheepWoolItemId.equals(itemId)) {
                return sheepWool;
            }
            if (sheepBedItemId().equals(itemId)) {
                return sheepBeds;
            }
            if ("minecraft:rotten_flesh".equals(itemId)) {
                return rottenFlesh;
            }
            if (doorItemId().equals(itemId)) {
                return doors;
            }
            if (signItemId().equals(itemId)) {
                return signs;
            }
            if ("minecraft:campfire".equals(itemId)) {
                return campfires;
            }
            if ("minecraft:wheat_seeds".equals(itemId)) {
                return wheatSeeds;
            }
            if ("minecraft:wheat".equals(itemId)) {
                return wheat;
            }
            if ("minecraft:bread".equals(itemId)) {
                return breads;
            }
            if ("minecraft:diamond".equals(itemId)) {
                return diamonds;
            }
            if ("minecraft:lapis_lazuli".equals(itemId)) {
                return lapisLazuli;
            }
            return 0;
        }

        private int planksRecipeDisplayId() {
            return switch (planksItemId) {
                case "minecraft:acacia_planks" -> 0;
                case "minecraft:birch_planks" -> 2;
                case "minecraft:cherry_planks" -> 5;
                case "minecraft:dark_oak_planks" -> 12;
                case "minecraft:jungle_planks" -> 16;
                case "minecraft:mangrove_planks" -> 17;
                case "minecraft:oak_planks" -> 18;
                case "minecraft:pale_oak_planks" -> 19;
                case "minecraft:spruce_planks" -> 20;
                default -> throw new IllegalStateException("unsupported test planks item " + planksItemId);
            };
        }

        private String doorItemId() {
            return switch (planksItemId) {
                case "minecraft:acacia_planks" -> "minecraft:acacia_door";
                case "minecraft:birch_planks" -> "minecraft:birch_door";
                case "minecraft:cherry_planks" -> "minecraft:cherry_door";
                case "minecraft:dark_oak_planks" -> "minecraft:dark_oak_door";
                case "minecraft:jungle_planks" -> "minecraft:jungle_door";
                case "minecraft:mangrove_planks" -> "minecraft:mangrove_door";
                case "minecraft:oak_planks" -> "minecraft:oak_door";
                case "minecraft:pale_oak_planks" -> "minecraft:pale_oak_door";
                case "minecraft:spruce_planks" -> "minecraft:spruce_door";
                default -> throw new IllegalStateException("unsupported test planks item " + planksItemId);
            };
        }

        private int doorRecipeDisplayId() {
            return switch (planksItemId) {
                case "minecraft:acacia_planks" -> 35;
                case "minecraft:birch_planks" -> 36;
                case "minecraft:cherry_planks" -> 37;
                case "minecraft:dark_oak_planks" -> 38;
                case "minecraft:jungle_planks" -> 39;
                case "minecraft:mangrove_planks" -> 40;
                case "minecraft:oak_planks" -> 41;
                case "minecraft:pale_oak_planks" -> 42;
                case "minecraft:spruce_planks" -> 43;
                default -> throw new IllegalStateException("unsupported test planks item " + planksItemId);
            };
        }

        private String signItemId() {
            return switch (planksItemId) {
                case "minecraft:acacia_planks" -> "minecraft:acacia_sign";
                case "minecraft:birch_planks" -> "minecraft:birch_sign";
                case "minecraft:cherry_planks" -> "minecraft:cherry_sign";
                case "minecraft:dark_oak_planks" -> "minecraft:dark_oak_sign";
                case "minecraft:jungle_planks" -> "minecraft:jungle_sign";
                case "minecraft:mangrove_planks" -> "minecraft:mangrove_sign";
                case "minecraft:oak_planks" -> "minecraft:oak_sign";
                case "minecraft:pale_oak_planks" -> "minecraft:pale_oak_sign";
                case "minecraft:spruce_planks" -> "minecraft:spruce_sign";
                default -> throw new IllegalStateException("unsupported test planks item " + planksItemId);
            };
        }

        private int signRecipeDisplayId() {
            return switch (planksItemId) {
                case "minecraft:acacia_planks" -> 44;
                case "minecraft:birch_planks" -> 45;
                case "minecraft:cherry_planks" -> 46;
                case "minecraft:dark_oak_planks" -> 47;
                case "minecraft:jungle_planks" -> 48;
                case "minecraft:mangrove_planks" -> 49;
                case "minecraft:oak_planks" -> 50;
                case "minecraft:pale_oak_planks" -> 51;
                case "minecraft:spruce_planks" -> 52;
                default -> throw new IllegalStateException("unsupported test planks item " + planksItemId);
            };
        }

        private String sheepBedItemId() {
            return sheepWoolItemId.replace("_wool", "_bed");
        }

        private int sheepBedRecipeDisplayId() {
            return "minecraft:white_wool".equals(sheepWoolItemId) ? 34 : 61;
        }

        @Override
        public ScenarioHeldItem selectHotbarItem(String itemId, int count, Duration timeout) {
            operations.add("selectHotbar:" + itemId + ":" + count);
            return new ScenarioHeldItem(itemId, count);
        }

        @Override
        public ScenarioBlockTarget dropSelectedItem(String itemId, int count, Duration timeout) {
            operations.add("dropSelected:" + itemId + ":" + count);
            decrementItem(itemId, count);
            return new ScenarioBlockTarget(
                4,
                64,
                5,
                "up",
                "playable-two-client-inventory-drop-marker",
                itemId
            );
        }

        @Override
        public ScenarioEntityObservation findVisibleEntity(
            List<String> entityTypeIds,
            ScenarioReach reach,
            Duration timeout
        ) {
            operations.add("findEntity:" + String.join("|", entityTypeIds) + ":" + reach.label());
            if (reach == ScenarioReach.OUTSIDE_SURVIVAL_REACH && entityTypeIds.contains("minecraft:sheep")) {
                return new ScenarioEntityObservation(
                    "minecraft:sheep",
                    43 + sheepWool,
                    entityUuid(43 + sheepWool),
                    9.5,
                    64.0,
                    8.5,
                    81.0,
                    firstObservedSheepWoolItemId
                );
            }
            if (reach == ScenarioReach.OUTSIDE_SURVIVAL_REACH && entityTypeIds.contains("minecraft:cow")) {
                return new ScenarioEntityObservation(
                    "minecraft:cow", 42, entityUuid(42), 8.5, 64.0, 8.5, 64.0, null
                );
            }
            if (reach == ScenarioReach.OUTSIDE_SURVIVAL_REACH && entityTypeIds.contains("minecraft:chicken")) {
                return new ScenarioEntityObservation(
                    "minecraft:chicken", 44, entityUuid(44), 10.5, 64.0, 8.5, 100.0, null
                );
            }
            if (reach == ScenarioReach.OUTSIDE_SURVIVAL_REACH && entityTypeIds.contains("minecraft:zombie")) {
                return new ScenarioEntityObservation(
                    "minecraft:zombie", 99, entityUuid(99), 12.5, 64.0, 8.5, 80.0, null
                );
            }
            return null;
        }

        @Override
        public ScenarioEntityObservation findVisibleSheepWithWool(
            String woolItemId,
            ScenarioReach reach,
            Duration timeout
        ) {
            operations.add("findSheepWool:" + woolItemId + ":" + reach.label());
            if (reach != ScenarioReach.OUTSIDE_SURVIVAL_REACH || !sheepWoolItemId.equals(woolItemId)) {
                return null;
            }
            return new ScenarioEntityObservation(
                "minecraft:sheep",
                100 + sheepWool,
                entityUuid(100 + sheepWool),
                9.5,
                64.0,
                8.5,
                81.0,
                sheepWoolItemId
            );
        }

        @Override
        public ScenarioEntityObservation visibleEntity(List<String> entityTypeIds, ScenarioReach reach) {
            operations.add("visibleEntity:" + String.join("|", entityTypeIds) + ":" + reach.label());
            if (visiblePassiveDuringSoak && entityTypeIds.contains("minecraft:cow")) {
                visiblePassiveDuringSoak = false;
                return new ScenarioEntityObservation(
                    "minecraft:cow",
                    198,
                    entityUuid(198),
                    3.5,
                    64.0,
                    3.5,
                    4.0,
                    null
                );
            }
            boolean shouldReturnHostile = visibleHostileDuringSoak && reach == ScenarioReach.WITHIN_SURVIVAL_REACH;
            shouldReturnHostile |= visibleHostileOutsideReachDuringSoak
                && reach == ScenarioReach.OUTSIDE_SURVIVAL_REACH;
            if (shouldReturnHostile) {
                visibleHostileDuringSoak = false;
                visibleHostileOutsideReachDuringSoak = false;
                if (!entityTypeIds.contains(visibleHostileTypeDuringSoak)) {
                    return null;
                }
                return new ScenarioEntityObservation(
                    visibleHostileTypeDuringSoak,
                    199,
                    entityUuid(199),
                    4.5,
                    64.0,
                    4.5,
                    9.0,
                    null
                );
            }
            return null;
        }

        private static UUID entityUuid(int entityId) {
            return new UUID(0L, Integer.toUnsignedLong(entityId));
        }

        @Override
        public ScenarioEntityMotionObservation waitForEntityMotion(
            ScenarioEntityObservation entity,
            double minimumHorizontalDistance,
            double minimumVerticalRise,
            Duration timeout
        ) {
            operations.add(
                "motion:" + entity.entityType() + ":" + minimumHorizontalDistance + ":" + minimumVerticalRise
            );
            double speed = switch (entity.entityType()) {
                case "minecraft:cow" -> 0.10;
                case "minecraft:sheep" -> 0.115;
                case "minecraft:chicken" -> 0.125;
                default -> 0.0;
            };
            double verticalRise = "minecraft:cow".equals(entity.entityType()) ? 1.1 : 0.0;
            return new ScenarioEntityMotionObservation(
                entity.entityType(),
                entity.entityId(),
                entity.x() + 1.25,
                entity.y() + verticalRise,
                entity.z(),
                1.25,
                verticalRise,
                speed,
                1.5
            );
        }

        @Override
        public ScenarioPlayerObservation waitForVisiblePlayer(String playerName, Duration timeout) {
            operations.add("waitPlayer:" + playerName);
            if ("SolarisPrimary".equals(playerName)) {
                return new ScenarioPlayerObservation(
                    playerName,
                    visiblePlayerEntityId,
                    10.0,
                    64.0,
                    8.0,
                    16.0
                );
            }
            return null;
        }

        @Override
        public boolean waitForNoVisiblePlayer(String playerName, Duration timeout) {
            operations.add("waitNoPlayer:" + playerName);
            return "SolarisPrimary".equals(playerName);
        }

        @Override
        public ScenarioPlayerObservation waitForMovedPlayer(
            String playerName,
            ScenarioPlayerObservation baseline,
            double minHorizontalDelta,
            Duration timeout
        ) {
            operations.add("waitPlayerMoved:" + playerName + ":" + minHorizontalDelta);
            if ("SolarisPrimary".equals(playerName)) {
                return new ScenarioPlayerObservation(
                    playerName,
                    baseline.entityId(),
                    baseline.x() + minHorizontalDelta + 0.25,
                    baseline.y(),
                    baseline.z(),
                    baseline.distanceSquared()
                );
            }
            return null;
        }

        @Override
        public boolean approachEntity(ScenarioEntityObservation entity, Duration timeout) {
            String kind = "minecraft:zombie".equals(entity.entityType()) ? "loaded-hostile" : "loaded-passive";
            operations.add("approachEntity:" + entity.entityType() + ":" + kind);
            return true;
        }

        @Override
        public ScenarioBreakResult attackEntityUntilDropCollected(
            ScenarioEntityObservation entity,
            String expectedDropItemId,
            int expectedSelectedCount,
            Duration timeout
        ) {
            operations.add(
                "attackEntityDrop:"
                    + entity.entityType()
                    + ":entity_id="
                    + entity.entityId()
                    + ":"
                    + expectedDropItemId
                    + ":"
                    + expectedSelectedCount
            );
            if ("minecraft:beef".equals(expectedDropItemId)) {
                beef += expectedSelectedCount;
            } else if ("minecraft:porkchop".equals(expectedDropItemId)) {
                porkchops += expectedSelectedCount;
            } else if ("minecraft:chicken".equals(expectedDropItemId)) {
                chickens += expectedSelectedCount;
            } else if (sheepWoolItemId.equals(expectedDropItemId)) {
                sheepWool += expectedSelectedCount;
            } else if ("minecraft:sheep".equals(entity.entityType())) {
                return new ScenarioBreakResult(false, false, false, false, new ScenarioHeldItem("minecraft:air", 0));
            } else if ("minecraft:rotten_flesh".equals(expectedDropItemId)) {
                rottenFlesh += expectedSelectedCount;
            }
            return new ScenarioBreakResult(
                true,
                true,
                true,
                true,
                new ScenarioHeldItem(expectedDropItemId, expectedSelectedCount)
            );
        }

        @Override
        public boolean attackEntityUntilRemoved(ScenarioEntityObservation entity, Duration timeout) {
            operations.add("attackEntityUntilRemoved:" + entity.entityType() + ":" + entity.entityId());
            if (dieOnNextHostileAttack) {
                dieOnNextHostileAttack = false;
                healthAfterHostileCombat = 0.0F;
                materializeSurvivalDeathDrops();
                return false;
            }
            return true;
        }

        @Override
        public float playerHealth() {
            operations.add("playerHealth");
            return healthAfterHostileCombat;
        }

        @Override
        public float waitForPlayerHealthBelow(double health, Duration duration) {
            operations.add("waitHealthBelow:" + health);
            if (healthAfterHostileCombat >= health) {
                float damage = equippedIronChestplate ? 2.46F : 3.0F;
                healthAfterHostileCombat = Math.max(0.0F, healthAfterHostileCombat - damage);
            }
            return healthAfterHostileCombat;
        }

        @Override
        public ScenarioShieldBlockResult blockAttackWithSelectedShield(String itemId, Duration timeout) {
            operations.add("blockAttackWithSelectedShield:" + itemId);
            boolean started = "minecraft:shield".equals(itemId) && shields > 0;
            int damageAfter = started && shieldBlockedAttackObserved ? 4 : 0;
            return new ScenarioShieldBlockResult(
                started,
                started && shieldBlockedAttackObserved,
                healthAfterHostileCombat,
                healthAfterHostileCombat,
                0,
                damageAfter
            );
        }

        @Override
        public boolean quickEquipSelectedArmor(String itemId, String armorSlot, Duration duration) {
            operations.add("quickEquip:" + itemId + ":" + armorSlot);
            if ("minecraft:iron_chestplate".equals(itemId) && "chest".equals(armorSlot) && ironChestplates > 0) {
                ironChestplates -= 1;
                equippedIronChestplate = true;
                return true;
            }
            return "minecraft:iron_chestplate".equals(itemId)
                && "chest".equals(armorSlot)
                && equippedIronChestplate;
        }

        @Override
        public ScenarioHeldItem equippedArmor(String armorSlot) {
            operations.add("equippedArmor:" + armorSlot);
            if ("chest".equals(armorSlot) && equippedIronChestplate) {
                return new ScenarioHeldItem("minecraft:iron_chestplate", 1);
            }
            return new ScenarioHeldItem("minecraft:air", 0);
        }

        @Override
        public boolean drainHungerBySprinting(Duration timeout) {
            operations.add("drainHungerBySprinting");
            foodLevel = 19;
            return true;
        }

        @Override
        public ScenarioFoodUseResult eatSelectedFood(String itemId, int itemCountBefore, Duration timeout) {
            operations.add("eatSelectedFood:" + itemId + ":" + itemCountBefore);
            int beforeFood = foodLevel;
            int beforeCount = inventoryCountWithoutRecording(itemId);
            if (inventoryCountWithoutRecording(itemId) > 0 && foodLevel < 20) {
                decrementItem(itemId, 1);
                foodLevel = 20;
            }
            return new ScenarioFoodUseResult(
                true,
                beforeFood,
                foodLevel,
                beforeCount,
                inventoryCountWithoutRecording(itemId)
            );
        }

        @Override
        public ScenarioBlockPair findDryPlaceablePair(ScenarioReach reach) {
            operations.add("findDry:" + reach.label());
            if (woodenHoes > 0) {
                int plot = farmPlots++;
                return new ScenarioBlockPair(
                    new ScenarioBlockTarget(
                        2 + plot,
                        64,
                        2,
                        "up",
                        "farm-" + plot + "-clicked",
                        "minecraft:grass_block"
                    ),
                    new ScenarioBlockTarget(
                        2 + plot,
                        65,
                        2,
                        "down",
                        "farm-" + plot + "-target",
                        "minecraft:air"
                    )
                );
            }
            return new ScenarioBlockPair(
                new ScenarioBlockTarget(1, 64, 1, "up", nextPlacementLabel() + "-clicked", "minecraft:dirt"),
                new ScenarioBlockTarget(1, 65, 1, "down", nextPlacementLabel() + "-target", "minecraft:air")
            );
        }

        @Override
        public ScenarioBlockPair findUnobstructedPlaceablePair(ScenarioReach reach) {
            return findDryPlaceablePair(reach);
        }

        @Override
        public ScenarioBlockPair findTillableSoil(ScenarioReach reach) {
            operations.add("findTillable:" + reach.label());
            int plot = farmPlots++;
            return new ScenarioBlockPair(
                new ScenarioBlockTarget(
                    2 + plot,
                    64,
                    2,
                    "up",
                    "farm-" + plot + "-clicked",
                    "minecraft:grass_block"
                ),
                new ScenarioBlockTarget(
                    2 + plot,
                    65,
                    2,
                    "down",
                    "farm-" + plot + "-target",
                    "minecraft:air"
                )
            );
        }

        private String nextPlacementLabel() {
            if (campfires > 0) {
                return "campfire";
            }
            if (signs > 0) {
                return "sign";
            }
            if (sheepBeds > 0) {
                return "bed";
            }
            if (doors > 0) {
                return "door";
            }
            if (furnaces > 0 && torches > 0) {
                return "torch";
            }
            if (chests > 0) {
                return "chest";
            }
            if (furnaces > 0) {
                return "furnace";
            }
            return "table";
        }

        @Override
        public ScenarioUseResult useItemOn(ScenarioBlockTarget clicked, ScenarioHeldItem heldItem) {
            operations.add("use:" + heldItem.itemId() + ":" + clicked.label());
            if ("minecraft:stonecutter".equals(heldItem.itemId()) && !stonecutterPlaced) {
                stonecutterPlaced = true;
            }
            if ("minecraft:stonecutter".equals(clicked.blockId()) && stonecutterPlaced) {
                stonecutterMenuOpen = true;
                stonecutterScreenClassName =
                    "net.minecraft.client.gui.screens.inventory.StonecutterScreen";
                if (reuseStonecutterContainerIdOnReopen && nextStonecutterContainerId > 8) {
                    activeContainerId = 8;
                } else {
                    activeContainerId = nextStonecutterContainerId++;
                }
            }
            if ("minecraft:wheat_seeds".equals(heldItem.itemId()) && wheatSeeds > 0) {
                wheatSeeds -= 1;
            }
            if (
                "minecraft:crafting_table".equals(heldItem.itemId())
                    && "table-clicked".equals(clicked.label())
            ) {
                return new ScenarioUseResult(craftingTablePlaceUseResult);
            }
            return new ScenarioUseResult("success");
        }

        @Override
        public boolean waitForBlock(ScenarioBlockTarget target, String blockId, Duration duration) {
            operations.add("waitBlock:" + target.label() + ":" + blockId);
            if (
                "table-target".equals(target.label())
                    && "minecraft:crafting_table".equals(blockId)
                    && !craftingTablePlacementObserved
            ) {
                return false;
            }
            return true;
        }

        @Override
        public boolean waitForBlockProperty(
            ScenarioBlockTarget target,
            String property,
            String value,
            Duration duration
        ) {
            operations.add("waitProperty:" + target.label() + ":" + property + ":" + value);
            return true;
        }

        @Override
        public ScenarioLightLevel lightLevel(ScenarioBlockTarget target) {
            operations.add("light:" + target.label());
            return new ScenarioLightLevel(cropSkyLight, 0);
        }

        @Override
        public boolean waitForVisibleItemDrop(String itemId, ScenarioBlockTarget near, Duration timeout) {
            operations.add("waitDrop:" + itemId + ":" + near.label());
            return true;
        }

        @Override
        public boolean waitForNoVisibleItemDrop(String itemId, ScenarioBlockTarget near, Duration timeout) {
            operations.add("waitNoDrop:" + itemId + ":" + near.label());
            return true;
        }

        private void materializeSurvivalDeathDrops() {
            droppedWoodenPickaxes += woodenPickaxes;
            droppedWoodenSwords += woodenSwords;
            woodenPickaxes = 0;
            woodenSwords = 0;
        }

        @Override
        public boolean waitForDeathScreen(Duration duration) {
            operations.add("waitDeathScreen");
            if (healthAfterHostileCombat > 0.0F) {
                return false;
            }
            if (droppedWoodenPickaxes == 0 && droppedWoodenSwords == 0) {
                materializeSurvivalDeathDrops();
            }
            return true;
        }

        @Override
        public boolean standOnBlockUntilDeath(ScenarioBlockTarget target, Duration duration) {
            operations.add("standOnBlockUntilDeath:" + target.label() + ":" + target.blockId());
            healthAfterHostileCombat = 0.0F;
            materializeSurvivalDeathDrops();
            return true;
        }

        @Override
        public boolean performRespawn(Duration duration) {
            operations.add("respawn");
            healthAfterHostileCombat = 20.0F;
            return true;
        }

        @Override
        public boolean waitForSignEditor(ScenarioBlockTarget target, Duration duration) {
            operations.add("signEditor:" + target.label());
            return true;
        }

        @Override
        public void updateSignText(ScenarioBlockTarget target, List<String> lines) {
            operations.add("signText:" + target.label() + ":" + String.join("|", lines));
        }

        @Override
        public boolean waitForSignText(ScenarioBlockTarget target, List<String> lines, Duration duration) {
            operations.add("waitSignText:" + target.label() + ":" + String.join("|", lines));
            return true;
        }

        @Override
        public boolean waitForScreenClassName(String className, Duration duration) {
            operations.add("screen:" + className);
            if ("net.minecraft.client.gui.screens.inventory.StonecutterScreen".equals(className)) {
                return className.equals(stonecutterScreenClassName);
            }
            return true;
        }

        @Override
        public boolean closeCurrentScreen(Duration duration) {
            operations.add("closeScreen");
            boolean closed = !failCloseCurrentScreen
                || operations.stream().noneMatch(operation -> operation.startsWith("moveToContainer:0:"));
            if (closed && stonecutterMenuOpen) {
                if ("minecraft:cobblestone".equals(containerItemIds[0])) {
                    cobblestones += containerCounts[0];
                }
                containerItemIds[0] = null;
                containerCounts[0] = 0;
                containerItemIds[1] = null;
                containerCounts[1] = 0;
                stonecutterOfferSelected = false;
                stonecutterMenuOpen = false;
                stonecutterScreenClassName = null;
                activeContainerId = 7;
            }
            return closed;
        }

        @Override
        public int activeContainerId() {
            operations.add("containerId");
            return activeContainerId;
        }

        @Override
        public boolean moveSelectedItemToContainerSlot(
            int containerSlot,
            String itemId,
            int count,
            Duration duration
        ) {
            operations.add("moveToContainer:" + containerSlot + ":" + itemId + ":" + count);
            if (containerSlot < 0 || containerSlot >= containerItemIds.length) {
                return false;
            }
            int movedCount = count;
            if (logItemId.equals(itemId)) {
                if (oakLogs < count) {
                    return false;
                }
                if (containerSlot == 0) {
                    movedCount = oakLogs;
                }
                oakLogs -= movedCount;
            } else if (planksItemId.equals(itemId)) {
                if (oakPlanks < count) {
                    return false;
                }
                if (containerSlot == 1) {
                    movedCount = oakPlanks;
                }
                oakPlanks -= movedCount;
            } else if ("minecraft:wooden_pickaxe".equals(itemId)) {
                if (woodenPickaxes < count) {
                    return false;
                }
                woodenPickaxes -= count;
            } else if ("minecraft:beef".equals(itemId)) {
                if (beef < count) {
                    return false;
                }
                beef -= count;
            } else if ("minecraft:porkchop".equals(itemId)) {
                if (porkchops < count) {
                    return false;
                }
                porkchops -= count;
            } else if ("minecraft:chicken".equals(itemId)) {
                if (chickens < count) {
                    return false;
                }
                chickens -= count;
            } else if ("minecraft:charcoal".equals(itemId)) {
                if (charcoals < count) {
                    return false;
                }
                charcoals -= count;
            } else if ("minecraft:raw_iron".equals(itemId)) {
                if (rawIron < count) {
                    return false;
                }
                if (containerSlot == 0) {
                    movedCount = rawIron;
                }
                rawIron -= movedCount;
            } else if ("minecraft:cobblestone".equals(itemId)) {
                if (cobblestones < count || containerSlot != 0) {
                    return false;
                }
                movedCount = cobblestones;
                cobblestones = 0;
            } else {
                return false;
            }
            containerItemIds[containerSlot] = itemId;
            containerCounts[containerSlot] = movedCount;
            return true;
        }

        @Override
        public boolean waitForContainerSlot(
            int containerSlot,
            String itemId,
            int count,
            Duration duration
        ) {
            operations.add("waitContainer:" + containerSlot + ":" + itemId + ":" + count);
            if (
                containerSlot == 2
                    && "minecraft:charcoal".equals(itemId)
                    && logItemId.equals(containerItemIds[0])
                    && planksItemId.equals(containerItemIds[1])
            ) {
                containerCounts[0] = Math.max(0, containerCounts[0] - 1);
                if (containerCounts[0] == 0) {
                    containerItemIds[0] = null;
                }
                containerCounts[1] = Math.max(0, containerCounts[1] - 1);
                if (containerCounts[1] == 0) {
                    containerItemIds[1] = null;
                }
                containerItemIds[2] = "minecraft:charcoal";
                containerCounts[2] = 1;
            }
            if (
                containerSlot == 2
                    && !"minecraft:raw_iron".equals(containerItemIds[0])
                    && cookedItemFor(containerItemIds[0]).equals(itemId)
            ) {
                containerItemIds[0] = null;
                containerCounts[0] = 0;
                containerCounts[1] = Math.max(0, containerCounts[1] - 1);
                if (containerCounts[1] == 0) {
                    containerItemIds[1] = null;
                }
                containerItemIds[2] = itemId;
                containerCounts[2] = 1;
            }
            if (containerSlot == 2 && "minecraft:iron_ingot".equals(itemId) && "minecraft:raw_iron".equals(containerItemIds[0])) {
                int smeltedCount = Math.min(containerCounts[0], count);
                containerCounts[0] = Math.max(0, containerCounts[0] - smeltedCount);
                if (containerCounts[0] == 0) {
                    containerItemIds[0] = null;
                }
                containerCounts[1] = Math.max(0, containerCounts[1] - smeltedCount);
                if (containerCounts[1] == 0) {
                    containerItemIds[1] = null;
                }
                containerItemIds[2] = "minecraft:iron_ingot";
                containerCounts[2] = smeltedCount;
            }
            return itemId.equals(containerItemIds[containerSlot]) && containerCounts[containerSlot] >= count;
        }

        @Override
        public boolean moveContainerSlotToInventory(
            int containerSlot,
            String itemId,
            int count,
            Duration duration
        ) {
            operations.add("moveFromContainer:" + containerSlot + ":" + itemId + ":" + count);
            if (
                containerSlot < 0
                    || containerSlot >= containerItemIds.length
                    || !itemId.equals(containerItemIds[containerSlot])
                    || containerCounts[containerSlot] < count
            ) {
                return false;
            }
            if (
                stonecutterMenuOpen
                    && stonecutterOfferSelected
                    && containerSlot == 1
                    && "minecraft:cobblestone_slab".equals(itemId)
                    && "minecraft:cobblestone".equals(containerItemIds[0])
                    && containerCounts[0] > 0
            ) {
                containerCounts[0] -= 1;
                if (containerCounts[0] == 0) {
                    containerItemIds[0] = null;
                    containerItemIds[1] = null;
                    containerCounts[1] = 0;
                    stonecutterOfferSelected = false;
                }
                cobblestoneSlabs += 2;
                return true;
            }
            int movedCount = containerCounts[containerSlot];
            containerCounts[containerSlot] = 0;
            containerItemIds[containerSlot] = null;
            if ("minecraft:charcoal".equals(itemId)) {
                charcoals += movedCount;
            } else if (logItemId.equals(itemId)) {
                oakLogs += movedCount;
            } else if (planksItemId.equals(itemId)) {
                oakPlanks += movedCount;
            } else if ("minecraft:cooked_beef".equals(itemId)) {
                cookedBeef += movedCount;
            } else if ("minecraft:cooked_porkchop".equals(itemId)) {
                cookedPorkchops += movedCount;
            } else if ("minecraft:cooked_chicken".equals(itemId)) {
                cookedChickens += movedCount;
            } else if ("minecraft:iron_ingot".equals(itemId)) {
                ironIngots += movedCount;
                totalExperience += movedCount * 7 / 10;
            } else if ("minecraft:raw_iron".equals(itemId)) {
                rawIron += movedCount;
            }
            return true;
        }

        @Override
        public boolean waitForContainerSlotEmpty(int containerSlot, Duration duration) {
            operations.add("waitContainerEmpty:" + containerSlot);
            return containerSlot >= 0
                && containerSlot < containerItemIds.length
                && (containerItemIds[containerSlot] == null || containerCounts[containerSlot] == 0);
        }

        @Override
        public int findContainerSlot(String itemId, int count) {
            operations.add("findContainerSlot:" + itemId + ":" + count);
            for (int slot = 0; slot < containerItemIds.length; slot++) {
                if (itemId.equals(containerItemIds[slot]) && containerCounts[slot] == count) {
                    return slot;
                }
            }
            return -1;
        }

        @Override
        public boolean quickMoveContainerSlot(int containerSlot, Duration duration) {
            operations.add("quickMoveContainer:" + containerSlot);
            if (containerSlot < 0 || containerSlot >= containerItemIds.length) {
                return false;
            }
            if (
                stonecutterMenuOpen
                    && stonecutterOfferSelected
                    && containerSlot == 1
                    && "minecraft:cobblestone".equals(containerItemIds[0])
                    && containerCounts[0] > 0
            ) {
                cobblestoneSlabs += containerCounts[0] * 2;
                containerItemIds[0] = null;
                containerCounts[0] = 0;
                containerItemIds[1] = null;
                containerCounts[1] = 0;
                stonecutterOfferSelected = false;
                return true;
            }
            String itemId = containerItemIds[containerSlot];
            int count = containerCounts[containerSlot];
            if (itemId == null || count <= 0) {
                return false;
            }
            switch (itemId) {
                case "minecraft:diamond" -> diamonds += count;
                case "minecraft:lapis_lazuli" -> lapisLazuli += count;
                case "minecraft:bread" -> breads += count;
                default -> {
                    return false;
                }
            }
            containerItemIds[containerSlot] = null;
            containerCounts[containerSlot] = 0;
            return true;
        }

        private static String cookedItemFor(String rawItemId) {
            if (rawItemId == null) {
                return "";
            }
            return switch (rawItemId) {
                case "minecraft:beef" -> "minecraft:cooked_beef";
                case "minecraft:porkchop" -> "minecraft:cooked_porkchop";
                case "minecraft:chicken" -> "minecraft:cooked_chicken";
                case "minecraft:raw_iron" -> "minecraft:iron_ingot";
                default -> "";
            };
        }

        private void decrementItem(String itemId, int count) {
            if (logItemId.equals(itemId)) {
                oakLogs -= count;
                return;
            }
            if (planksItemId.equals(itemId)) {
                oakPlanks -= count;
                return;
            }
            switch (itemId) {
                case "minecraft:beef" -> beef -= count;
                case "minecraft:porkchop" -> porkchops -= count;
                case "minecraft:chicken" -> chickens -= count;
                case "minecraft:cooked_beef" -> cookedBeef -= count;
                case "minecraft:cooked_porkchop" -> cookedPorkchops -= count;
                case "minecraft:cooked_chicken" -> cookedChickens -= count;
                case "minecraft:bread" -> breads -= count;
                default -> throw new IllegalArgumentException("unsupported test item " + itemId);
            }
        }

        @Override
        public boolean waitForDayTimeAtOrAfter(long dayTime, Duration duration) {
            operations.add("waitNight");
            return true;
        }

        @Override
        public boolean waitForDayTimeBelow(long dayTime, Duration duration) {
            operations.add("waitMorning");
            return true;
        }

        @Override
        public ScenarioHeldItem giveAndSelect(String itemId, int count, int hotbarSlot, Duration timeout) {
            operations.add("debug:give:" + itemId + ":" + count + ":" + hotbarSlot);
            if ("minecraft:cobblestone".equals(itemId)) {
                cobblestones += count;
            }
            return new ScenarioHeldItem(itemId, count);
        }

        @Override
        public boolean clickContainerButton(int buttonId, Duration duration) {
            operations.add("containerButton:" + buttonId);
            if (
                !stonecutterMenuOpen
                    || buttonId < 0
                    || buttonId >= 3
                    || !"minecraft:cobblestone".equals(containerItemIds[0])
                    || containerCounts[0] <= 0
            ) {
                return false;
            }
            stonecutterOfferSelected = buttonId == stonecutterSlabOfferId;
            if (stonecutterOfferSelected) {
                containerItemIds[1] = "minecraft:cobblestone_slab";
                containerCounts[1] = 2;
            } else {
                containerItemIds[1] = buttonId == 0
                    ? "minecraft:cobblestone_stairs"
                    : "minecraft:cobblestone_wall";
                containerCounts[1] = 1;
            }
            return true;
        }

        @Override
        public void sendCommand(String command) {
            operations.add("debug:command");
        }

        @Override
        public void sendChatMessage(String message) {
            operations.add("sendChat:" + message);
        }

        @Override
        public boolean waitForChatMessage(String expectedText, Duration timeout) {
            operations.add("waitChat:" + expectedText);
            return true;
        }

        @Override
        public boolean teleportTo(double x, double y, double z, Duration timeout) {
            operations.add("debug:teleport");
            return true;
        }

        @Override
        public ScenarioBlockPair findOccupiedPair(ScenarioReach reach) {
            throw new UnsupportedOperationException("not used by playable scenario");
        }

        @Override
        public ScenarioBlockPair findPlaceablePair(ScenarioReach reach) {
            throw new UnsupportedOperationException("not used by playable scenario");
        }

        @Override
        public boolean waitForStableBlocks(ScenarioBlockPair pair, Duration duration) {
            throw new UnsupportedOperationException("not used by playable scenario");
        }

        @Override
        public boolean waitForNoFluid(ScenarioBlockTarget target, Duration duration) {
            throw new UnsupportedOperationException("not used by playable scenario");
        }

        @Override
        public ScenarioBreakResult breakBlock(
            ScenarioBlockTarget target,
            String expectedDropItemId,
            int expectedSelectedCount,
            Duration timeout
        ) {
            throw new UnsupportedOperationException("not used by playable scenario");
        }
    }
}
