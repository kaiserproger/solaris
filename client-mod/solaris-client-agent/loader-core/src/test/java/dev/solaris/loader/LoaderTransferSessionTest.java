package dev.solaris.loader;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.MessageDigest;
import java.util.Arrays;
import java.util.HexFormat;
import java.util.Set;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

final class LoaderTransferSessionTest {
    @TempDir
    Path cacheDirectory;

    @Test
    void missingBundleIsVerifiedAndAtomicallyPublishedBeforeAck() throws Exception {
        byte[] artifact = LoaderTestArchive.screenOnly();
        String sha256 = sha256(artifact);
        String cacheKey = "example:screen/1/" + sha256;
        byte[] manifest = manifest(artifact.length, sha256, cacheKey);
        LoaderClientTransport transport = new LoaderClientTransport();

        LoaderOutgoing request =
                transport.acceptManifest(manifest, environment(), cacheDirectory);
        assertEquals(LoaderOutgoing.Kind.REQUEST, request.kind());
        assertTrue(new String(request.bytes(), StandardCharsets.UTF_8).contains(cacheKey));

        int split = artifact.length / 2;
        assertTrue(transport
                .acceptArtifact(chunk(
                        cacheKey,
                        0,
                        false,
                        Arrays.copyOfRange(artifact, 0, split)))
                .isEmpty());
        LoaderOutgoing acknowledgement = transport
                .acceptArtifact(chunk(
                        cacheKey,
                        split,
                        true,
                        Arrays.copyOfRange(artifact, split, artifact.length)))
                .orElseThrow();

        assertEquals(LoaderOutgoing.Kind.ACKNOWLEDGEMENT, acknowledgement.kind());
        assertTrue(new String(acknowledgement.bytes(), StandardCharsets.UTF_8)
                .contains("\"cached_bundles\":[\"" + cacheKey + "\"]"));
        Path published = cacheDirectory
                .resolve("example")
                .resolve("screen")
                .resolve("1")
                .resolve(sha256 + ".bundle");
        assertArrayEquals(artifact, Files.readAllBytes(published));

        LoaderOutgoing cached =
                new LoaderClientTransport().acceptManifest(manifest, environment(), cacheDirectory);
        assertEquals(LoaderOutgoing.Kind.ACKNOWLEDGEMENT, cached.kind());
    }

    @Test
    void hashMismatchPublishesNothingAndRemovesStagingFile() throws Exception {
        byte[] expected = LoaderTestArchive.screenOnly();
        byte[] corrupt = expected.clone();
        corrupt[corrupt.length - 1] ^= 1;
        String sha256 = sha256(expected);
        String cacheKey = "example:screen/1/" + sha256;
        LoaderClientTransport transport = new LoaderClientTransport();
        transport.acceptManifest(
                manifest(expected.length, sha256, cacheKey),
                environment(),
                cacheDirectory);

        assertThrows(
                IllegalArgumentException.class,
                () -> transport.acceptArtifact(chunk(cacheKey, 0, true, corrupt)));

        Path versionDirectory =
                cacheDirectory.resolve("example").resolve("screen").resolve("1");
        assertFalse(Files.exists(versionDirectory.resolve(sha256 + ".bundle")));
        try (var entries = Files.list(versionDirectory)) {
            assertEquals(0, entries.count());
        }
    }

    @Test
    void outOfOrderOrOversizedBytesFailClosed() {
        byte[] artifact = LoaderTestArchive.screenOnly();
        String sha256 = sha256(artifact);
        String cacheKey = "example:screen/1/" + sha256;
        LoaderClientTransport transport = new LoaderClientTransport();
        transport.acceptManifest(
                manifest(artifact.length, sha256, cacheKey),
                environment(),
                cacheDirectory);

        assertThrows(
                IllegalArgumentException.class,
                () -> transport.acceptArtifact(chunk(cacheKey, 1, true, artifact)));
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
                return Set.of(LoaderPermission.OPEN_SCREENS);
            }
        };
    }

    private static byte[] manifest(int size, String sha256, String cacheKey) {
        return """
                {"protocol":1,"bundles":[{
                  "owner":"example","id":"screen","version":"1",
                  "artifact":"client/screen.zip","sha256":"%s","size_bytes":%d,
                  "loaders":["fabric"],"content":["screens"],"permissions":["open_screens"],
                  "cache_key":"%s"
                }]}
                """.formatted(sha256, size, cacheKey).getBytes(StandardCharsets.UTF_8);
    }

    private static byte[] chunk(String cacheKey, long offset, boolean last, byte[] bytes) {
        byte[] key = cacheKey.getBytes(StandardCharsets.UTF_8);
        return ByteBuffer.allocate(2 + 2 + key.length + 8 + 1 + bytes.length)
                .order(ByteOrder.BIG_ENDIAN)
                .putShort((short) LoaderHandshake.PROTOCOL_VERSION)
                .putShort((short) key.length)
                .put(key)
                .putLong(offset)
                .put((byte) (last ? 1 : 0))
                .put(bytes)
                .array();
    }

    private static String sha256(byte[] bytes) {
        try {
            return HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(bytes));
        } catch (Exception error) {
            throw new AssertionError(error);
        }
    }
}
