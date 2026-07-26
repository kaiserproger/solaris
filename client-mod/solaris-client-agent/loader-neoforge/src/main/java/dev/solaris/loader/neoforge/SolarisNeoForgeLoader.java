package dev.solaris.loader.neoforge;

import dev.solaris.loader.LoaderActivatedContent;
import dev.solaris.loader.LoaderInteractionAction;
import dev.solaris.loader.LoaderInteractionDefinition;
import dev.solaris.loader.LoaderOutgoing;
import dev.solaris.loader.LoaderPermissionController;
import dev.solaris.loader.LoaderPermissionRequest;
import dev.solaris.loader.LoaderPlatform;
import dev.solaris.loader.LoaderRuntimeSettings;
import dev.solaris.loader.LoaderScreenDefinition;
import dev.solaris.loader.LoaderScreenOpen;
import dev.solaris.loader.minecraft.LoaderRuntimeResourcePack;
import dev.solaris.loader.minecraft.LoaderMinecraftDisplay;
import dev.solaris.loader.minecraft.LoaderBlockCarrier;
import dev.solaris.loader.minecraft.LoaderMinecraftBlock;
import java.net.InetSocketAddress;
import java.net.SocketAddress;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.stream.IntStream;
import java.util.WeakHashMap;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.Consumer;
import java.util.stream.Collectors;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.screens.ConfirmScreen;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.client.multiplayer.ServerData;
import net.minecraft.network.chat.Component;
import net.minecraft.network.Connection;
import net.minecraft.network.protocol.common.ServerboundCustomPayloadPacket;
import net.minecraft.network.protocol.common.custom.CustomPacketPayload;
import net.neoforged.bus.api.IEventBus;
import net.neoforged.fml.common.Mod;
import net.neoforged.neoforge.client.event.ClientPlayerNetworkEvent;
import net.neoforged.neoforge.client.event.ClientTickEvent;
import net.neoforged.neoforge.common.NeoForge;
import net.neoforged.neoforge.network.event.RegisterPayloadHandlersEvent;
import net.neoforged.neoforge.registries.DeferredBlock;
import net.neoforged.neoforge.registries.DeferredItem;
import net.neoforged.neoforge.registries.DeferredRegister;
import net.minecraft.world.item.BlockItem;
import net.minecraft.world.level.block.Block;

@Mod(SolarisNeoForgeLoader.MOD_ID)
public final class SolarisNeoForgeLoader {
    public static final String MOD_ID = "solaris_loader";
    private static final DeferredRegister.Blocks BLOCKS =
            DeferredRegister.createBlocks(MOD_ID);
    private static final DeferredRegister.Items ITEMS =
            DeferredRegister.createItems(MOD_ID);
    private static final List<DeferredBlock<Block>> LOADER_BLOCKS =
            IntStream.range(0, LoaderBlockCarrier.MAX_CARRIERS)
                    .mapToObj(index -> BLOCKS.register(
                            LoaderBlockCarrier.path(index),
                            () -> LoaderBlockCarrier.createBlock(index)))
                    .toList();
    @SuppressWarnings("unused")
    private static final List<DeferredItem<BlockItem>> LOADER_BLOCK_ITEMS =
            IntStream.range(0, LoaderBlockCarrier.MAX_CARRIERS)
                    .mapToObj(index -> ITEMS.register(
                            LoaderBlockCarrier.path(index),
                            () -> LoaderBlockCarrier.createItem(
                                    index,
                                    LOADER_BLOCKS.get(index).get())))
                    .toList();
    private static final Map<Connection, LoaderPermissionController> CONTROLLERS =
            Collections.synchronizedMap(new WeakHashMap<>());
    private static final AtomicReference<LoaderActivatedContent> ACTIVE_CONTENT =
            new AtomicReference<>(LoaderActivatedContent.empty());
    private static final LoaderRuntimeResourcePack RESOURCE_PACK =
            new LoaderRuntimeResourcePack();
    private static final AtomicBoolean RESOURCE_PACK_REGISTERED =
            new AtomicBoolean();
    private static final LoaderNeoForgeMcpRuntime MCP_RUNTIME =
            LoaderNeoForgeMcpRuntime.createIfEnabled();

    public SolarisNeoForgeLoader(IEventBus modBus) {
        BLOCKS.register(modBus);
        ITEMS.register(modBus);
        modBus.addListener(SolarisNeoForgeLoader::registerPayloads);
        NeoForge.EVENT_BUS.addListener(SolarisNeoForgeLoader::onClientTickPre);
        NeoForge.EVENT_BUS.addListener(SolarisNeoForgeLoader::onClientTickPost);
        NeoForge.EVENT_BUS.addListener(SolarisNeoForgeLoader::onLoggingIn);
        NeoForge.EVENT_BUS.addListener(SolarisNeoForgeLoader::onLoggingOut);
    }

    private static void onClientTickPre(ClientTickEvent.Pre event) {
        if (MCP_RUNTIME != null) {
            MCP_RUNTIME.beforeTick();
        }
    }

    private static void onClientTickPost(ClientTickEvent.Post event) {
        if (MCP_RUNTIME != null) {
            MCP_RUNTIME.afterTick();
        }
    }

    private static void onLoggingOut(ClientPlayerNetworkEvent.LoggingOut event) {
        publishMcpState();
        Connection connection = event.getConnection();
        if (connection != null) {
            unmount(Minecraft.getInstance(), connection);
        }
    }

    private static void onLoggingIn(ClientPlayerNetworkEvent.LoggingIn event) {
        publishMcpState();
    }

    private static void registerPayloads(RegisterPayloadHandlersEvent event) {
        var registrar = event.registrar("1").optional();
        registrar.playToClient(
                LoaderOpenScreenPayload.TYPE,
                LoaderOpenScreenPayload.CODEC,
                (payload, context) -> {
                    Connection origin = context.connection();
                    Minecraft.getInstance().execute(() -> {
                        Minecraft client = Minecraft.getInstance();
                        var listener = client.getConnection();
                        boolean originActive = origin.isConnected()
                                && listener != null
                                && listener.getConnection() == origin;
                        Optional<LoaderScreenDefinition> screen =
                                resolveScreen(payload.bytes(), originActive);
                        screen.ifPresent(definition ->
                                client.setScreen(new LoaderTextScreen(
                                        definition,
                                        interactionsFor(definition),
                                        interaction -> sendInteraction(
                                                client,
                                                origin,
                                                interaction),
                                        LoaderMinecraftDisplay.forScreen(
                                                definition,
                                                ACTIVE_CONTENT.get()))));
                    });
                });
        registrar.playToServer(
                LoaderInteractionPayload.TYPE,
                LoaderInteractionPayload.CODEC,
                (payload, context) -> context.disconnect(
                        Component.literal("Solaris Loader interaction is client-only")));
        registrar.configurationToClient(
                LoaderManifestPayload.TYPE,
                LoaderManifestPayload.CODEC,
                (manifest, context) -> {
                    try {
                        ACTIVE_CONTENT.set(LoaderActivatedContent.empty());
                        Connection connection = context.connection();
                        Minecraft client = Minecraft.getInstance();
                        ensureResourcePackRegistered(client);
                        watchConnection(client, connection);
                        controller(connection).acceptManifest(
                                manifest.bytes(),
                                LoaderPlatform.NEOFORGE,
                                LoaderRuntimeSettings.LOADER_VERSION,
                                serverIdentity(connection),
                                LoaderRuntimeSettings.cacheDirectory(),
                                connection::isConnected,
                                SolarisNeoForgeLoader::prompt,
                                outgoing -> send(
                                        Minecraft.getInstance(),
                                        connection,
                                        outgoing),
                                reason -> disconnect(context.connection(), reason));
                    } catch (IllegalArgumentException error) {
                        disconnect(context.connection(), error.getMessage());
                    }
                });
        registrar.configurationToClient(
                LoaderArtifactPayload.TYPE,
                LoaderArtifactPayload.CODEC,
                (artifact, context) -> {
                    try {
                        controller(context.connection())
                                .acceptArtifact(
                                        artifact.bytes(),
                                        context.connection()::isConnected)
                                .ifPresent(outgoing -> send(
                                        Minecraft.getInstance(),
                                        context.connection(),
                                        outgoing));
                    } catch (IllegalArgumentException error) {
                        disconnect(context.connection(), error.getMessage());
                    }
                });
        registrar.configurationToServer(
                LoaderRequestPayload.TYPE,
                LoaderRequestPayload.CODEC,
                (payload, context) -> context.disconnect(
                        Component.literal("Solaris Loader request is client-only")));
        registrar.configurationToServer(
                LoaderAckPayload.TYPE,
                LoaderAckPayload.CODEC,
                (payload, context) -> context.disconnect(
                        Component.literal("Solaris Loader acknowledgement is client-only")));
    }

    static CustomPacketPayload payload(LoaderOutgoing outgoing) {
        activate(outgoing);
        return switch (outgoing.kind()) {
            case REQUEST -> new LoaderRequestPayload(outgoing.bytes());
            case ACKNOWLEDGEMENT -> new LoaderAckPayload(outgoing.bytes());
        };
    }

    private static void send(
            Minecraft client,
            Connection connection,
            LoaderOutgoing outgoing) {
        if (outgoing.kind() == LoaderOutgoing.Kind.REQUEST) {
            connection.send(new ServerboundCustomPayloadPacket(payload(outgoing)));
            return;
        }
        RESOURCE_PACK.mount(
                        client,
                        connection,
                        connection::isConnected,
                        outgoing.activatedContent())
                .whenCompleteAsync((unused, error) -> {
                    if (error != null
                            || !connection.isConnected()
                            || !RESOURCE_PACK.owns(connection)) {
                        unmount(client, connection);
                        disconnect(
                                connection,
                                error == null
                                        ? "connection closed before Loader resources mounted"
                                        : "mounting Loader resources: " + error.getMessage());
                        return;
                    }
                    connection.send(new ServerboundCustomPayloadPacket(payload(outgoing)));
                }, client);
    }

    static void activate(LoaderOutgoing outgoing) {
        if (outgoing.kind() == LoaderOutgoing.Kind.ACKNOWLEDGEMENT) {
            ACTIVE_CONTENT.set(outgoing.activatedContent());
        }
    }

    public static LoaderActivatedContent activeContent() {
        return ACTIVE_CONTENT.get();
    }

    static void clearActiveContent() {
        ACTIVE_CONTENT.set(LoaderActivatedContent.empty());
    }

    private static void watchConnection(
            Minecraft client,
            Connection connection) {
        connection.channel().closeFuture().addListener(
                ignored -> unmount(client, connection));
    }

    private static void ensureResourcePackRegistered(Minecraft client) {
        if (RESOURCE_PACK_REGISTERED.compareAndSet(false, true)) {
            client.getResourcePackRepository()
                    .addPackFinder(RESOURCE_PACK.repositorySource());
        }
    }

    private static void unmount(
            Minecraft client,
            Connection connection) {
        RESOURCE_PACK.unmount(
                client,
                connection,
                SolarisNeoForgeLoader::clearActiveContent);
    }

    static Optional<LoaderScreenDefinition> resolveScreen(
            byte[] payload,
            boolean connectionActive) {
        return LoaderScreenOpen.resolve(payload, ACTIVE_CONTENT.get(), connectionActive);
    }

    static List<LoaderInteractionDefinition> interactionsFor(
            LoaderScreenDefinition screen) {
        return ACTIVE_CONTENT.get().interactions().values().stream()
                .filter(interaction -> interaction.screenId().equals(screen.id()))
                .sorted(Comparator.comparing(LoaderInteractionDefinition::id))
                .toList();
    }

    private static void sendInteraction(
            Minecraft client,
            Connection origin,
            LoaderInteractionDefinition interaction) {
        var listener = client.getConnection();
        boolean originActive = origin.isConnected()
                && listener != null
                && listener.getConnection() == origin;
        LoaderInteractionAction.encode(
                        interaction,
                        ACTIVE_CONTENT.get(),
                        originActive)
                .ifPresent(bytes -> origin.send(
                        new ServerboundCustomPayloadPacket(
                                new LoaderInteractionPayload(bytes))));
    }

    private static LoaderPermissionController controller(Connection connection) {
        synchronized (CONTROLLERS) {
            return CONTROLLERS.computeIfAbsent(
                    connection,
                    ignored -> new LoaderPermissionController(
                            LoaderRuntimeSettings.permissionDecisionsFile(),
                            LoaderMinecraftBlock::carrierStateIds));
        }
    }

    private static void prompt(
            LoaderPermissionRequest request,
            Consumer<Boolean> decision) {
        Minecraft client = Minecraft.getInstance();
        String permissions = request.permissions().stream()
                .map(permission -> permission.wireName())
                .collect(Collectors.joining(", "));
        client.execute(() -> {
            Screen parent = client.screen;
            client.setScreen(new ConfirmScreen(
                    allowed -> {
                        client.setScreen(parent);
                        publishMcpState();
                        decision.accept(allowed);
                    },
                    Component.literal("Allow Solaris content from " + request.serverIdentity() + "?"),
                    Component.literal("Requested permissions: " + permissions
                            + ". Downloaded content is untrusted."),
                    Component.literal("Allow"),
                    Component.literal("Deny")));
            publishMcpState();
        });
    }

    private static void publishMcpState() {
        if (MCP_RUNTIME != null) {
            MCP_RUNTIME.publishState();
        }
    }

    private static String serverIdentity(Connection connection) {
        ServerData server = Minecraft.getInstance().getCurrentServer();
        return serverIdentity(
                server == null ? null : server.ip,
                connection.getRemoteAddress());
    }

    static String serverIdentity(
            String configuredAddress,
            SocketAddress remoteAddress) {
        if (configuredAddress != null && !configuredAddress.isBlank()) {
            return configuredAddress;
        }
        if (remoteAddress instanceof InetSocketAddress address) {
            return address.getHostString() + ":" + address.getPort();
        }
        throw new IllegalArgumentException(
                "Solaris Loader cannot identify the current server");
    }

    private static void disconnect(Connection connection, String reason) {
        connection.disconnect(Component.literal(
                "Solaris Loader rejected manifest: " + reason));
    }
}
