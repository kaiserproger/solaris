package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientFacade;
import dev.solaris.agent.client.ClientScenarioReport;
import dev.solaris.agent.client.ClientSnapshot;
import net.minecraft.client.Minecraft;
import net.minecraft.client.Screenshot;
import net.minecraft.client.gui.screens.ConnectScreen;
import net.minecraft.client.multiplayer.ClientPacketListener;
import net.minecraft.client.multiplayer.ServerData;
import net.minecraft.client.multiplayer.TransferState;
import net.minecraft.client.multiplayer.resolver.ServerAddress;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.network.Connection;

import java.lang.reflect.Method;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;

public final class MinecraftClientFacade implements ClientFacade {
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
    public void moveForward(int durationMillis) throws Exception {
        if (durationMillis <= 0 || durationMillis > 5_000) {
            throw new IllegalArgumentException("durationMillis must be 1..5000");
        }
        MinecraftClientExecutor executor = new MinecraftClientExecutor();
        executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            minecraft.options.keySprint.setDown(true);
            minecraft.options.keyUp.setDown(true);
            return null;
        });
        try {
            Thread.sleep(durationMillis);
        } finally {
            executor.callOnClientThread(() -> {
                Minecraft minecraft = Minecraft.getInstance();
                minecraft.options.keyUp.setDown(false);
                minecraft.options.keySprint.setDown(false);
                return null;
            });
        }
    }

    @Override
    public Path screenshot(Path path) throws Exception {
        Minecraft minecraft = Minecraft.getInstance();
        Path directory = screenshotBaseDirectory(path);
        Files.createDirectories(directory.resolve("screenshots"));
        Screenshot.grab(
            directory.toFile(),
            path.getFileName().toString(),
            minecraft.getMainRenderTarget(),
            1,
            message -> {
            }
        );
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
        return new M94RejectedBlockScenario().run(id, screenshotsDir, client);
    }

    @Override
    public void disconnect() {
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

    private static Minecraft requireInPlay() {
        Minecraft minecraft = Minecraft.getInstance();
        if (minecraft.player == null || minecraft.level == null || minecraft.gameMode == null) {
            throw new IllegalStateException("client is not in play");
        }
        return minecraft;
    }
}
