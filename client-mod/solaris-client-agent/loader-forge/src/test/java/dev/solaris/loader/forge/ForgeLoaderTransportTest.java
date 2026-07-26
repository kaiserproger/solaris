package dev.solaris.loader.forge;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import dev.solaris.loader.LoaderClientTransport;
import dev.solaris.loader.LoaderEnvironment;
import dev.solaris.loader.LoaderInteractionAction;
import dev.solaris.loader.LoaderOutgoing;
import dev.solaris.loader.LoaderPermission;
import dev.solaris.loader.LoaderPlatform;
import io.netty.buffer.Unpooled;
import java.io.ByteArrayOutputStream;
import java.net.InetSocketAddress;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.Base64;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.MessageDigest;
import java.util.HexFormat;
import java.util.Set;
import java.util.zip.ZipEntry;
import java.util.zip.ZipOutputStream;
import net.minecraft.network.FriendlyByteBuf;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.core.RegistryAccess;
import net.minecraft.network.protocol.common.ClientboundCustomPayloadPacket;
import net.minecraft.network.protocol.common.custom.CustomPacketPayload;
import net.minecraft.network.protocol.common.custom.DiscardedPayload;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import java.lang.reflect.Method;
import net.minecraftforge.network.ForgePayload;
import net.minecraftforge.network.NetworkRegistry;

final class ForgeLoaderTransportTest {
    private static final byte[] SERVER_MANIFEST_PAYLOAD = Base64.getDecoder().decode(
            "F3NvbGFyaXM6bG9hZGVyL21hbmlmZXN0eyJwcm90b2NvbCI6MSwiYnVuZGxlcyI6W3sib3duZXIiOiJydWJ5LWxpdmUiLCJpZCI6InJpY2gtY29udGVudCIsInZlcnNpb24iOiIxIiwiYXJ0aWZhY3QiOiJjbGllbnQvcmljaC1jb250ZW50LnppcCIsInNoYTI1NiI6IjcwZGQ1MjdhYzBjNTA3NWZhZjFkZmY2NWU4ZTQyNmY2NTc3NDZkNDIyMTVlNGZjNGZkMTgyNDRhYzViOWQ3NjUiLCJzaXplX2J5dGVzIjoxMDA5LCJsb2FkZXJzIjpbImZhYnJpYyIsIm5lb2ZvcmdlIiwiZm9yZ2UiXSwiY29udGVudCI6WyJibG9ja3MiLCJpdGVtcyIsInNjcmVlbnMiLCJhc3NldHMiLCJpbnRlcmFjdGlvbnMiXSwicGVybWlzc2lvbnMiOlsicmVnaXN0ZXJfYmxvY2tzIiwicmVnaXN0ZXJfaXRlbXMiLCJvcGVuX3NjcmVlbnMiLCJsb2FkX2Fzc2V0cyIsInNlbmRfaW50ZXJhY3Rpb25zIl0sImNhY2hlX2tleSI6InJ1YnktbGl2ZTpyaWNoLWNvbnRlbnQvMS83MGRkNTI3YWMwYzUwNzVmYWYxZGZmNjVlOGU0MjZmNjU3NzQ2ZDQyMjE1ZTRmYzRmZDE4MjQ0YWM1YjlkNzY1In0seyJvd25lciI6InNhcHBoaXJlLWxpdmUiLCJpZCI6InJpY2gtY29udGVudCIsInZlcnNpb24iOiIxIiwiYXJ0aWZhY3QiOiJjbGllbnQvcmljaC1jb250ZW50LnppcCIsInNoYTI1NiI6IjZjMTY0MjViMmJmOWM1NDE1MTg0MzQ1YzRjYjZiYzEwZTk4YmY0MWEzZTczZGMyN2IzOTE1YWE3OTYyNDE4YTUiLCJzaXplX2J5dGVzIjoxMDQ4LCJsb2FkZXJzIjpbImZhYnJpYyIsIm5lb2ZvcmdlIiwiZm9yZ2UiXSwiY29udGVudCI6WyJibG9ja3MiLCJpdGVtcyIsInNjcmVlbnMiLCJhc3NldHMiLCJpbnRlcmFjdGlvbnMiXSwicGVybWlzc2lvbnMiOlsicmVnaXN0ZXJfYmxvY2tzIiwicmVnaXN0ZXJfaXRlbXMiLCJvcGVuX3NjcmVlbnMiLCJsb2FkX2Fzc2V0cyIsInNlbmRfaW50ZXJhY3Rpb25zIl0sImNhY2hlX2tleSI6InNhcHBoaXJlLWxpdmU6cmljaC1jb250ZW50LzEvNmMxNjQyNWIyYmY5YzU0MTUxODQzNDVjNGNiNmJjMTBlOThiZjQxYTNlNzNkYzI3YjM5MTVhYTc5NjI0MThhNSJ9XX0="
    );
    private static final byte[] ARCHIVE = archive();
    private static final String HASH = sha256(ARCHIVE);
    private static final String CACHE_KEY = "example:screen/1/" + HASH;

    @TempDir
    Path cacheDirectory;

    @Test
    void configurationServerIdentityFallsBackToRemoteAddress() {
        assertEquals(
                "play.example.test:25565",
                SolarisForgeLoader.serverIdentity(
                        "play.example.test:25565",
                        new InetSocketAddress("127.0.0.1", 25570)));
        assertEquals(
                "127.0.0.1:25570",
                SolarisForgeLoader.serverIdentity(
                        null,
                        new InetSocketAddress("127.0.0.1", 25570)));
    }

    @Test
    void verifiedCacheProducesRawConfigurationAck() throws Exception {
        byte[] manifest = manifest("forge");
        LoaderEnvironment environment = environment();
        Path cached = cacheDirectory.resolve("example/screen/1/" + HASH + ".bundle");
        Files.createDirectories(cached.getParent());
        Files.write(cached, ARCHIVE);

        FriendlyByteBuf inbound = new FriendlyByteBuf(Unpooled.buffer());
        LoaderManifestPayload.CODEC.encode(inbound, new LoaderManifestPayload(manifest));
        LoaderManifestPayload decodedManifest = LoaderManifestPayload.CODEC.decode(inbound);
        LoaderOutgoing outgoing = new LoaderClientTransport()
                .acceptManifest(decodedManifest.bytes(), environment, cacheDirectory);
        assertEquals(LoaderOutgoing.Kind.ACKNOWLEDGEMENT, outgoing.kind());
        byte[] acknowledgement = outgoing.bytes();
        FriendlyByteBuf wire = new FriendlyByteBuf(Unpooled.buffer());
        LoaderAckPayload.CODEC.encode(wire, new LoaderAckPayload(acknowledgement));
        LoaderAckPayload decoded = LoaderAckPayload.CODEC.decode(wire);

        assertArrayEquals(acknowledgement, decoded.bytes());
        assertTrue(new String(acknowledgement, StandardCharsets.UTF_8)
                .contains("\"platform\":\"forge\""));
        assertInstanceOf(LoaderAckPayload.class, SolarisForgeLoader.payload(outgoing));
        assertTrue(SolarisForgeLoader.activeContent()
                .screens()
                .containsKey("example:welcome"));
        var interaction = SolarisForgeLoader.activeContent()
                .interactions()
                .get("example:continue");
        assertEquals(1, SolarisForgeLoader.interactionsFor(
                        SolarisForgeLoader.activeContent()
                                .screens()
                                .get("example:welcome"))
                .size());
        byte[] interactionBytes = LoaderInteractionAction
                .encode(interaction, SolarisForgeLoader.activeContent(), true)
                .orElseThrow();
        RegistryFriendlyByteBuf interactionWire =
                new RegistryFriendlyByteBuf(Unpooled.buffer(), RegistryAccess.EMPTY);
        LoaderInteractionPayload.CODEC.encode(
                interactionWire,
                new LoaderInteractionPayload(interactionBytes));
        assertArrayEquals(
                interactionBytes,
                LoaderInteractionPayload.CODEC.decode(interactionWire).bytes());
        assertEquals(
                "solaris:loader/interaction",
                LoaderInteractionPayload.TYPE.id().toString());
        byte[] openScreen = openScreen("example:welcome");
        RegistryFriendlyByteBuf openWire =
                new RegistryFriendlyByteBuf(Unpooled.buffer(), RegistryAccess.EMPTY);
        LoaderOpenScreenPayload.CODEC.encode(
                openWire, new LoaderOpenScreenPayload(openScreen));
        assertEquals(openScreen.length, openWire.readableBytes());
        assertTrue(SolarisForgeLoader.resolveScreen(
                        LoaderOpenScreenPayload.CODEC.decode(openWire).bytes(),
                        true)
                .isPresent());
        SolarisForgeLoader.clearActiveContent();
        assertTrue(SolarisForgeLoader.activeContent().screens().isEmpty());
        assertTrue(SolarisForgeLoader.resolveScreen(openScreen, true).isEmpty());
        assertEquals("solaris:loader/manifest", LoaderManifestPayload.TYPE.id().toString());
        assertEquals("solaris:loader/ack", LoaderAckPayload.TYPE.id().toString());
        assertEquals("solaris:loader/request", LoaderRequestPayload.TYPE.id().toString());
        assertEquals("solaris:loader/artifact", LoaderArtifactPayload.TYPE.id().toString());

        FriendlyByteBuf requestWire = new FriendlyByteBuf(Unpooled.buffer());
        LoaderRequestPayload.CODEC.encode(
                requestWire, new LoaderRequestPayload("request".getBytes(StandardCharsets.UTF_8)));
        assertArrayEquals(
                "request".getBytes(StandardCharsets.UTF_8),
                LoaderRequestPayload.CODEC.decode(requestWire).bytes());
        FriendlyByteBuf artifactWire = new FriendlyByteBuf(Unpooled.buffer());
        LoaderArtifactPayload.CODEC.encode(
                artifactWire, new LoaderArtifactPayload("artifact".getBytes(StandardCharsets.UTF_8)));
        assertArrayEquals(
                "artifact".getBytes(StandardCharsets.UTF_8),
                LoaderArtifactPayload.CODEC.decode(artifactWire).bytes());
    }

    @Test
    void decodeCapturedManifestThroughConfigCodec() {
        ensureLoaderChannelRegistered();

        assertNotNull(NetworkRegistry.findTarget(LoaderManifestPayload.TYPE.id()));

        FriendlyByteBuf capturedPayload =
                new FriendlyByteBuf(Unpooled.wrappedBuffer(SERVER_MANIFEST_PAYLOAD));
        ClientboundCustomPayloadPacket decoded =
                ClientboundCustomPayloadPacket.CONFIG_STREAM_CODEC.decode(capturedPayload);

        assertFalse(decoded.payload() instanceof DiscardedPayload);
        assertInstanceOf(ForgePayload.class, decoded.payload());

        ForgePayload payload = (ForgePayload) decoded.payload();
        assertEquals(LoaderManifestPayload.TYPE.id(), payload.id());
        LoaderManifestPayload manifestPayload = LoaderManifestPayload.CODEC.decode(
                new FriendlyByteBuf(payload.data().copy()));
        assertTrue(new String(manifestPayload.bytes(), StandardCharsets.UTF_8)
                .startsWith("{\"protocol\":1"));
    }

    private static void ensureLoaderChannelRegistered() {
        if (NetworkRegistry.findTarget(LoaderManifestPayload.TYPE.id()) != null) {
            return;
        }
        try {
            Method buildChannel =
                    SolarisForgeLoader.class.getDeclaredMethod("buildChannel");
            buildChannel.setAccessible(true);
            buildChannel.invoke(null);
        } catch (Exception error) {
            throw new AssertionError("Failed to initialize Solaris Forge channel", error);
        }
    }

    private static LoaderEnvironment environment() {
        return new LoaderEnvironment() {
            @Override
            public LoaderPlatform platform() {
                return LoaderPlatform.FORGE;
            }

            @Override
            public String loaderVersion() {
                return "0.1.0";
            }

            @Override
            public Set<LoaderPermission> grantedPermissions() {
                return Set.of(
                        LoaderPermission.OPEN_SCREENS,
                        LoaderPermission.SEND_INTERACTIONS);
            }
        };
    }

    private static byte[] manifest(String platform) {
        return """
                {"protocol":1,"bundles":[{
                  "owner":"example","id":"screen","version":"1",
                  "artifact":"client/screen.zip","sha256":"%s","size_bytes":%d,
                  "loaders":["%s"],"content":["screens","interactions"],
                  "permissions":["open_screens","send_interactions"],
                  "cache_key":"%s"
                }]}
                """.formatted(HASH, ARCHIVE.length, platform, CACHE_KEY)
                .getBytes(StandardCharsets.UTF_8);
    }

    private static byte[] archive() {
        try {
            ByteArrayOutputStream bytes = new ByteArrayOutputStream();
            try (ZipOutputStream zip = new ZipOutputStream(bytes)) {
                zip.putNextEntry(new ZipEntry("solaris-client.json"));
                zip.write("""
                        {"schema":1,"screens":[{
                          "id":"example:welcome","title":"Welcome","body":"Forge"
                        }],"blocks":[],"items":[],"assets":[],"interactions":[{
                          "id":"example:continue","screen_id":"example:welcome",
                          "label":"Continue","payload":"accepted"
                        }]}
                        """.getBytes(StandardCharsets.UTF_8));
                zip.closeEntry();
            }
            return bytes.toByteArray();
        } catch (Exception error) {
            throw new AssertionError(error);
        }
    }

    private static String sha256(byte[] bytes) {
        try {
            return HexFormat.of().formatHex(
                    MessageDigest.getInstance("SHA-256").digest(bytes));
        } catch (Exception error) {
            throw new AssertionError(error);
        }
    }

    private static byte[] openScreen(String id) {
        byte[] bytes = id.getBytes(StandardCharsets.UTF_8);
        return ByteBuffer.allocate(4 + bytes.length)
                .order(ByteOrder.BIG_ENDIAN)
                .putShort((short) 1)
                .putShort((short) bytes.length)
                .put(bytes)
                .array();
    }
}
