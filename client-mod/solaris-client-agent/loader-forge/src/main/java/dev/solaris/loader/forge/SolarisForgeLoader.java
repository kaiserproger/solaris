package dev.solaris.loader.forge;

import dev.solaris.loader.LoaderActivatedContent;
import dev.solaris.loader.LoaderHandshake;
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
import dev.solaris.loader.minecraft.LoaderMinecraftBlock;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.WeakHashMap;
import java.net.InetSocketAddress;
import java.net.SocketAddress;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.Consumer;
import java.util.stream.Collectors;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.screens.ConfirmScreen;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.client.multiplayer.ServerData;
import net.minecraft.network.Connection;
import net.minecraft.network.FriendlyByteBuf;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.network.chat.Component;
import net.minecraft.network.protocol.common.custom.CustomPacketPayload;
import net.minecraftforge.client.event.ClientPlayerNetworkEvent;
import net.minecraftforge.event.TickEvent.ClientTickEvent;
import net.minecraftforge.fml.config.ConfigTracker;
import net.minecraftforge.fml.event.lifecycle.FMLClientSetupEvent;
import net.minecraftforge.fml.common.Mod;
import net.minecraftforge.fml.javafmlmod.FMLJavaModLoadingContext;
import net.minecraftforge.network.Channel;
import net.minecraftforge.network.ChannelBuilder;
import net.minecraftforge.network.payload.PayloadConnection;
import net.minecraftforge.network.payload.PayloadFlow;

@Mod(SolarisForgeLoader.MOD_ID)
public final class SolarisForgeLoader {
    public static final String MOD_ID = "solaris_loader";
    private static final Map<Connection, LoaderPermissionController> CONTROLLERS =
            Collections.synchronizedMap(new WeakHashMap<>());
    private static final AtomicReference<LoaderActivatedContent> ACTIVE_CONTENT =
            new AtomicReference<>(LoaderActivatedContent.empty());
    private static final LoaderRuntimeResourcePack RESOURCE_PACK =
            new LoaderRuntimeResourcePack();
    private static final AtomicBoolean RESOURCE_PACK_REGISTERED =
            new AtomicBoolean();
    private static final LoaderForgeMcpRuntime MCP_RUNTIME =
            LoaderForgeMcpRuntime.createIfEnabled();
    private static volatile boolean clientSetupComplete;
    private static Channel<CustomPacketPayload> channel;

    public SolarisForgeLoader(FMLJavaModLoadingContext context) {
        SolarisForgeContent.register(context);
        FMLClientSetupEvent.getBus(context.getModBusGroup())
                .addListener(SolarisForgeLoader::onClientSetup);
        ClientTickEvent.Pre.BUS.addListener(SolarisForgeLoader::onClientTickPre);
        ClientTickEvent.Post.BUS.addListener(SolarisForgeLoader::onClientTickPost);
        ClientPlayerNetworkEvent.LoggingIn.BUS.addListener(
                SolarisForgeLoader::onLoggingIn);
        ClientPlayerNetworkEvent.LoggingOut.BUS.addListener(
                SolarisForgeLoader::onLoggingOut);
        channel = buildChannel();
    }

    private static void onClientSetup(FMLClientSetupEvent event) {
        ConfigTracker.loadDefaultServerConfigs();
        clientSetupComplete = true;
    }

    private static void onClientTickPre(ClientTickEvent.Pre event) {
        if (MCP_RUNTIME != null && clientSetupComplete) {
            MCP_RUNTIME.beforeTick();
        }
    }

    private static void onClientTickPost(ClientTickEvent.Post event) {
        if (MCP_RUNTIME != null && clientSetupComplete) {
            MCP_RUNTIME.afterTick();
        }
    }

    private static void onLoggingIn(ClientPlayerNetworkEvent.LoggingIn event) {
        publishMcpState();
    }

    private static void onLoggingOut(ClientPlayerNetworkEvent.LoggingOut event) {
        publishMcpState();
        Connection connection = event.getConnection();
        if (connection != null) {
            unmount(Minecraft.getInstance(), connection);
        }
    }

    private static Channel<CustomPacketPayload> buildChannel() {
        PayloadConnection<CustomPacketPayload> connection =
                ChannelBuilder.named("solaris:loader")
                        .networkProtocolVersion(LoaderHandshake.PROTOCOL_VERSION)
                        .optional()
                        .payloadChannel();
        PayloadFlow<FriendlyByteBuf, CustomPacketPayload> flow =
                connection.configuration().clientbound();
        PayloadFlow<RegistryFriendlyByteBuf, CustomPacketPayload> play =
                connection.play().clientbound();
        play.add(
                LoaderOpenScreenPayload.TYPE,
                LoaderOpenScreenPayload.CODEC,
                (payload, context) -> {
                    Minecraft client = Minecraft.getInstance();
                    Connection origin = context.getConnection();
                    client.execute(() -> {
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
                    context.setPacketHandled(true);
                });
        PayloadFlow<RegistryFriendlyByteBuf, CustomPacketPayload> playServerbound =
                play.serverbound();
        playServerbound.add(
                LoaderInteractionPayload.TYPE,
                LoaderInteractionPayload.CODEC,
                (payload, context) -> context.setPacketHandled(true));
        flow.add(
                LoaderManifestPayload.TYPE,
                LoaderManifestPayload.CODEC,
                (manifest, context) -> {
                    try {
                        ACTIVE_CONTENT.set(LoaderActivatedContent.empty());
                        Connection networkConnection = context.getConnection();
                        Minecraft client = Minecraft.getInstance();
                        ensureResourcePackRegistered(client);
                        watchConnection(client, networkConnection);
                        controller(networkConnection).acceptManifest(
                                        manifest.bytes(),
                                        LoaderPlatform.FORGE,
                                        LoaderRuntimeSettings.LOADER_VERSION,
                                        serverIdentity(networkConnection),
                                        LoaderRuntimeSettings.cacheDirectory(),
                                        networkConnection::isConnected,
                                        SolarisForgeLoader::prompt,
                                        outgoing -> send(
                                                Minecraft.getInstance(),
                                                networkConnection,
                                                outgoing),
                                        reason -> disconnect(networkConnection, reason));
                    } catch (IllegalArgumentException error) {
                        disconnect(context.getConnection(), error.getMessage());
                    }
                    context.setPacketHandled(true);
                });
        flow.add(
                LoaderArtifactPayload.TYPE,
                LoaderArtifactPayload.CODEC,
                (artifact, context) -> {
                    try {
                        controller(context.getConnection())
                                .acceptArtifact(
                                        artifact.bytes(),
                                        context.getConnection()::isConnected)
                                .ifPresent(outgoing -> send(
                                        Minecraft.getInstance(),
                                        context.getConnection(),
                                        outgoing));
                    } catch (IllegalArgumentException error) {
                        disconnect(context.getConnection(), error.getMessage());
                    }
                    context.setPacketHandled(true);
                });
        PayloadFlow<FriendlyByteBuf, CustomPacketPayload> serverbound = flow.serverbound();
        serverbound.add(
                LoaderRequestPayload.TYPE,
                LoaderRequestPayload.CODEC,
                (payload, context) -> {
                    context.getConnection()
                            .disconnect(Component.literal(
                                    "Solaris Loader request is client-only"));
                    context.setPacketHandled(true);
                });
        serverbound.add(
                LoaderAckPayload.TYPE,
                LoaderAckPayload.CODEC,
                (payload, context) -> {
                    context.getConnection()
                            .disconnect(Component.literal(
                                    "Solaris Loader acknowledgement is client-only"));
                    context.setPacketHandled(true);
                });
        return flow.build();
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
            channel.send(payload(outgoing), connection);
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
                    channel.send(payload(outgoing), connection);
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
                SolarisForgeLoader::clearActiveContent);
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
                .ifPresent(bytes -> channel.send(
                        new LoaderInteractionPayload(bytes),
                        origin));
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
