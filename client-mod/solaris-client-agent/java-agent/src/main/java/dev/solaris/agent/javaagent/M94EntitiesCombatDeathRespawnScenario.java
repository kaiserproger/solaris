package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;

import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

final class M94EntitiesCombatDeathRespawnScenario {
    static final String ID = "m94-05-entities-combat-death-respawn";
    private static final Duration ENTITY_TIMEOUT = Duration.ofSeconds(8);
    private static final Duration DEATH_TIMEOUT = Duration.ofSeconds(5);
    private static final Duration RESPAWN_TIMEOUT = Duration.ofSeconds(8);
    private static final double MAX_SUMMONED_ENTITY_DISTANCE_SQUARED = 256.0;

    ClientScenarioReport run(String id, Path screenshotsDir, ScenarioClient client) {
        if (!ID.equals(id)) {
            return new ClientScenarioReport("blocked", id, List.of("unsupported scenario id: " + id));
        }

        List<String> observations = new ArrayList<>();
        try {
            ScenarioEntityObservation cow = client.summonEntityNearPlayer(
                "minecraft:cow",
                0.0,
                0.0,
                4.0,
                ENTITY_TIMEOUT
            );
            boolean entityVisible = cow != null
                && cow.distanceSquared() <= MAX_SUMMONED_ENTITY_DISTANCE_SQUARED;
            observations.add(
                "visible entity: " + (entityVisible ? "passed" : "failed")
                    + " type=minecraft:cow"
                    + (entityVisible
                        ? " entity_id=" + cow.entityId()
                            + " position=" + coordinates(cow)
                            + " distance_squared=" + cow.distanceSquared()
                        : "")
            );

            client.sendCommand("debug survival damage 10000");
            boolean deathScreen = client.waitForDeathScreen(DEATH_TIMEOUT);
            boolean respawned = deathScreen && client.performRespawn(RESPAWN_TIMEOUT);
            observations.add(
                "death/respawn: " + (deathScreen && respawned ? "passed" : "failed")
                    + " death_screen=" + deathScreen
                    + " respawned=" + respawned
            );
            observations.add(
                "blocked: hostile combat, melee damage and knockback, mob drops, XP pickup, projectiles, "
                    + "shield timing, vehicles, and broad AI/pathing need dedicated in-client primitives "
                    + "before " + ID + " can be green"
            );
            observations.add("screenshots directory available to driver: " + screenshotsDir);

            return new ClientScenarioReport(
                entityVisible && deathScreen && respawned ? "blocked" : "failed",
                id,
                observations
            );
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private static String coordinates(ScenarioEntityObservation entity) {
        return entity.x() + "," + entity.y() + "," + entity.z();
    }
}
