package dev.solaris.agent.bridge;

import dev.solaris.agent.client.ClientCommands;
import dev.solaris.agent.client.ClientFacade;
import dev.solaris.agent.client.ClientScenarioReport;
import dev.solaris.agent.client.ClientSnapshot;
import dev.solaris.agent.client.ClientTaskExecutor;
import org.junit.jupiter.api.Test;

import java.nio.file.Path;
import java.util.List;
import java.util.concurrent.Callable;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class ClientCommandsTest {
    @Test
    void pingReportsBridgeVersionWithoutClientThread() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        CommandRegistry registry = ClientCommands.create(executor, new FakeClient());

        BridgeCommand ping = registry.find("ping").orElseThrow();

        assertEquals("0.1.0", ping.execute(request("ping", "{}")).get("bridge_version").getAsString());
        assertEquals(0, executor.calls);
    }

    @Test
    void stateRunsThroughClientThreadExecutor() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        CommandRegistry registry = ClientCommands.create(executor, new FakeClient());

        BridgeCommand state = registry.find("state").orElseThrow();

        assertEquals("minecraft:overworld", state.execute(request("state", "{}")).get("dimension").getAsString());
        assertEquals(1, executor.calls);
    }

    @Test
    void waitPlayPollsSnapshotsUntilClientReachesPlay() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        DelayedPlayClient client = new DelayedPlayClient();
        CommandRegistry registry = ClientCommands.create(executor, client);

        BridgeCommand waitPlay = registry.find("wait_play").orElseThrow();

        assertTrue(waitPlay.execute(
            request("wait_play", "{\"timeout_seconds\":1.0}")
        ).get("in_play").getAsBoolean());
        assertEquals(2, client.snapshotCalls);
        assertEquals(2, executor.calls);
    }

    @Test
    void waitPlaySurvivesTransientClientThreadUnavailability() throws Exception {
        TransientFailureExecutor executor = new TransientFailureExecutor();
        CommandRegistry registry = ClientCommands.create(executor, new FakeClient());

        BridgeCommand waitPlay = registry.find("wait_play").orElseThrow();

        assertTrue(waitPlay.execute(
            request("wait_play", "{\"timeout_seconds\":1.0}")
        ).get("in_play").getAsBoolean());
        assertEquals(2, executor.calls);
    }

    @Test
    void waitPlayTimesOutWithLastSnapshot() throws Exception {
        NeverPlayClient client = new NeverPlayClient();
        CommandRegistry registry = ClientCommands.create(new ImmediateExecutor(), client);

        BridgeCommand waitPlay = registry.find("wait_play").orElseThrow();

        assertEquals("", waitPlay.execute(
            request("wait_play", "{\"timeout_seconds\":0.01}")
        ).get("dimension").getAsString());
        assertTrue(client.snapshotCalls > 0);
    }

    @Test
    void connectParsesDriverServerAddressOnClientThread() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);

        BridgeCommand connect = registry.find("connect").orElseThrow();

        connect.execute(request("connect", "{\"server_addr\":\"127.0.0.1:25565\"}"));

        assertEquals("127.0.0.1", client.host);
        assertEquals(25565, client.port);
        assertEquals(1, executor.calls);
    }

    @Test
    void screenshotUsesExactDriverPath() throws Exception {
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(new ImmediateExecutor(), client);
        Path path = Path.of("run/screenshots/m94-02b-rejected-block-resync.png");

        BridgeCommand screenshot = registry.find("screenshot").orElseThrow();

        assertEquals(path.toString(), screenshot.execute(
            request("screenshot", "{\"path\":\"" + path + "\"}")
        ).get("path").getAsString());
        assertEquals(path, client.screenshotPath);
    }

    @Test
    void moveForwardDoesNotHoldTheClientThreadExecutor() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);

        BridgeCommand moveForward = registry.find("move_forward").orElseThrow();

        assertEquals("ok", moveForward.execute(
            request("move_forward", "{\"duration_millis\":750}")
        ).get("status").getAsString());
        assertEquals(750, client.moveForwardMillis);
        assertEquals(0, executor.calls, "movement duration must not block the client thread executor");
    }

    @Test
    void runScenarioReturnsStructuredScenarioReport() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        FakeClient client = new FakeClient();
        CommandRegistry registry = ClientCommands.create(executor, client);

        BridgeCommand runScenario = registry.find("run_scenario").orElseThrow();

        assertEquals("passed", runScenario.execute(request(
            "run_scenario",
            "{\"id\":\"m94-02b-rejected-block-resync\",\"screenshots_dir\":\"run/screenshots\"}"
        )).get("result").getAsString());
        assertEquals("m94-02b-rejected-block-resync", client.scenarioId);
        assertEquals(Path.of("run/screenshots"), client.scenarioScreenshotsDir);
        assertEquals(0, executor.calls, "long-running scenarios must not block the client thread");
    }

    private static BridgeRequest request(String command, String payload) {
        return BridgeCodec.decodeRequest(
            "{\"id\":1,\"secret\":\"s\",\"command\":\"" + command + "\",\"payload\":" + payload + "}"
        );
    }

    private static final class ImmediateExecutor implements ClientTaskExecutor {
        int calls;

        @Override
        public <T> T callOnClientThread(Callable<T> callable) throws Exception {
            calls += 1;
            return callable.call();
        }
    }

    private static final class TransientFailureExecutor implements ClientTaskExecutor {
        int calls;

        @Override
        public <T> T callOnClientThread(Callable<T> callable) throws Exception {
            calls += 1;
            if (calls == 1) {
                throw new IllegalStateException("minecraft singleton is not initialized");
            }
            return callable.call();
        }
    }

    private static class FakeClient implements ClientFacade {
        String host;
        int port;
        Path screenshotPath;
        String scenarioId;
        Path scenarioScreenshotsDir;
        int moveForwardMillis;

        @Override
        public ClientSnapshot snapshot() {
            return new ClientSnapshot(
                true,
                "minecraft:overworld",
                10.0,
                64.0,
                -3.0,
                2,
                "none",
                ""
            );
        }

        @Override
        public void connect(String host, int port) {
            this.host = host;
            this.port = port;
        }

        @Override
        public void selectHotbarSlot(int slot) {
        }

        @Override
        public void lookAtBlock(int x, int y, int z, String face) {
        }

        @Override
        public void useItemOn(int x, int y, int z, String face) {
        }

        @Override
        public void moveForward(int durationMillis) {
            moveForwardMillis = durationMillis;
        }

        @Override
        public Path screenshot(Path path) {
            screenshotPath = path;
            return path;
        }

        @Override
        public ClientScenarioReport runScenario(String id, Path screenshotsDir) {
            scenarioId = id;
            scenarioScreenshotsDir = screenshotsDir;
            return new ClientScenarioReport("passed", id, List.of("fake client executed scenario"));
        }

        @Override
        public void disconnect() {
        }
    }

    private static final class DelayedPlayClient extends FakeClient {
        int snapshotCalls;

        @Override
        public ClientSnapshot snapshot() {
            snapshotCalls += 1;
            if (snapshotCalls == 1) {
                return new ClientSnapshot(false, "", 0.0, 0.0, 0.0, -1, "joining", "");
            }
            return super.snapshot();
        }
    }

    private static final class NeverPlayClient extends FakeClient {
        int snapshotCalls;

        @Override
        public ClientSnapshot snapshot() {
            snapshotCalls += 1;
            return new ClientSnapshot(false, "", 0.0, 0.0, 0.0, -1, "joining", "");
        }
    }
}
