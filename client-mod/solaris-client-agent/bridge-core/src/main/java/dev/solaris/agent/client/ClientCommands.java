package dev.solaris.agent.client;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import dev.solaris.agent.bridge.CommandRegistry;

import java.nio.file.Path;
import java.time.Duration;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Set;
import java.util.UUID;
import java.util.concurrent.TimeoutException;

public final class ClientCommands {
    private static final String BRIDGE_VERSION = "0.1.0";
    private static final Set<String> ALLOWED_INPUTS = Set.of(
        "forward", "back", "left", "right", "jump", "sneak", "sprint", "attack", "use",
        "swap_offhand"
    );
    private static final Set<String> BLOCK_FACES = Set.of(
        "down", "up", "north", "south", "west", "east"
    );
    private static final Set<String> ENTITY_INTERACTION_HANDS = Set.of("main_hand", "off_hand");
    private static final Set<String> CONTAINER_CLICK_BUTTONS = Set.of("primary", "secondary");

    private ClientCommands() {
    }

    public static CommandRegistry create(ClientTaskExecutor executor, ClientFacade client) {
        CommandRegistry registry = new CommandRegistry();
        registry.registerConcurrent("ping", request -> {
            JsonObject payload = new JsonObject();
            payload.addProperty("bridge_version", BRIDGE_VERSION);
            payload.addProperty("agent", "solaris-client-agent");
            return payload;
        });
        registry.register("connect", request -> {
            ServerAddress address = parseServerAddress(request.payload());
            return executor.callOnClientThread(() -> {
                client.connect(address.host, address.port);
                return ok();
            });
        });
        registry.registerConcurrent(
            "wait_play",
            request -> waitPlay(executor, client, timeoutSeconds(request.payload()))
        );
        registry.registerConcurrent("state", request -> versionedSnapshot(executor, client));
        registry.registerConcurrent("wait_state_change", request -> {
            JsonObject payload = request.payload();
            long observedVersion = boundedLong(payload, "observed_version", 0L, Long.MAX_VALUE);
            if (client.stateVersion() == observedVersion) {
                boolean changed = client.awaitStateChange(observedVersion, eventTimeout(payload));
                if (!changed) {
                    throw new TimeoutException("client state event did not arrive");
                }
            }
            return versionedSnapshot(executor, client);
        });
        registry.registerConcurrent("observe", request -> executor.callOnClientThread(client::observe));
        registry.registerConcurrent("read_block", request -> {
            JsonObject payload = request.payload();
            int x = boundedInt(payload, "x", Integer.MIN_VALUE, Integer.MAX_VALUE);
            int y = boundedInt(payload, "y", Integer.MIN_VALUE, Integer.MAX_VALUE);
            int z = boundedInt(payload, "z", Integer.MIN_VALUE, Integer.MAX_VALUE);
            return executor.callOnClientThread(() -> client.readBlock(x, y, z));
        });
        registry.registerConcurrent("wait_loaded_block", request -> {
            JsonObject payload = request.payload();
            return client.waitForLoadedBlock(
                boundedInt(payload, "x", Integer.MIN_VALUE, Integer.MAX_VALUE),
                boundedInt(payload, "y", Integer.MIN_VALUE, Integer.MAX_VALUE),
                boundedInt(payload, "z", Integer.MIN_VALUE, Integer.MAX_VALUE),
                eventTimeout(payload)
            );
        });
        registry.registerConcurrent("scan_blocks", request -> {
            JsonObject payload = request.payload();
            int minX = boundedInt(payload, "min_x", Integer.MIN_VALUE, Integer.MAX_VALUE);
            int minY = boundedInt(payload, "min_y", Integer.MIN_VALUE, Integer.MAX_VALUE);
            int minZ = boundedInt(payload, "min_z", Integer.MIN_VALUE, Integer.MAX_VALUE);
            int maxX = boundedInt(payload, "max_x", Integer.MIN_VALUE, Integer.MAX_VALUE);
            int maxY = boundedInt(payload, "max_y", Integer.MIN_VALUE, Integer.MAX_VALUE);
            int maxZ = boundedInt(payload, "max_z", Integer.MIN_VALUE, Integer.MAX_VALUE);
            int maxBlocks = optionalBoundedInt(payload, "max_blocks", 1, 4096, 4096);
            ensureBoundedBox(minX, minY, minZ, maxX, maxY, maxZ, maxBlocks);
            return executor.callOnClientThread(() -> client.scanBlocks(
                minX,
                minY,
                minZ,
                maxX,
                maxY,
                maxZ,
                maxBlocks
            ));
        });
        registry.registerConcurrent("list_entities", request -> {
            JsonObject payload = request.payload();
            double radius = optionalBoundedDouble(payload, "radius", 0.0, 128.0, 32.0);
            int limit = optionalBoundedInt(payload, "limit", 1, 512, 128);
            return executor.callOnClientThread(() -> client.listEntities(radius, limit));
        });
        registry.registerConcurrent("recipe_book", request -> {
            int limit = optionalBoundedInt(request.payload(), "limit", 1, 8192, 2048);
            return executor.callOnClientThread(() -> client.readRecipeBook(limit));
        });
        registry.registerConcurrent("wait_visible_entity", request -> {
            JsonObject payload = request.payload();
            return client.waitForVisibleEntity(
                boundedString(payload, "entity_type", 128),
                optionalBoundedDouble(payload, "radius", 0.0, 128.0, 32.0),
                eventTimeout(payload)
            );
        });
        registry.registerConcurrent("wait_entity_motion", request -> {
            JsonObject payload = request.payload();
            return client.waitForEntityMotion(
                boundedInt(payload, "entity_id", 0, Integer.MAX_VALUE),
                boundedUuid(payload, "entity_uuid"),
                boundedString(payload, "entity_type", 128),
                optionalBoundedDouble(
                    payload,
                    "minimum_horizontal_distance",
                    0.001,
                    128.0,
                    0.01
                ),
                optionalBoundedDouble(payload, "minimum_vertical_rise", 0.0, 128.0, 0.0),
                eventTimeout(payload)
            );
        });
        registry.registerConcurrent("wait_entity_removed", request -> {
            JsonObject payload = request.payload();
            return client.waitForEntityRemoved(
                boundedInt(payload, "entity_id", 0, Integer.MAX_VALUE),
                boundedUuid(payload, "entity_uuid"),
                boundedString(payload, "entity_type", 128),
                eventTimeout(payload)
            );
        });
        registry.registerConcurrent("wait_health_below", request -> {
            JsonObject payload = request.payload();
            return client.waitForHealthBelow(
                boundedDouble(payload, "health", 0.001, 2048.0),
                eventTimeout(payload)
            );
        });
        registry.registerConcurrent("wait_inventory", request -> {
            JsonObject payload = request.payload();
            return client.waitForInventoryCount(
                boundedString(payload, "item_id", 128),
                boundedInt(payload, "count", 0, 4096),
                eventTimeout(payload)
            );
        });
        registry.registerConcurrent("wait_for_container_slot", request -> {
            JsonObject payload = request.payload();
            return client.waitForContainerSlot(
                boundedInt(payload, "slot", 0, Short.MAX_VALUE),
                boundedString(payload, "item_id", 128),
                boundedInt(payload, "count", 1, 4096),
                eventTimeout(payload)
            );
        });
        registry.registerConcurrent("wait_visible_item", request -> {
            JsonObject payload = request.payload();
            return client.waitForVisibleItem(
                boundedString(payload, "item_id", 128),
                boundedInt(payload, "x", Integer.MIN_VALUE, Integer.MAX_VALUE),
                boundedInt(payload, "y", Integer.MIN_VALUE, Integer.MAX_VALUE),
                boundedInt(payload, "z", Integer.MIN_VALUE, Integer.MAX_VALUE),
                eventTimeout(payload)
            );
        });
        registry.registerConcurrent("wait_no_visible_item", request -> {
            JsonObject payload = request.payload();
            return client.waitForNoVisibleItem(
                boundedString(payload, "item_id", 128),
                boundedInt(payload, "x", Integer.MIN_VALUE, Integer.MAX_VALUE),
                boundedInt(payload, "y", Integer.MIN_VALUE, Integer.MAX_VALUE),
                boundedInt(payload, "z", Integer.MIN_VALUE, Integer.MAX_VALUE),
                eventTimeout(payload)
            );
        });
        registry.register("set_hotbar_slot", request -> {
            int slot = boundedInt(request.payload(), "slot", 0, 8);
            return executor.callOnClientThread(() -> {
                client.selectHotbarSlot(slot);
                return ok();
            });
        });
        registry.register("select_hotbar_item", request -> {
            JsonObject payload = request.payload();
            return client.selectHotbarItem(
                boundedString(payload, "item_id", 128),
                boundedInt(payload, "count", 1, 64),
                eventTimeout(payload)
            );
        });
        registry.register("navigate_to_block", request -> {
            JsonObject payload = request.payload();
            return client.navigateToBlock(
                boundedInt(payload, "x", Integer.MIN_VALUE, Integer.MAX_VALUE),
                boundedInt(payload, "y", Integer.MIN_VALUE, Integer.MAX_VALUE),
                boundedInt(payload, "z", Integer.MIN_VALUE, Integer.MAX_VALUE),
                eventTimeout(payload)
            );
        });
        registry.register("approach_entity", request -> {
            JsonObject payload = request.payload();
            return client.approachEntity(
                boundedInt(payload, "entity_id", 0, Integer.MAX_VALUE),
                eventTimeout(payload)
            );
        });
        registry.register("interact_entity", request -> {
            JsonObject payload = request.payload();
            return client.interactEntity(
                boundedInt(payload, "entity_id", 0, Integer.MAX_VALUE),
                boundedUuid(payload, "entity_uuid"),
                boundedString(payload, "entity_type", 128),
                entityInteractionHand(payload)
            );
        });
        registry.register("attack_entity_once", request -> {
            JsonObject payload = request.payload();
            return client.attackEntityOnce(
                boundedInt(payload, "entity_id", 0, Integer.MAX_VALUE),
                boundedUuid(payload, "entity_uuid"),
                boundedString(payload, "entity_type", 128),
                eventTimeout(payload)
            );
        });
        registry.register("attack_entity_until_drop_collected", request -> {
            JsonObject payload = request.payload();
            return client.attackEntityUntilDropCollected(
                boundedInt(payload, "entity_id", 0, Integer.MAX_VALUE),
                boundedString(payload, "expected_drop_item_id", 128),
                boundedInt(payload, "expected_drop_count", 1, 64),
                eventTimeout(payload)
            );
        });
        registry.register("look_at_block", request -> {
            BlockTarget target = blockTarget(request.payload());
            return executor.callOnClientThread(() -> {
                client.lookAtBlock(target.x, target.y, target.z, target.face);
                return ok();
            });
        });
        registry.register("use_item_on", request -> {
            JsonObject payload = request.payload();
            BlockTarget target = blockTarget(payload);
            return executor.callOnClientThread(() -> client.useItemOn(
                target.x,
                target.y,
                target.z,
                target.face,
                entityInteractionHand(payload)
            ));
        });
        registry.register("break_block", request -> {
            JsonObject payload = request.payload();
            BlockTarget target = blockTarget(payload);
            return client.breakBlock(
                target.x,
                target.y,
                target.z,
                target.face,
                boundedString(payload, "expected_drop_item_id", 128),
                boundedInt(payload, "expected_drop_count", 1, 64),
                eventTimeout(payload)
            );
        });
        registry.register("move_forward", request -> {
            client.moveForward(inputTicks(request.payload()));
            return ok();
        });
        registry.register("move_backward", request -> {
            client.moveBackward(inputTicks(request.payload()));
            return ok();
        });
        registry.register("press_inputs", request -> {
            JsonObject payload = request.payload();
            List<String> inputs = inputKeys(payload);
            client.pressInputs(inputs, inputTicks(payload));
            return ok();
        });
        registry.register("wait_ticks", request -> {
            client.waitTicks(boundedInt(request.payload(), "ticks", 1, 255));
            return ok();
        });
        registry.register("move_by", request -> {
            JsonObject payload = request.payload();
            int dxCm = boundedInt(payload, "dx_cm", Short.MIN_VALUE, Short.MAX_VALUE);
            int dzCm = boundedInt(payload, "dz_cm", Short.MIN_VALUE, Short.MAX_VALUE);
            return executor.callOnClientThread(() -> {
                client.moveByCentimeters(dxCm, dzCm);
                return ok();
            });
        });
        registry.register("look", request -> {
            JsonObject payload = request.payload();
            int yawDeg = boundedInt(payload, "yaw_deg", -180, 180);
            int pitchDeg = boundedInt(payload, "pitch_deg", -90, 90);
            return executor.callOnClientThread(() -> {
                client.look(yawDeg, pitchDeg);
                return ok();
            });
        });
        registry.register("close_screen", request -> executor.callOnClientThread(() -> {
            client.closeCurrentScreen();
            return ok();
        }));
        registry.register("open_inventory", request -> executor.callOnClientThread(() -> {
            client.openInventory();
            return ok();
        }));
        registry.register("respawn", request -> {
            JsonObject payload = request.payload();
            boolean hasKeys = payload.has("keys");
            boolean hasTicks = payload.has("ticks");
            if (hasKeys != hasTicks) {
                throw new IllegalArgumentException("respawn keys and ticks must be provided together");
            }
            if (hasKeys) {
                client.respawnWithInputs(
                    inputKeys(payload),
                    inputTicks(payload),
                    respawnTimeout(payload)
                );
            } else {
                client.respawn(respawnTimeout(payload));
            }
            JsonObject response = new JsonObject();
            response.addProperty("status", "respawned");
            return response;
        });
        registry.register("quick_move_container_slot", request -> {
            JsonObject payload = request.payload();
            return client.quickMoveContainerSlot(
                boundedInt(payload, "slot", 0, Short.MAX_VALUE),
                eventTimeout(payload)
            );
        });
        registry.register("click_container_slot", request -> {
            JsonObject payload = request.payload();
            return client.clickContainerSlot(
                boundedInt(payload, "slot", 0, Short.MAX_VALUE),
                containerClickButton(payload),
                eventTimeout(payload)
            );
        });
        registry.register("click_container_button", request -> {
            JsonObject payload = request.payload();
            return client.clickContainerButton(
                boundedInt(payload, "button_id", 0, Integer.MAX_VALUE),
                eventTimeout(payload)
            );
        });
        registry.register("send_chat", request -> {
            JsonObject payload = request.payload();
            String message = boundedString(payload, "message", 256);
            boolean command = optionalBoolean(payload, "command", false);
            return executor.callOnClientThread(() -> {
                client.sendChat(message, command);
                return ok();
            });
        });
        registry.register("drop_selected_item", request -> {
            JsonObject payload = request.payload();
            String itemId = boundedString(payload, "item_id", 128);
            int count = boundedInt(payload, "count", 1, 64);
            return client.dropSelectedItem(itemId, count, eventTimeout(payload));
        });
        registry.register("screenshot", request -> {
            Path path = Path.of(boundedString(request.payload(), "path", 1024));
            Path written = client.screenshot(path);
            JsonObject response = ok();
            response.addProperty("path", written.toString());
            return response;
        });
        registry.register("run_scenario", request -> {
            JsonObject payload = request.payload();
            ClientScenarioReport report = client.runScenario(
                boundedString(payload, "id", 128),
                scenarioArtifactsDirectory(payload)
            );
            return scenarioReportJson(report);
        });
        registry.register("disconnect", request -> executor.callOnClientThread(() -> {
            client.disconnect();
            return ok();
        }));
        return registry;
    }

    private static JsonObject versionedSnapshot(
        ClientTaskExecutor executor,
        ClientFacade client
    ) throws Exception {
        return executor.callOnClientThread(() -> {
            long stateVersion = client.stateVersion();
            return snapshotJson(client.snapshot(), stateVersion);
        });
    }

    private static JsonObject snapshotJson(ClientSnapshot snapshot, long stateVersion) {
        JsonObject payload = new JsonObject();
        payload.addProperty("state_version", stateVersion);
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
        long observedVersion;
        while (true) {
            observedVersion = client.stateVersion();
            try {
                lastSnapshot = executor.callOnClientThread(client::snapshot);
            } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
                throw error;
            } catch (Exception error) {
                lastSnapshot = unavailableSnapshot(error.getMessage());
            }
            if (lastSnapshot.inPlay()) {
                return snapshotJson(lastSnapshot, observedVersion);
            }
            long remainingNanos = deadlineNanos - System.nanoTime();
            if (remainingNanos <= 0L
                || !client.awaitStateChange(observedVersion, Duration.ofNanos(remainingNanos))) {
                return snapshotJson(lastSnapshot, observedVersion);
            }
        }
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
            String address = boundedString(payload, "server_addr", 512);
            int separator = address.lastIndexOf(':');
            if (separator <= 0 || separator == address.length() - 1) {
                throw new IllegalArgumentException("server_addr must be host:port");
            }
            return validatedServerAddress(
                address.substring(0, separator),
                address.substring(separator + 1)
            );
        }
        String host = payload.has("host") ? boundedString(payload, "host", 253) : "127.0.0.1";
        validateHost(host);
        int port = optionalBoundedInt(payload, "port", 1, 65_535, 25565);
        return new ServerAddress(host, port);
    }

    private static double timeoutSeconds(JsonObject payload) {
        return optionalBoundedDouble(payload, "timeout_seconds", 0.001, 3600.0, 30.0);
    }

    private static Duration eventTimeout(JsonObject payload) {
        double seconds = optionalBoundedDouble(payload, "timeout_seconds", 0.1, 120.0, 8.0);
        return Duration.ofNanos((long) (seconds * 1_000_000_000L));
    }

    private static Duration respawnTimeout(JsonObject payload) {
        double seconds = optionalBoundedDouble(payload, "timeout_seconds", 0.1, 120.0, 10.0);
        return Duration.ofNanos((long) (seconds * 1_000_000_000L));
    }

    private static int inputTicks(JsonObject payload) {
        return boundedInt(payload, "ticks", 1, 255);
    }

    private static ServerAddress validatedServerAddress(String host, String portText) {
        validateHost(host);
        final int port;
        try {
            port = Integer.parseInt(portText);
        } catch (NumberFormatException error) {
            throw new IllegalArgumentException("server port must be an integer", error);
        }
        if (port < 1 || port > 65_535) {
            throw new IllegalArgumentException("server port must be between 1 and 65535");
        }
        return new ServerAddress(host, port);
    }

    private static void validateHost(String host) {
        if (host.isBlank() || host.chars().anyMatch(Character::isWhitespace) || host.contains("/")) {
            throw new IllegalArgumentException("server host is invalid");
        }
    }

    private static BlockTarget blockTarget(JsonObject payload) {
        int x = boundedInt(payload, "x", Integer.MIN_VALUE, Integer.MAX_VALUE);
        int y = boundedInt(payload, "y", Integer.MIN_VALUE, Integer.MAX_VALUE);
        int z = boundedInt(payload, "z", Integer.MIN_VALUE, Integer.MAX_VALUE);
        String face = boundedString(payload, "face", 5);
        if (!BLOCK_FACES.contains(face)) {
            throw new IllegalArgumentException("face must be down, up, north, south, west, or east");
        }
        return new BlockTarget(x, y, z, face);
    }

    private static String entityInteractionHand(JsonObject payload) {
        String hand = payload.has("hand") ? boundedString(payload, "hand", 16) : "main_hand";
        if (!ENTITY_INTERACTION_HANDS.contains(hand)) {
            throw new IllegalArgumentException("hand must be main_hand or off_hand");
        }
        return hand;
    }

    private static String containerClickButton(JsonObject payload) {
        String button = boundedString(payload, "button", 9);
        if (!CONTAINER_CLICK_BUTTONS.contains(button)) {
            throw new IllegalArgumentException("button must be primary or secondary");
        }
        return button;
    }

    private static int boundedInt(JsonObject payload, String key, int minimum, int maximum) {
        if (!payload.has(key)
            || !payload.get(key).isJsonPrimitive()
            || !payload.get(key).getAsJsonPrimitive().isNumber()) {
            throw new IllegalArgumentException(key + " must be an integer");
        }
        final int value;
        try {
            value = payload.get(key).getAsBigDecimal().intValueExact();
        } catch (ArithmeticException | NumberFormatException | UnsupportedOperationException error) {
            throw new IllegalArgumentException(key + " must be an integer", error);
        }
        if (value < minimum || value > maximum) {
            throw new IllegalArgumentException(
                key + " must be between " + minimum + " and " + maximum
            );
        }
        return value;
    }

    private static long boundedLong(JsonObject payload, String key, long minimum, long maximum) {
        if (!payload.has(key)
            || !payload.get(key).isJsonPrimitive()
            || !payload.get(key).getAsJsonPrimitive().isNumber()) {
            throw new IllegalArgumentException(key + " must be an integer");
        }
        final long value;
        try {
            value = payload.get(key).getAsBigDecimal().longValueExact();
        } catch (ArithmeticException | NumberFormatException | UnsupportedOperationException error) {
            throw new IllegalArgumentException(key + " must be an integer", error);
        }
        if (value < minimum || value > maximum) {
            throw new IllegalArgumentException(
                key + " must be between " + minimum + " and " + maximum
            );
        }
        return value;
    }

    private static int optionalBoundedInt(
        JsonObject payload,
        String key,
        int minimum,
        int maximum,
        int defaultValue
    ) {
        return payload.has(key) ? boundedInt(payload, key, minimum, maximum) : defaultValue;
    }

    private static double optionalBoundedDouble(
        JsonObject payload,
        String key,
        double minimum,
        double maximum,
        double defaultValue
    ) {
        if (!payload.has(key)) {
            return defaultValue;
        }
        if (!payload.get(key).isJsonPrimitive()
            || !payload.get(key).getAsJsonPrimitive().isNumber()) {
            throw new IllegalArgumentException(key + " must be a number");
        }
        double value = payload.get(key).getAsDouble();
        if (!Double.isFinite(value) || value < minimum || value > maximum) {
            throw new IllegalArgumentException(
                key + " must be between " + minimum + " and " + maximum
            );
        }
        return value;
    }

    private static double boundedDouble(
        JsonObject payload,
        String key,
        double minimum,
        double maximum
    ) {
        if (!payload.has(key)) {
            throw new IllegalArgumentException(key + " must be a number");
        }
        return optionalBoundedDouble(payload, key, minimum, maximum, minimum);
    }

    private static void ensureBoundedBox(
        int minX,
        int minY,
        int minZ,
        int maxX,
        int maxY,
        int maxZ,
        int maxBlocks
    ) {
        if (minX > maxX || minY > maxY || minZ > maxZ) {
            throw new IllegalArgumentException("scan bounds must satisfy min <= max on every axis");
        }
        long sizeX = (long) maxX - minX + 1L;
        long sizeY = (long) maxY - minY + 1L;
        long sizeZ = (long) maxZ - minZ + 1L;
        if (sizeX > maxBlocks
            || sizeY > maxBlocks
            || sizeZ > maxBlocks
            || sizeX > maxBlocks / sizeY
            || sizeX * sizeY > maxBlocks / sizeZ) {
            throw new IllegalArgumentException("scan volume exceeds max_blocks=" + maxBlocks);
        }
    }

    private static List<String> inputKeys(JsonObject payload) {
        if (!payload.has("keys") || !payload.get("keys").isJsonArray()) {
            throw new IllegalArgumentException("keys must be an array");
        }
        JsonArray keys = payload.getAsJsonArray("keys");
        if (keys.isEmpty() || keys.size() > 8) {
            throw new IllegalArgumentException("keys must contain 1..8 unique inputs");
        }
        LinkedHashSet<String> inputs = new LinkedHashSet<>();
        keys.forEach(value -> {
            if (!value.isJsonPrimitive() || !value.getAsJsonPrimitive().isString()) {
                throw new IllegalArgumentException("each input key must be a string");
            }
            String input = value.getAsString();
            if (!ALLOWED_INPUTS.contains(input)) {
                throw new IllegalArgumentException("unsupported input key: " + input);
            }
            if (!inputs.add(input)) {
                throw new IllegalArgumentException("duplicate input key: " + input);
            }
        });
        return List.copyOf(inputs);
    }

    private static String boundedString(JsonObject payload, String key, int maximumLength) {
        if (!payload.has(key)
            || !payload.get(key).isJsonPrimitive()
            || !payload.get(key).getAsJsonPrimitive().isString()) {
            throw new IllegalArgumentException(key + " must be a string");
        }
        String value = payload.get(key).getAsString();
        if (value.isBlank() || value.length() > maximumLength) {
            throw new IllegalArgumentException(key + " must contain 1.." + maximumLength + " characters");
        }
        return value;
    }

    private static UUID boundedUuid(JsonObject payload, String key) {
        String value = boundedString(payload, key, 36);
        final UUID uuid;
        try {
            uuid = UUID.fromString(value);
        } catch (IllegalArgumentException error) {
            throw new IllegalArgumentException(key + " must be a UUID", error);
        }
        if (!uuid.toString().equalsIgnoreCase(value)) {
            throw new IllegalArgumentException(key + " must use canonical UUID form");
        }
        return uuid;
    }

    private static Path scenarioArtifactsDirectory(JsonObject payload) {
        if (payload.has("artifacts_dir")) {
            return Path.of(boundedString(payload, "artifacts_dir", 1024));
        }
        if (payload.has("screenshots_dir")) {
            return Path.of(boundedString(payload, "screenshots_dir", 1024));
        }
        return Path.of("run", "mcp-artifacts");
    }

    private static boolean optionalBoolean(JsonObject payload, String key, boolean defaultValue) {
        if (!payload.has(key)) {
            return defaultValue;
        }
        if (!payload.get(key).isJsonPrimitive()
            || !payload.get(key).getAsJsonPrimitive().isBoolean()) {
            throw new IllegalArgumentException(key + " must be a boolean");
        }
        return payload.get(key).getAsBoolean();
    }

    private static JsonObject ok() {
        JsonObject payload = new JsonObject();
        payload.addProperty("status", "ok");
        return payload;
    }

    private record ServerAddress(String host, int port) {
    }

    private record BlockTarget(int x, int y, int z, String face) {
    }
}
