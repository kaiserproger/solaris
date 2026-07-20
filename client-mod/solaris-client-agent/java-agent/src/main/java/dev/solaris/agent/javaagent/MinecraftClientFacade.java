package dev.solaris.agent.javaagent;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import dev.solaris.agent.client.ClientFacade;
import dev.solaris.agent.client.ClientScenarioReport;
import dev.solaris.agent.client.ClientSnapshot;
import net.minecraft.client.KeyMapping;
import net.minecraft.client.Minecraft;
import net.minecraft.client.Screenshot;
import net.minecraft.client.gui.screens.ConnectScreen;
import net.minecraft.client.multiplayer.ClientPacketListener;
import net.minecraft.client.multiplayer.ServerData;
import net.minecraft.client.multiplayer.TransferState;
import net.minecraft.client.multiplayer.resolver.ServerAddress;
import net.minecraft.commands.arguments.EntityAnchorArgument;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.network.Connection;
import net.minecraft.network.protocol.game.ServerboundMovePlayerPacket;
import net.minecraft.world.phys.Vec3;

import java.lang.reflect.Method;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.List;
import java.util.Map;
import java.util.TreeSet;
import java.util.UUID;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

public final class MinecraftClientFacade implements ClientFacade {
    private static final long MAX_TICK_WAIT_NANOS = 30_000_000_000L;

    @Override
    public ClientSnapshot snapshot() {
        Minecraft minecraft = Minecraft.getInstance();
        boolean inPlay = minecraft.player != null && minecraft.level != null;
        String dimension = minecraft.level == null
            ? ""
            : minecraft.level.dimension().identifier().toString();
        String screen = minecraft.screen == null ? "none" : minecraft.screen.getClass().getName();
        return new ClientSnapshot(
            inPlay,
            dimension,
            minecraft.player == null ? 0.0 : minecraft.player.getX(),
            minecraft.player == null ? 0.0 : minecraft.player.getY(),
            minecraft.player == null ? 0.0 : minecraft.player.getZ(),
            minecraft.player == null ? -1 : minecraft.player.getInventory().getSelectedSlot(),
            screen,
            ""
        );
    }

    @Override
    public long stateVersion() {
        return ClientStateEvents.version();
    }

    @Override
    public boolean awaitStateChange(long observedVersion, java.time.Duration timeout)
        throws InterruptedException {
        return ClientStateEvents.awaitChange(observedVersion, timeout);
    }

    @Override
    public JsonObject observe() {
        return MinecraftClientObservation.observe(Minecraft.getInstance());
    }

    @Override
    public JsonObject readBlock(int x, int y, int z) {
        return MinecraftClientObservation.readBlock(requireInPlay(), new BlockPos(x, y, z));
    }

    @Override
    public JsonObject waitForLoadedBlock(
        int x,
        int y,
        int z,
        java.time.Duration timeout
    ) throws Exception {
        MinecraftClientExecutor executor = new MinecraftClientExecutor();
        BlockPos position = new BlockPos(x, y, z);
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        while (true) {
            long observedVersion = ClientStateEvents.version();
            JsonObject block = executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                if (!minecraft.level.hasChunk(position.getX() >> 4, position.getZ() >> 4)) {
                    return null;
                }
                return MinecraftClientObservation.readBlock(minecraft, position);
            });
            if (block != null) {
                return block;
            }
            long remainingNanos = deadlineNanos - System.nanoTime();
            if (remainingNanos <= 0L || !ClientStateEvents.awaitChange(
                observedVersion,
                java.time.Duration.ofNanos(remainingNanos)
            )) {
                throw new IllegalStateException(
                    "client chunk did not load at " + x + ", " + y + ", " + z
                );
            }
        }
    }

    @Override
    public JsonObject scanBlocks(
        int minX,
        int minY,
        int minZ,
        int maxX,
        int maxY,
        int maxZ,
        int maxBlocks
    ) {
        return MinecraftClientObservation.scanBlocks(
            requireInPlay(),
            minX,
            minY,
            minZ,
            maxX,
            maxY,
            maxZ,
            maxBlocks
        );
    }

    @Override
    public JsonObject listEntities(double radius, int limit) {
        return MinecraftClientObservation.listEntities(requireInPlay(), radius, limit);
    }

    @Override
    public JsonObject readRecipeBook(int limit) {
        Minecraft minecraft = requireInPlay();
        var collections = minecraft.player.getRecipeBook().getCollections();
        TreeSet<Integer> displayIds = new TreeSet<>();
        for (var collection : collections) {
            for (var entry : collection.getRecipes()) {
                displayIds.add(entry.id().index());
            }
        }

        JsonArray listedIds = new JsonArray();
        int listed = 0;
        for (int displayId : displayIds) {
            if (listed == limit) {
                break;
            }
            listedIds.add(displayId);
            listed += 1;
        }
        int contiguousPrefix = 0;
        while (displayIds.contains(contiguousPrefix)) {
            contiguousPrefix += 1;
        }

        JsonObject result = new JsonObject();
        result.addProperty("collection_count", collections.size());
        result.addProperty("entry_count", displayIds.size());
        result.addProperty("listed_count", listed);
        result.addProperty("truncated", listed < displayIds.size());
        result.addProperty("contiguous_prefix_count", contiguousPrefix);
        result.addProperty("min_display_id", displayIds.isEmpty() ? -1 : displayIds.first());
        result.addProperty("max_display_id", displayIds.isEmpty() ? -1 : displayIds.last());
        result.add("display_ids", listedIds);
        return result;
    }

    @Override
    public JsonObject waitForVisibleEntity(
        String entityType,
        double radius,
        java.time.Duration timeout
    ) throws Exception {
        MinecraftClientExecutor executor = new MinecraftClientExecutor();
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        do {
            long observedVersion = ClientStateEvents.version();
            JsonObject entity = executor.callOnClientThread(
                () -> visibleEntityOnClientThread(entityType, radius)
            );
            if (entity != null) {
                entity.addProperty("matched", true);
                return entity;
            }
            long remainingNanos = deadlineNanos - System.nanoTime();
            if (remainingNanos <= 0L || !ClientStateEvents.awaitChange(
                observedVersion,
                java.time.Duration.ofNanos(remainingNanos)
            )) {
                break;
            }
        } while (true);
        throw new IllegalStateException(
            "entity did not become visible type=" + entityType + " radius=" + radius
        );
    }

    @Override
    public JsonObject waitForEntityMotion(
        int entityId,
        UUID entityUuid,
        String entityType,
        double minimumHorizontalDistance,
        double minimumVerticalRise,
        Duration timeout
    ) throws Exception {
        MinecraftScenarioClient client = new MinecraftScenarioClient(new MinecraftClientExecutor());
        ScenarioEntityMotionObservation motion = client.waitForEntityMotion(
            entityId,
            entityUuid,
            entityType,
            minimumHorizontalDistance,
            minimumVerticalRise,
            timeout
        );
        if (motion == null) {
            throw new IllegalStateException(
                "entity identity left client-visible state before motion matched id=" + entityId
                    + " uuid=" + entityUuid + " type=" + entityType
            );
        }
        if (
            motion.horizontalDistance() < minimumHorizontalDistance
                || motion.verticalRise() < minimumVerticalRise
        ) {
            throw new TimeoutException(
                "entity motion did not match before timeout id=" + entityId
                    + " horizontal_distance=" + motion.horizontalDistance()
                    + " vertical_rise=" + motion.verticalRise()
            );
        }

        JsonObject result = entityIdentity(entityId, entityUuid, entityType);
        result.addProperty("matched", true);
        result.addProperty("end_x", motion.endX());
        result.addProperty("end_y", motion.endY());
        result.addProperty("end_z", motion.endZ());
        result.addProperty("horizontal_distance", motion.horizontalDistance());
        result.addProperty("vertical_rise", motion.verticalRise());
        result.addProperty("max_horizontal_speed", motion.maxHorizontalSpeed());
        result.addProperty("minimum_yaw_delta", motion.minimumYawDelta());
        return result;
    }

    @Override
    public JsonObject waitForEntityRemoved(
        int entityId,
        UUID entityUuid,
        String entityType,
        Duration timeout
    ) throws Exception {
        MinecraftScenarioClient client = new MinecraftScenarioClient(new MinecraftClientExecutor());
        if (!client.waitForEntityRemoved(entityId, entityUuid, entityType, timeout)) {
            throw new TimeoutException(
                "entity remained client-visible before timeout id=" + entityId
                    + " uuid=" + entityUuid + " type=" + entityType
            );
        }
        JsonObject result = entityIdentity(entityId, entityUuid, entityType);
        result.addProperty("removed", true);
        return result;
    }

    private static JsonObject entityIdentity(int entityId, UUID entityUuid, String entityType) {
        JsonObject result = new JsonObject();
        result.addProperty("entity_id", entityId);
        result.addProperty("entity_uuid", entityUuid.toString());
        result.addProperty("entity_type", entityType);
        return result;
    }

    @Override
    public JsonObject waitForHealthBelow(double health, java.time.Duration timeout) throws Exception {
        MinecraftScenarioClient client = new MinecraftScenarioClient(new MinecraftClientExecutor());
        float observedHealth = client.waitForPlayerHealthBelow((float) health, timeout);
        if (observedHealth >= health - 0.001) {
            throw new IllegalStateException(
                "player health did not fall below " + health + " latest=" + observedHealth
            );
        }
        JsonObject result = new JsonObject();
        result.addProperty("matched", true);
        result.addProperty("health", observedHealth);
        result.addProperty("below", health);
        return result;
    }

    @Override
    public JsonObject waitForInventoryCount(
        String itemId,
        int count,
        java.time.Duration timeout
    ) throws Exception {
        MinecraftScenarioClient client = new MinecraftScenarioClient(new MinecraftClientExecutor());
        if (!client.waitForInventoryCount(itemId, count, timeout)) {
            throw new IllegalStateException(
                "inventory count did not reach " + count + " for " + itemId
            );
        }
        JsonObject result = new JsonObject();
        result.addProperty("matched", true);
        result.addProperty("item_id", itemId);
        result.addProperty("count", count);
        return result;
    }

    private static JsonObject visibleEntityOnClientThread(String entityType, double radius) {
        JsonObject visible = MinecraftClientObservation.listEntities(
            requireInPlay(),
            radius,
            Integer.MAX_VALUE
        );
        for (var element : visible.getAsJsonArray("entities")) {
            JsonObject entity = element.getAsJsonObject();
            if (entityType.equals(entity.get("entity_type").getAsString())) {
                return entity;
            }
        }
        return null;
    }

    @Override
    public JsonObject waitForVisibleItem(
        String itemId,
        int x,
        int y,
        int z,
        java.time.Duration timeout
    ) throws Exception {
        return waitForItemVisibility(itemId, x, y, z, timeout, true);
    }

    @Override
    public JsonObject waitForNoVisibleItem(
        String itemId,
        int x,
        int y,
        int z,
        java.time.Duration timeout
    ) throws Exception {
        return waitForItemVisibility(itemId, x, y, z, timeout, false);
    }

    @Override
    public void connect(String host, int port) {
        Minecraft minecraft = Minecraft.getInstance();
        String address = host + ":" + port;
        ServerData serverData = new ServerData("Solaris", address, ServerData.Type.OTHER);
        TransferState transferState = new TransferState(Map.of(), Map.of(), false);
        ConnectScreen.startConnecting(
            minecraft.screen,
            minecraft,
            ServerAddress.parseString(address),
            serverData,
            false,
            transferState
        );
    }

    @Override
    public void selectHotbarSlot(int slot) {
        MinecraftScenarioClient.selectHotbarSlotOnClientThread(slot);
    }

    @Override
    public JsonObject selectHotbarItem(String itemId, int count, java.time.Duration timeout)
        throws Exception {
        MinecraftScenarioClient client = new MinecraftScenarioClient(new MinecraftClientExecutor());
        ScenarioHeldItem selected = client.selectHotbarItem(itemId, count, timeout);
        if (!selected.matches(itemId, count)) {
            throw new IllegalStateException(
                "item was not selected expected=" + itemId + " x" + count
                    + " actual=" + selected.itemId() + " x" + selected.count()
            );
        }
        JsonObject result = new JsonObject();
        result.addProperty("selected", true);
        result.addProperty("item_id", selected.itemId());
        result.addProperty("count", selected.count());
        return result;
    }

    @Override
    public JsonObject navigateToBlock(int x, int y, int z, java.time.Duration timeout) throws Exception {
        MinecraftClientExecutor executor = new MinecraftClientExecutor();
        NavigationTarget target = executor.callOnClientThread(
            () -> navigationTargetOnClientThread(x, y, z)
        );
        BlockNavigation.Result navigation = BlockNavigation.run(
            target.route(),
            timeout,
            new BlockNavigation.Runtime() {
                @Override
                public long nanoTime() {
                    return System.nanoTime();
                }

                @Override
                public long tickVersion() {
                    return ClientStateEvents.tickVersion();
                }

                @Override
                public BlockNavigation.Observation observe() throws Exception {
                    return executor.callOnClientThread(
                        () -> navigationObservationOnClientThread(target)
                    );
                }

                @Override
                public void apply(BlockNavigation.Inputs inputs) throws Exception {
                    executor.callOnClientThread(() -> {
                        applyNavigationInputsOnClientThread(inputs);
                        return null;
                    });
                }

                @Override
                public boolean awaitTickChange(long observedVersion, java.time.Duration remaining)
                    throws InterruptedException {
                    return ClientStateEvents.awaitTickChange(observedVersion, remaining);
                }

                @Override
                public void clearInputs() throws Exception {
                    executor.callOnClientThread(() -> {
                        clearNavigationInputsOnClientThread();
                        return null;
                    });
                }
            }
        );
        BlockNavigation.Observation observation = navigation.observation();
        switch (navigation.terminal()) {
            case ARRIVED -> {
                JsonObject result = new JsonObject();
                result.addProperty("arrived", true);
                result.addProperty("x", target.position().getX());
                result.addProperty("y", target.position().getY());
                result.addProperty("z", target.position().getZ());
                result.addProperty("player_x", observation.playerX());
                result.addProperty("player_y", observation.playerY());
                result.addProperty("player_z", observation.playerZ());
                return result;
            }
            case TARGET_UNLOADED -> throw new IllegalStateException(
                "target chunk became unloaded during navigation: "
                    + target.position().getX() + ", " + target.position().getY() + ", "
                    + target.position().getZ()
            );
            case UNREACHABLE -> throw new IllegalStateException(
                "target is unreachable from the current loaded client position: "
                    + target.position().getX() + ", "
                    + target.position().getY() + ", "
                    + target.position().getZ()
            );
            case INVALID_OBSERVATION -> throw new IllegalStateException(
                "client produced an invalid navigation observation"
            );
            case TIMED_OUT -> throw new TimeoutException(
                "navigation timed out before arrival at "
                    + target.position().getX() + ", "
                    + target.position().getY() + ", "
                    + target.position().getZ()
            );
        }
        throw new IllegalStateException("unknown navigation result: " + navigation.terminal());
    }

    @Override
    public JsonObject approachEntity(int entityId, java.time.Duration timeout) throws Exception {
        MinecraftClientExecutor executor = new MinecraftClientExecutor();
        ScenarioEntityObservation entity = executor.callOnClientThread(
            () -> entityObservationOnClientThread(entityId)
        );
        MinecraftScenarioClient client = new MinecraftScenarioClient(executor);
        boolean inReach = client.approachEntity(entity, timeout);
        JsonObject result = new JsonObject();
        result.addProperty("entity_id", entityId);
        result.addProperty("entity_uuid", entity.entityUuid().toString());
        result.addProperty("entity_type", entity.entityType());
        result.addProperty("in_reach", inReach);
        return result;
    }

    @Override
    public JsonObject interactEntity(
        int entityId,
        UUID entityUuid,
        String entityType,
        String hand
    ) throws Exception {
        return interactEntity(
            new MinecraftScenarioClient(new MinecraftClientExecutor()),
            new ScenarioEntityInteraction(
                new ScenarioEntityIdentity(entityId, entityUuid, entityType),
                hand
            )
        );
    }

    static JsonObject interactEntity(ScenarioClient client, ScenarioEntityInteraction interaction)
        throws Exception {
        ScenarioEntityInteractionResult observed = client.interactEntity(interaction);
        JsonObject result = new JsonObject();
        result.addProperty("entity_id", interaction.identity().entityId());
        result.addProperty("entity_uuid", interaction.identity().entityUuid().toString());
        result.addProperty("entity_type", interaction.identity().entityType());
        result.addProperty("hand", interaction.hand());
        result.addProperty("dispatched", true);
        result.addProperty("result", observed.result());
        result.addProperty("consumes_action", observed.consumesAction());
        result.addProperty("hit_x", observed.hitX());
        result.addProperty("hit_y", observed.hitY());
        result.addProperty("hit_z", observed.hitZ());
        return result;
    }

    @Override
    public JsonObject attackEntityUntilDropCollected(
        int entityId,
        String expectedDropItemId,
        int expectedDropCount,
        java.time.Duration timeout
    ) throws Exception {
        MinecraftClientExecutor executor = new MinecraftClientExecutor();
        ScenarioEntityObservation entity = executor.callOnClientThread(
            () -> entityObservationOnClientThread(entityId)
        );
        MinecraftScenarioClient client = new MinecraftScenarioClient(executor);
        ScenarioBreakResult attack = client.attackEntityUntilDropCollected(
            entity,
            expectedDropItemId,
            expectedDropCount,
            timeout
        );
        JsonObject result = new JsonObject();
        result.addProperty("entity_id", entityId);
        result.addProperty("entity_uuid", entity.entityUuid().toString());
        result.addProperty("entity_type", entity.entityType());
        result.addProperty("expected_drop_item_id", expectedDropItemId);
        result.addProperty("expected_drop_count", expectedDropCount);
        result.addProperty("attack_started", attack.started());
        result.addProperty("entity_removed", attack.becameAir());
        result.addProperty("saw_drop", attack.sawDrop());
        result.addProperty("pickup_restored", attack.pickupRestored());
        result.addProperty("selected_item_id", attack.selectedItem().itemId());
        result.addProperty("selected_item_count", attack.selectedItem().count());
        return result;
    }

    private static ScenarioEntityObservation entityObservationOnClientThread(int entityId) {
        Minecraft minecraft = requireInPlay();
        var entity = minecraft.level.getEntity(entityId);
        if (entity == null) {
            throw new IllegalStateException("entity is not client-visible: " + entityId);
        }
        double distanceSquared = minecraft.player.position().distanceToSqr(entity.position());
        return new ScenarioEntityObservation(
            BuiltInRegistries.ENTITY_TYPE.getKey(entity.getType()).toString(),
            entityId,
            entity.getUUID(),
            entity.getX(),
            entity.getY(),
            entity.getZ(),
            distanceSquared,
            null
        );
    }

    private static NavigationTarget navigationTargetOnClientThread(int x, int y, int z) {
        Minecraft minecraft = requireInPlay();
        int minY = minecraft.level.dimensionType().minY();
        int maxY = minY + minecraft.level.dimensionType().height();
        if (y < minY || y >= maxY) {
            throw new IllegalArgumentException("target y is outside the client world height: " + y);
        }
        BlockPos position = new BlockPos(x, y, z);
        if (!minecraft.level.hasChunk(position.getX() >> 4, position.getZ() >> 4)) {
            throw new IllegalStateException(
                "target chunk is not client-loaded: " + x + ", " + y + ", " + z
            );
        }
        if (!BlockNavigation.withinBounds(
            minecraft.player.getX(),
            minecraft.player.getY(),
            minecraft.player.getZ(),
            x,
            y,
            z
        )) {
            throw new IllegalArgumentException(
                "target exceeds bounded navigation range of "
                    + BlockNavigation.MAX_HORIZONTAL_DISTANCE + " horizontal blocks and "
                    + BlockNavigation.MAX_VERTICAL_DISTANCE + " vertical blocks"
            );
        }
        return new NavigationTarget(
            position,
            new BlockNavigation.Route(
                minecraft.player.getX(),
                minecraft.player.getY(),
                minecraft.player.getZ(),
                x,
                y,
                z
            )
        );
    }

    private static BlockNavigation.Observation navigationObservationOnClientThread(
        NavigationTarget target
    ) {
        Minecraft minecraft = requireInPlay();
        BlockPos targetPosition = target.position();
        if (!minecraft.level.hasChunk(targetPosition.getX() >> 4, targetPosition.getZ() >> 4)) {
            return new BlockNavigation.Observation(
                minecraft.player.getX(),
                minecraft.player.getY(),
                minecraft.player.getZ(),
                minecraft.player.onGround(),
                minecraft.level.noCollision(minecraft.player, minecraft.player.getBoundingBox()),
                false,
                minecraft.player.horizontalCollision,
                null
            );
        }
        if (minecraft.screen != null) {
            minecraft.player.closeContainer();
            minecraft.setScreen(null);
        }
        boolean collisionFree = minecraft.level.noCollision(
            minecraft.player,
            minecraft.player.getBoundingBox()
        );
        Vec3 center = Vec3.atLowerCornerWithOffset(targetPosition, 0.5, 0.5, 0.5);
        minecraft.player.lookAt(EntityAnchorArgument.Anchor.EYES, center);
        MovementClearance clearance = MovementDetour.clearance(minecraft, center);
        return new BlockNavigation.Observation(
            minecraft.player.getX(),
            minecraft.player.getY(),
            minecraft.player.getZ(),
            minecraft.player.onGround(),
            collisionFree,
            true,
            minecraft.player.horizontalCollision,
            clearance
        );
    }

    private static void applyNavigationInputsOnClientThread(BlockNavigation.Inputs inputs) {
        Minecraft minecraft = requireInPlay();
        minecraft.options.keySprint.setDown(inputs.sprint());
        minecraft.options.keyUp.setDown(inputs.forward());
        minecraft.options.keyJump.setDown(inputs.jump());
        minecraft.options.keyLeft.setDown(inputs.left());
        minecraft.options.keyRight.setDown(inputs.right());
    }

    private static void clearNavigationInputsOnClientThread() {
        Minecraft minecraft = Minecraft.getInstance();
        if (minecraft.options == null) {
            return;
        }
        minecraft.options.keyUp.setDown(false);
        minecraft.options.keySprint.setDown(false);
        minecraft.options.keyJump.setDown(false);
        minecraft.options.keyLeft.setDown(false);
        minecraft.options.keyRight.setDown(false);
    }

    @Override
    public void lookAtBlock(int x, int y, int z, String face) {
        MinecraftScenarioClient.lookAtBlockOnClientThread(
            new ScenarioBlockTarget(x, y, z, face, "manual-command", blockIdAt(x, y, z))
        );
    }

    @Override
    public void useItemOn(int x, int y, int z, String face) {
        MinecraftScenarioClient.useItemOnClientThread(
            new ScenarioBlockTarget(x, y, z, face, "manual-command", blockIdAt(x, y, z))
        );
    }

    @Override
    public void moveForward(int ticks) throws Exception {
        moveWithKey(ticks, true);
    }

    @Override
    public void moveBackward(int ticks) throws Exception {
        moveWithKey(ticks, false);
    }

    @Override
    public void pressInputs(List<String> inputs, int ticks) throws Exception {
        MinecraftClientExecutor executor = new MinecraftClientExecutor();
        try {
            executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                for (String input : inputs) {
                    setInput(minecraft, input, true);
                }
                return null;
            });
            waitTicks(ticks);
        } finally {
            executor.callOnClientThread(() -> {
                Minecraft minecraft = Minecraft.getInstance();
                for (String input : inputs) {
                    setInput(minecraft, input, false);
                }
                return null;
            });
        }
    }

    private void moveWithKey(int ticks, boolean forward) throws Exception {
        if (ticks <= 0 || ticks > 255) {
            throw new IllegalArgumentException("ticks must be 1..255");
        }
        MinecraftClientExecutor executor = new MinecraftClientExecutor();
        executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            if (forward) {
                minecraft.options.keySprint.setDown(true);
                minecraft.options.keyUp.setDown(true);
            } else {
                minecraft.options.keyDown.setDown(true);
            }
            return null;
        });
        try {
            waitTicks(ticks);
        } finally {
            executor.callOnClientThread(() -> {
                Minecraft minecraft = Minecraft.getInstance();
                if (forward) {
                    minecraft.options.keyUp.setDown(false);
                    minecraft.options.keySprint.setDown(false);
                } else {
                    minecraft.options.keyDown.setDown(false);
                }
                return null;
            });
        }
    }

    @Override
    public void waitTicks(int ticks) throws Exception {
        if (ticks <= 0 || ticks > 255) {
            throw new IllegalArgumentException("ticks must be 1..255");
        }
        MinecraftClientExecutor executor = new MinecraftClientExecutor();
        int startTick = executor.callOnClientThread(() -> requireInPlay().player.tickCount);
        long deadlineNanos = System.nanoTime() + MAX_TICK_WAIT_NANOS;
        while (true) {
            long observedVersion = ClientStateEvents.tickVersion();
            int currentTick = executor.callOnClientThread(() -> requireInPlay().player.tickCount);
            if (Integer.toUnsignedLong(currentTick - startTick) >= ticks) {
                return;
            }
            long remainingNanos = deadlineNanos - System.nanoTime();
            if (remainingNanos <= 0L
                || !ClientStateEvents.awaitTickChange(
                    observedVersion,
                    java.time.Duration.ofNanos(remainingNanos)
                )) {
                throw new IllegalStateException(
                    "client did not advance " + ticks + " ticks within 30 seconds"
                );
            }
        }
    }

    @Override
    public void moveByCentimeters(int dxCm, int dzCm) {
        Minecraft minecraft = requireInPlay();
        double x = minecraft.player.getX() + dxCm / 100.0;
        double y = minecraft.player.getY();
        double z = minecraft.player.getZ() + dzCm / 100.0;
        minecraft.player.setPos(x, y, z);
        minecraft.getConnection().send(
            new ServerboundMovePlayerPacket.Pos(
                x,
                y,
                z,
                minecraft.player.onGround(),
                minecraft.player.horizontalCollision
            )
        );
    }

    @Override
    public void look(int yawDeg, int pitchDeg) {
        Minecraft minecraft = requireInPlay();
        minecraft.player.setYRot(yawDeg);
        minecraft.player.setXRot(pitchDeg);
        minecraft.getConnection().send(
            new ServerboundMovePlayerPacket.Rot(
                yawDeg,
                pitchDeg,
                minecraft.player.onGround(),
                minecraft.player.horizontalCollision
            )
        );
    }

    @Override
    public void closeCurrentScreen() {
        Minecraft minecraft = requireInPlay();
        MinecraftScenarioClient.closeCurrentScreenOnClientThread(minecraft);
    }

    @Override
    public void respawn(Duration timeout) throws Exception {
        respawn(new MinecraftScenarioClient(new MinecraftClientExecutor()), timeout);
    }

    static void respawn(ScenarioClient client, Duration timeout) throws Exception {
        boolean respawned = client.performRespawn(timeout);
        if (!respawned) {
            throw new TimeoutException("client respawn was not confirmed");
        }
    }

    @Override
    public JsonObject quickMoveContainerSlot(int slot, java.time.Duration timeout) throws Exception {
        boolean confirmed = new MinecraftScenarioClient(new MinecraftClientExecutor())
            .quickMoveContainerSlot(slot, timeout);
        if (!confirmed) {
            throw new TimeoutException("server did not confirm quick move for container slot " + slot);
        }
        JsonObject result = new JsonObject();
        result.addProperty("confirmed", true);
        result.addProperty("slot", slot);
        return result;
    }

    @Override
    public JsonObject clickContainerSlot(int slot, String button, java.time.Duration timeout) throws Exception {
        boolean confirmed = new MinecraftScenarioClient(new MinecraftClientExecutor())
            .clickContainerSlot(slot, button, timeout);
        if (!confirmed) {
            throw new TimeoutException("server did not confirm " + button + " click for container slot " + slot);
        }
        JsonObject result = new JsonObject();
        result.addProperty("confirmed", true);
        result.addProperty("slot", slot);
        result.addProperty("button", button);
        return result;
    }

    @Override
    public JsonObject clickContainerButton(int buttonId, java.time.Duration timeout) throws Exception {
        boolean confirmed = new MinecraftScenarioClient(new MinecraftClientExecutor())
            .clickContainerButton(buttonId, timeout);
        if (!confirmed) {
            throw new TimeoutException("server did not confirm container button " + buttonId);
        }
        JsonObject result = new JsonObject();
        result.addProperty("confirmed", true);
        result.addProperty("button_id", buttonId);
        return result;
    }

    @Override
    public void sendChat(String message, boolean command) {
        Minecraft minecraft = requireInPlay();
        if (command) {
            String normalized = message.startsWith("/") ? message.substring(1) : message;
            if (normalized.isBlank()) {
                throw new IllegalArgumentException("command must not be blank");
            }
            minecraft.getConnection().sendCommand(normalized);
        } else {
            minecraft.getConnection().sendChat(message);
        }
    }

    @Override
    public JsonObject dropSelectedItem(String itemId, int count, java.time.Duration timeout)
        throws Exception {
        MinecraftScenarioClient client = new MinecraftScenarioClient(new MinecraftClientExecutor());
        int before = client.inventoryCount(itemId);
        ScenarioBlockTarget target = client.dropSelectedItem(itemId, count, timeout);
        int after = client.inventoryCount(itemId);
        int expectedAfter = Math.max(0, before - count);
        if (after > expectedAfter) {
            throw new IllegalStateException(
                "selected item debit was not confirmed expected_at_most="
                    + expectedAfter
                    + " actual="
                    + after
            );
        }
        if (!client.waitForVisibleItemDrop(itemId, target, timeout)) {
            throw new IllegalStateException("dropped item entity did not become visible: " + itemId);
        }

        JsonObject result = new JsonObject();
        result.addProperty("status", "confirmed");
        result.addProperty("item_id", itemId);
        result.addProperty("count", count);
        result.addProperty("inventory_before", before);
        result.addProperty("inventory_after", after);
        result.addProperty("visible", true);
        result.addProperty("x", target.x());
        result.addProperty("y", target.y());
        result.addProperty("z", target.z());
        return result;
    }

    private static JsonObject waitForItemVisibility(
        String itemId,
        int x,
        int y,
        int z,
        java.time.Duration timeout,
        boolean visible
    ) throws Exception {
        MinecraftScenarioClient client = new MinecraftScenarioClient(new MinecraftClientExecutor());
        ScenarioBlockTarget target = new ScenarioBlockTarget(x, y, z, "up", "mcp-item-wait", itemId);
        boolean matched = visible
            ? client.waitForVisibleItemDrop(itemId, target, timeout)
            : client.waitForNoVisibleItemDrop(itemId, target, timeout);
        if (!matched) {
            throw new IllegalStateException(
                "item visibility did not become " + visible + " near " + x + "," + y + "," + z
            );
        }
        JsonObject result = new JsonObject();
        result.addProperty("matched", true);
        result.addProperty("item_id", itemId);
        result.addProperty("visible", visible);
        result.addProperty("x", x);
        result.addProperty("y", y);
        result.addProperty("z", z);
        return result;
    }

    @Override
    public Path screenshot(Path path) throws Exception {
        Path directory = screenshotBaseDirectory(path);
        Files.createDirectories(directory.resolve("screenshots"));
        CompletableFuture<String> completion = new CompletableFuture<>();
        new MinecraftClientExecutor().callOnClientThread(() -> {
            Minecraft minecraft = Minecraft.getInstance();
            Screenshot.grab(
                directory.toFile(),
                path.getFileName().toString(),
                minecraft.getMainRenderTarget(),
                1,
                message -> completion.complete(message.getString())
            );
            return null;
        });
        final String resultMessage;
        try {
            resultMessage = completion.get(30, TimeUnit.SECONDS);
        } catch (InterruptedException error) {
            Thread.currentThread().interrupt();
            throw error;
        } catch (ExecutionException error) {
            throw new IllegalStateException("screenshot write failed", error.getCause());
        } catch (TimeoutException error) {
            throw new IllegalStateException("screenshot did not finish within 30 seconds", error);
        }
        if (!Files.isRegularFile(path)) {
            throw new IllegalStateException("screenshot was not written: " + resultMessage);
        }
        return path;
    }

    static Path screenshotBaseDirectory(Path path) {
        Path directory = path.getParent();
        if (directory == null) {
            throw new IllegalArgumentException("screenshot path must be inside a screenshots directory");
        }
        Path directoryName = directory.getFileName();
        if (directoryName == null || !"screenshots".equals(directoryName.toString())) {
            throw new IllegalArgumentException("screenshot path must be inside a screenshots directory");
        }
        Path baseDirectory = directory.getParent();
        return baseDirectory == null ? Path.of(".") : baseDirectory;
    }

    @Override
    public ClientScenarioReport runScenario(String id, Path screenshotsDir) {
        ScenarioClient client = new MinecraftScenarioClient(new MinecraftClientExecutor());
        if (PlayableBuildingPlacementScenario.supports(id)) {
            return new PlayableBuildingPlacementScenario().run(id, screenshotsDir, client);
        }
        if (PlayableRealClientLoopScenario.supports(id)) {
            return new PlayableRealClientLoopScenario().run(id, screenshotsDir, client);
        }
        if (M94BlocksFluidsFarmingDropsScenario.ID.equals(id)) {
            return new M94BlocksFluidsFarmingDropsScenario().run(id, screenshotsDir, client);
        }
        if (M94SolidBlockScenario.ID.equals(id)) {
            return new M94SolidBlockScenario().run(id, screenshotsDir, client);
        }
        if (M94WaterBucketScenario.ID.equals(id)) {
            return new M94WaterBucketScenario().run(id, screenshotsDir, client);
        }
        if (M94SignsBedsCampfiresScenario.ID.equals(id)) {
            return new M94SignsBedsCampfiresScenario().run(id, screenshotsDir, client);
        }
        if (M94EntitiesCombatDeathRespawnScenario.ID.equals(id)) {
            return new M94EntitiesCombatDeathRespawnScenario().run(id, screenshotsDir, client);
        }
        if (M94SaveRestartVisibilityScenario.supports(id)) {
            return new M94SaveRestartVisibilityScenario().run(id, screenshotsDir, client);
        }
        if (M94M40M41RouteScenario.ID.equals(id)) {
            return new M94M40M41RouteScenario().run(id, screenshotsDir, client);
        }
        if (M94SignScenario.ID.equals(id)) {
            return new M94SignScenario().run(id, screenshotsDir, client);
        }
        if (M94InventoryCraftingScenario.supports(id)) {
            return new M94InventoryCraftingScenario().run(id, screenshotsDir, client);
        }
        if (M94EnchantingScenario.ID.equals(id)) {
            return new M94EnchantingScenario().run(id, screenshotsDir, client);
        }
        return new M94RejectedBlockScenario().run(id, screenshotsDir, client);
    }

    @Override
    public void disconnect() {
        ClientStateEvents.clearItemTakenEvents();
        Minecraft minecraft = Minecraft.getInstance();
        ClientPacketListener listener = minecraft.getConnection();
        DisconnectSequence.run(
            () -> closeNetworkConnection(listener),
            minecraft::disconnectWithProgressScreen
        );
    }

    private static void closeNetworkConnection(ClientPacketListener listener) {
        if (listener == null) {
            return;
        }
        Connection connection = listener.getConnection();
        if (connection != null) {
            try {
                Class<?> componentType = Class.forName("net.minecraft.network.chat.Component");
                Object reason = componentType
                    .getMethod("literal", String.class)
                    .invoke(null, "Solaris real-client agent disconnect");
                Method disconnect = Connection.class.getMethod("disconnect", componentType);
                disconnect.invoke(connection, reason);
            } catch (ReflectiveOperationException error) {
                throw new IllegalStateException("failed to close client network connection", error);
            }
        }
    }

    private static String blockIdAt(int x, int y, int z) {
        Minecraft minecraft = Minecraft.getInstance();
        if (minecraft.level == null) {
            return "minecraft:air";
        }
        return BuiltInRegistries.BLOCK
            .getKey(minecraft.level.getBlockState(new BlockPos(x, y, z)).getBlock())
            .toString();
    }

    private static void setInput(Minecraft minecraft, String input, boolean down) {
        switch (input) {
            case "forward" -> minecraft.options.keyUp.setDown(down);
            case "back" -> minecraft.options.keyDown.setDown(down);
            case "left" -> minecraft.options.keyLeft.setDown(down);
            case "right" -> minecraft.options.keyRight.setDown(down);
            case "jump" -> minecraft.options.keyJump.setDown(down);
            case "sneak" -> minecraft.options.keyShift.setDown(down);
            case "sprint" -> minecraft.options.keySprint.setDown(down);
            case "attack" -> minecraft.options.keyAttack.setDown(down);
            case "use" -> minecraft.options.keyUse.setDown(down);
            case "swap_offhand" -> {
                KeyMapping mapping = minecraft.options.keySwapOffhand;
                KeyMapping.set(mapping.getDefaultKey(), down);
                if (down) {
                    KeyMapping.click(mapping.getDefaultKey());
                }
            }
            default -> throw new IllegalArgumentException("unsupported input key: " + input);
        }
    }

    private record NavigationTarget(BlockPos position, BlockNavigation.Route route) {
    }

    private static Minecraft requireInPlay() {
        Minecraft minecraft = Minecraft.getInstance();
        if (minecraft.player == null || minecraft.level == null || minecraft.gameMode == null) {
            throw new IllegalStateException("client is not in play");
        }
        return minecraft;
    }
}
