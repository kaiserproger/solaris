package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;

import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;

final class M94M40M41RouteScenario {
    static final String ID = "m94-07-m40-m41-route-with-metrics";
    static final String DEEP_WATER_ID = "m94-07a-deep-water-feel";

    private static final Duration ENTITY_TIMEOUT = Duration.ofSeconds(8);
    private static final Duration WATER_SETUP_TIMEOUT = Duration.ofSeconds(12);
    private static final Duration WATER_INPUT_TIMEOUT = Duration.ofSeconds(15);
    private static final Duration WATER_TICK_TIMEOUT = Duration.ofSeconds(12);
    private static final int FIXTURE_X = 4;
    private static final int FIXTURE_Y = 96;
    private static final int FIXTURE_Z = 0;
    private static final double MIN_VERTICAL_MOVEMENT = 0.15;
    private static final double MIN_SWIM_DISTANCE = 1.0;
    private static final double MAX_SWIMMING_EYE_HEIGHT = 0.8;
    private static final double MAX_SWIMMING_BODY_HEIGHT = 0.8;
    private static final double MAX_SUMMONED_ENTITY_DISTANCE_SQUARED = 256.0;

    static boolean supports(String id) {
        return ID.equals(id) || DEEP_WATER_ID.equals(id);
    }

    ClientScenarioReport run(String id, Path screenshotsDir, ScenarioClient client) {
        if (DEEP_WATER_ID.equals(id)) {
            return runDeepWater(id, screenshotsDir, client);
        }
        if (!ID.equals(id)) {
            return new ClientScenarioReport("blocked", id, List.of("unsupported scenario id: " + id));
        }

        List<String> observations = new ArrayList<>();
        ClientScenarioReport deepWater = runDeepWater(DEEP_WATER_ID, screenshotsDir, client);
        appendSubprobe("deep-water", deepWater, observations);
        if ("failed".equals(deepWater.result())) {
            return new ClientScenarioReport("failed", id, observations);
        }
        if ("blocked".equals(deepWater.result())) {
            return new ClientScenarioReport("blocked", id, observations);
        }

        ClientScenarioReport water = new M94WaterBucketScenario().run(
            M94WaterBucketScenario.ID,
            screenshotsDir,
            client
        );
        appendSubprobe("water-bucket", water, observations);
        if ("failed".equals(water.result())) {
            return new ClientScenarioReport("failed", id, observations);
        }
        if ("blocked".equals(water.result())) {
            return new ClientScenarioReport("blocked", id, observations);
        }

        ClientScenarioReport solid = new M94SolidBlockScenario().run(
            M94SolidBlockScenario.ID,
            screenshotsDir,
            client
        );
        appendSubprobe("solid/drop", solid, observations);
        if ("failed".equals(solid.result())) {
            return new ClientScenarioReport("failed", id, observations);
        }
        if ("blocked".equals(solid.result())) {
            return new ClientScenarioReport("blocked", id, observations);
        }

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
            observations.add(
                "blocked: sugar cane support/cascade/drop, TPS/lock log analysis, "
                    + "owner M40/M41 frozen-world route, and broad performance evidence need "
                    + "dedicated gates before " + ID + " can be green; B4 deep-water feel is passed"
            );
            observations.add("screenshots directory available to driver: " + screenshotsDir);

            return new ClientScenarioReport(entityVisible ? "blocked" : "failed", id, observations);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private ClientScenarioReport runDeepWater(
        String id,
        Path screenshotsDir,
        ScenarioClient client
    ) {
        List<String> observations = new ArrayList<>();
        try {
            if (!client.waitForTicks(120, WATER_TICK_TIMEOUT)) {
                return failed(id, observations, "initial chunk stream did not settle for 120 client ticks");
            }
            observations.add("initial client ticks before fixture: 120");
            String fixtureCommand = "debug water-corridor "
                + FIXTURE_X + " " + FIXTURE_Y + " " + FIXTURE_Z;
            client.sendCommand(fixtureCommand);
            observations.add("fixture command: " + fixtureCommand);
            String fixtureFeedback = "Debug water corridor at "
                + FIXTURE_X + " " + FIXTURE_Y + " " + FIXTURE_Z
                + " verified 68/68 block states";
            if (!client.waitForChatMessage(fixtureFeedback, WATER_SETUP_TIMEOUT)) {
                return failed(id, observations, "operator fixture did not confirm 68/68 block edits");
            }
            ScenarioBlockTarget fixtureBottom = new ScenarioBlockTarget(
                FIXTURE_X,
                FIXTURE_Y,
                FIXTURE_Z,
                "up",
                "deep-water-fixture-bottom",
                "minecraft:water"
            );
            ScenarioBlockTarget fixtureTop = new ScenarioBlockTarget(
                FIXTURE_X,
                FIXTURE_Y + 1,
                FIXTURE_Z,
                "up",
                "deep-water-fixture-top",
                "minecraft:water"
            );
            if (!client.waitForBlock(fixtureBottom, "minecraft:water", WATER_SETUP_TIMEOUT)
                || !client.waitForBlock(fixtureTop, "minecraft:water", WATER_SETUP_TIMEOUT)) {
                return failed(id, observations, "client did not observe both fixture water cells");
            }
            observations.add("client fixture blocks: bottom=water top=water");
            if (!client.waitForTicks(5, WATER_TICK_TIMEOUT)) {
                return failed(id, observations, "operator fixture did not advance five client ticks");
            }
            ScenarioDeepWaterTarget target = new ScenarioDeepWaterTarget(
                FIXTURE_X,
                FIXTURE_Y,
                FIXTURE_Y + 1,
                FIXTURE_Z,
                0.0F,
                "south"
            );

            observations.add(
                "deep-water target: x=" + target.x()
                    + " bottom_y=" + target.bottomY()
                    + " top_y=" + target.topY()
                    + " z=" + target.z()
                    + " direction=" + target.direction()
                    + " yaw=" + target.yaw()
            );
            client.setView(target.yaw(), 0.0F);

            if (!client.teleportTo(
                target.centerX(),
                target.topY() + 1.25,
                target.centerZ(),
                WATER_SETUP_TIMEOUT
            )) {
                return failed(id, observations, "source-water entry setup teleport did not converge");
            }
            if (!client.waitForTicks(20, WATER_TICK_TIMEOUT)) {
                return failed(id, observations, "source-water falling entry did not advance 20 client ticks");
            }
            ScenarioWaterObservation entry = client.waterObservation();
            appendWaterObservation("entry", entry, observations);
            boolean sourceEntry = entry.connected()
                && entry.inWater()
                && entry.underWater()
                && entry.waterFluidHeight() >= 1.0;
            if (!sourceEntry) {
                return failed(id, observations, "client did not enter the deep source-water column");
            }

            client.pressInputs(List.of("jump"), 8, WATER_INPUT_TIMEOUT);
            ScenarioWaterObservation ascent = client.waterObservation();
            appendWaterObservation("ascent", ascent, observations);
            double ascentDelta = ascent.y() - entry.y();
            observations.add("ascent delta_y=" + format(ascentDelta));
            if (!ascent.connected() || !ascent.inWater() || ascentDelta < MIN_VERTICAL_MOVEMENT) {
                return failed(id, observations, "jump input did not produce a retained water ascent");
            }

            if (!client.teleportTo(
                target.centerX(),
                target.topY() + 0.45,
                target.centerZ(),
                WATER_SETUP_TIMEOUT
            )) {
                return failed(id, observations, "dive setup teleport did not converge");
            }
            if (!client.waitForTicks(3, WATER_TICK_TIMEOUT)) {
                return failed(id, observations, "dive setup did not advance three client ticks");
            }
            ScenarioWaterObservation diveBefore = client.waterObservation();
            client.pressInputs(List.of("sneak"), 8, WATER_INPUT_TIMEOUT);
            ScenarioWaterObservation diveAfter = client.waterObservation();
            appendWaterObservation("dive-before", diveBefore, observations);
            appendWaterObservation("dive-after", diveAfter, observations);
            double diveDelta = diveAfter.y() - diveBefore.y();
            observations.add("dive delta_y=" + format(diveDelta));
            if (!diveAfter.connected() || !diveAfter.inWater() || diveDelta > -MIN_VERTICAL_MOVEMENT) {
                return failed(id, observations, "sneak input did not produce a retained water dive");
            }

            if (!client.teleportTo(
                target.centerX(),
                target.bottomY() + 0.05,
                target.centerZ(),
                WATER_SETUP_TIMEOUT
            )) {
                return failed(id, observations, "swim setup teleport did not converge");
            }
            client.setView(target.yaw(), 0.0F);
            if (!client.waitForTicks(3, WATER_TICK_TIMEOUT)) {
                return failed(id, observations, "swim setup did not advance three client ticks");
            }
            ScenarioWaterObservation swimBefore = client.waterObservation();
            client.pressInputs(List.of("sprint", "forward"), 30, WATER_INPUT_TIMEOUT);
            ScenarioWaterObservation swimAfter = client.waterObservation();
            appendWaterObservation("swim-before", swimBefore, observations);
            appendWaterObservation("swim-after", swimAfter, observations);
            double swimDistance = swimAfter.horizontalDistance(swimBefore);
            observations.add("swim horizontal_delta=" + format(swimDistance));
            boolean swimPose = swimAfter.swimming()
                && "swimming".equals(swimAfter.pose())
                && swimAfter.eyeHeight() <= MAX_SWIMMING_EYE_HEIGHT
                && swimAfter.bodyHeight() <= MAX_SWIMMING_BODY_HEIGHT;
            boolean eyeTransition = swimBefore.eyeHeight() > swimAfter.eyeHeight()
                && swimBefore.bodyHeight() > swimAfter.bodyHeight();
            if (!swimAfter.connected()
                || !swimAfter.inWater()
                || swimDistance < MIN_SWIM_DISTANCE
                || !swimPose
                || !eyeTransition) {
                return failed(id, observations, "sprint-forward did not produce vanilla swim pose/eye transition");
            }

            ScenarioWaterObservation airBaseline = swimAfter;
            if (!airBaseline.underWater()) {
                if (!client.teleportTo(
                    target.centerX(),
                    target.bottomY() + 0.05,
                    target.centerZ(),
                    WATER_SETUP_TIMEOUT
                )) {
                    return failed(id, observations, "air-loss setup teleport did not converge");
                }
                if (!client.waitForTicks(3, WATER_TICK_TIMEOUT)) {
                    return failed(id, observations, "air-loss setup did not advance three client ticks");
                }
                airBaseline = client.waterObservation();
            }
            if (!client.waitForTicks(40, WATER_TICK_TIMEOUT)) {
                return failed(id, observations, "air-loss observation did not advance 40 client ticks");
            }
            ScenarioWaterObservation airLoss = client.waterObservation();
            appendWaterObservation("air-baseline", airBaseline, observations);
            appendWaterObservation("air-loss", airLoss, observations);
            if (!airLoss.connected()
                || !airLoss.underWater()
                || airLoss.air() >= airBaseline.air()) {
                return failed(id, observations, "underwater eyes did not consume client-visible air");
            }

            if (!client.teleportTo(
                target.centerX() + 1.0,
                target.topY() + 1.0,
                target.centerZ(),
                WATER_SETUP_TIMEOUT
            )) {
                return failed(id, observations, "air-recovery dry-wall teleport did not converge");
            }
            if (!client.waitForTicks(20, WATER_TICK_TIMEOUT)) {
                return failed(id, observations, "air recovery did not advance 20 client ticks");
            }
            ScenarioWaterObservation recovered = client.waterObservation();
            appendWaterObservation("recovered", recovered, observations);
            if (!recovered.connected()
                || recovered.underWater()
                || recovered.air() <= airLoss.air()) {
                return failed(id, observations, "leaving water did not recover client-visible air");
            }

            observations.add(
                "deep-water checks: passed"
                    + " source_entry=true"
                    + " ascent=true"
                    + " dive=true"
                    + " swimming_pose=true"
                    + " fluid_height=true"
                    + " eye_transition=true"
                    + " air_loss=true"
                    + " air_recovery=true"
                    + " retained_input_movement=true"
                    + " connected=true"
            );
            observations.add("screenshots directory available to driver: " + screenshotsDir);
            return new ClientScenarioReport("passed", id, observations);
        } catch (Exception error) {
            observations.add("deep-water scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private static ClientScenarioReport failed(
        String id,
        List<String> observations,
        String message
    ) {
        observations.add("deep-water check failed: " + message);
        return new ClientScenarioReport("failed", id, observations);
    }

    private static void appendWaterObservation(
        String label,
        ScenarioWaterObservation observation,
        List<String> observations
    ) {
        observations.add(
            label
                + ": position=" + format(observation.x())
                + "," + format(observation.y())
                + "," + format(observation.z())
                + " eye_y=" + format(observation.eyeY())
                + " eye_height=" + format(observation.eyeHeight())
                + " body_height=" + format(observation.bodyHeight())
                + " in_water=" + observation.inWater()
                + " under_water=" + observation.underWater()
                + " swimming=" + observation.swimming()
                + " pose=" + observation.pose()
                + " fluid_height=" + format(observation.waterFluidHeight())
                + " feet_block=" + observation.feetBlockId()
                + " feet_fluid=" + observation.feetFluidId()
                + " feet_source=" + observation.feetFluidSource()
                + " feet_cell_height=" + format(observation.feetCellFluidHeight())
                + " eye_block=" + observation.eyeBlockId()
                + " eye_fluid=" + observation.eyeFluidId()
                + " eye_source=" + observation.eyeFluidSource()
                + " eye_cell_height=" + format(observation.eyeCellFluidHeight())
                + " air=" + observation.air() + "/" + observation.maxAir()
                + " health=" + format(observation.health())
                + " connected=" + observation.connected()
        );
    }

    private static void appendSubprobe(
        String label,
        ClientScenarioReport subprobe,
        List<String> observations
    ) {
        observations.add(label + " subprobe result: " + subprobe.result());
        for (String observation : subprobe.observations()) {
            observations.add(label + " subprobe: " + observation);
        }
    }

    private static String coordinates(ScenarioEntityObservation entity) {
        return entity.x() + "," + entity.y() + "," + entity.z();
    }

    private static String format(double value) {
        return String.format(Locale.ROOT, "%.4f", value);
    }
}
