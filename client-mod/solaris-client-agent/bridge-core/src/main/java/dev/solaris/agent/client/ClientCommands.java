package dev.solaris.agent.client;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import dev.solaris.agent.bridge.CommandRegistry;

import java.nio.file.Path;

public final class ClientCommands {
    private static final String BRIDGE_VERSION = "0.1.0";

    private ClientCommands() {
    }

    public static CommandRegistry create(ClientTaskExecutor executor, ClientFacade client) {
        CommandRegistry registry = new CommandRegistry();
        registry.register("ping", request -> {
            JsonObject payload = new JsonObject();
            payload.addProperty("bridge_version", BRIDGE_VERSION);
            payload.addProperty("agent", "solaris-client-agent");
            return payload;
        });
        registry.register("connect", request -> executor.callOnClientThread(() -> {
            ServerAddress address = parseServerAddress(request.payload());
            client.connect(address.host, address.port);
            return ok();
        }));
        registry.register("wait_play", request -> waitPlay(executor, client, timeoutSeconds(request.payload())));
        registry.register("state", request -> executor.callOnClientThread(() -> snapshotJson(client.snapshot())));
        registry.register("set_hotbar_slot", request -> executor.callOnClientThread(() -> {
            client.selectHotbarSlot(request.payload().get("slot").getAsInt());
            return ok();
        }));
        registry.register("look_at_block", request -> executor.callOnClientThread(() -> {
            JsonObject payload = request.payload();
            client.lookAtBlock(
                payload.get("x").getAsInt(),
                payload.get("y").getAsInt(),
                payload.get("z").getAsInt(),
                payload.get("face").getAsString()
            );
            return ok();
        }));
        registry.register("use_item_on", request -> executor.callOnClientThread(() -> {
            JsonObject payload = request.payload();
            client.useItemOn(
                payload.get("x").getAsInt(),
                payload.get("y").getAsInt(),
                payload.get("z").getAsInt(),
                payload.get("face").getAsString()
            );
            return ok();
        }));
        registry.register("move_forward", request -> {
            client.moveForward(durationMillis(request.payload()));
            return ok();
        });
        registry.register("screenshot", request -> executor.callOnClientThread(() -> {
            Path path = Path.of(request.payload().get("path").getAsString());
            Path written = client.screenshot(path);
            JsonObject response = ok();
            response.addProperty("path", written.toString());
            return response;
        }));
        registry.register("run_scenario", request -> {
            JsonObject payload = request.payload();
            ClientScenarioReport report = client.runScenario(
                payload.get("id").getAsString(),
                Path.of(payload.get("screenshots_dir").getAsString())
            );
            return scenarioReportJson(report);
        });
        registry.register("disconnect", request -> executor.callOnClientThread(() -> {
            client.disconnect();
            return ok();
        }));
        return registry;
    }

    private static JsonObject snapshotJson(ClientSnapshot snapshot) {
        JsonObject payload = new JsonObject();
        payload.addProperty("in_play", snapshot.inPlay());
        payload.addProperty("dimension", snapshot.dimension());
        payload.addProperty("x", snapshot.x());
        payload.addProperty("y", snapshot.y());
        payload.addProperty("z", snapshot.z());
        payload.addProperty("selected_hotbar_slot", snapshot.selectedHotbarSlot());
        payload.addProperty("current_screen", snapshot.currentScreen());
        payload.addProperty("disconnect_reason", snapshot.disconnectReason());
        return payload;
    }

    private static JsonObject waitPlay(
        ClientTaskExecutor executor,
        ClientFacade client,
        double timeoutSeconds
    ) throws Exception {
        long deadlineNanos = System.nanoTime() + (long) (timeoutSeconds * 1_000_000_000L);
        ClientSnapshot lastSnapshot = unavailableSnapshot("");
        do {
            try {
                lastSnapshot = executor.callOnClientThread(client::snapshot);
            } catch (Exception error) {
                lastSnapshot = unavailableSnapshot(error.getMessage());
            }
            if (lastSnapshot.inPlay()) {
                return snapshotJson(lastSnapshot);
            }
            Thread.sleep(10);
        } while (System.nanoTime() < deadlineNanos);
        return snapshotJson(lastSnapshot);
    }

    private static ClientSnapshot unavailableSnapshot(String reason) {
        return new ClientSnapshot(false, "", 0.0, 0.0, 0.0, -1, "client-unavailable", reason);
    }

    private static JsonObject scenarioReportJson(ClientScenarioReport report) {
        JsonObject payload = new JsonObject();
        payload.addProperty("result", report.result());
        payload.addProperty("id", report.id());
        JsonArray observations = new JsonArray();
        for (String observation : report.observations()) {
            observations.add(observation);
        }
        payload.add("observations", observations);
        return payload;
    }

    private static ServerAddress parseServerAddress(JsonObject payload) {
        if (payload.has("server_addr")) {
            String address = payload.get("server_addr").getAsString();
            int separator = address.lastIndexOf(':');
            if (separator <= 0 || separator == address.length() - 1) {
                throw new IllegalArgumentException("server_addr must be host:port");
            }
            return new ServerAddress(
                address.substring(0, separator),
                Integer.parseInt(address.substring(separator + 1))
            );
        }
        String host = payload.has("host") ? payload.get("host").getAsString() : "127.0.0.1";
        int port = payload.has("port") ? payload.get("port").getAsInt() : 25565;
        return new ServerAddress(host, port);
    }

    private static double timeoutSeconds(JsonObject payload) {
        return payload.has("timeout_seconds") ? payload.get("timeout_seconds").getAsDouble() : 30.0;
    }

    private static int durationMillis(JsonObject payload) {
        int durationMillis = payload.has("duration_millis")
            ? payload.get("duration_millis").getAsInt()
            : 750;
        if (durationMillis <= 0 || durationMillis > 5_000) {
            throw new IllegalArgumentException("duration_millis must be 1..5000");
        }
        return durationMillis;
    }

    private static JsonObject ok() {
        JsonObject payload = new JsonObject();
        payload.addProperty("status", "ok");
        return payload;
    }

    private record ServerAddress(String host, int port) {
    }
}
