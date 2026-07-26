package dev.solaris.loader.fabric;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertTrue;

import dev.solaris.loader.LoaderClientTransport;
import dev.solaris.loader.LoaderEnvironment;
import dev.solaris.loader.LoaderInteractionAction;
import dev.solaris.loader.LoaderOutgoing;
import dev.solaris.loader.LoaderPermission;
import dev.solaris.loader.LoaderPlatform;
import io.netty.buffer.Unpooled;
import java.io.ByteArrayOutputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.MessageDigest;
import java.util.HexFormat;
import java.util.Set;
import java.util.zip.ZipEntry;
import java.util.zip.ZipOutputStream;
import net.minecraft.network.FriendlyByteBuf;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

final class FabricLoaderTransportTest {
    private static final byte[] ARCHIVE = archive();
    private static final String HASH = sha256(ARCHIVE);
    private static final String CACHE_KEY = "example:screen/1/" + HASH;

    @TempDir
    Path cacheDirectory;

    @Test
    void verifiedCacheProducesRawConfigurationAck() throws Exception {
        byte[] manifest = manifest("fabric");
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
                .contains("\"platform\":\"fabric\""));
        assertInstanceOf(LoaderAckPayload.class, SolarisFabricLoader.payload(outgoing));
        assertTrue(SolarisFabricLoader.activeContent()
                .screens()
                .containsKey("example:welcome"));
        var interaction = SolarisFabricLoader.activeContent()
                .interactions()
                .get("example:continue");
        assertEquals(1, SolarisFabricLoader.interactionsFor(
                        SolarisFabricLoader.activeContent()
                                .screens()
                                .get("example:welcome"))
                .size());
        byte[] interactionBytes = LoaderInteractionAction
                .encode(interaction, SolarisFabricLoader.activeContent(), true)
                .orElseThrow();
        FriendlyByteBuf interactionWire = new FriendlyByteBuf(Unpooled.buffer());
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
        FriendlyByteBuf openWire = new FriendlyByteBuf(Unpooled.buffer());
        LoaderOpenScreenPayload.CODEC.encode(
                openWire, new LoaderOpenScreenPayload(openScreen));
        assertEquals(openScreen.length, openWire.readableBytes());
        assertTrue(SolarisFabricLoader.resolveScreen(
                        LoaderOpenScreenPayload.CODEC.decode(openWire).bytes(),
                        true)
                .isPresent());
        SolarisFabricLoader.clearActiveContent();
        assertTrue(SolarisFabricLoader.activeContent().screens().isEmpty());
        assertTrue(SolarisFabricLoader.resolveScreen(openScreen, true).isEmpty());
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

    private static LoaderEnvironment environment() {
        return new LoaderEnvironment() {
            @Override
            public LoaderPlatform platform() {
                return LoaderPlatform.FABRIC;
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
                          "id":"example:welcome","title":"Welcome","body":"Fabric"
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
