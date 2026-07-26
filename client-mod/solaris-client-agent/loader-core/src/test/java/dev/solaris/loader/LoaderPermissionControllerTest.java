package dev.solaris.loader;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicReference;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.Consumer;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

final class LoaderPermissionControllerTest {
    private static final byte[] ARCHIVE = LoaderTestArchive.screenOnly();
    private static final String HASH = LoaderTestArchive.sha256(ARCHIVE);
    private static final String CACHE_KEY = "example:screen/1/" + HASH;

    @TempDir
    Path cacheDirectory;

    @Test
    void denialIsStoredPerServerBeforeAnyArtifactRequestOrStaging() {
        Path decisions = cacheDirectory.resolve("permissions.properties");
        LoaderPermissionController controller = new LoaderPermissionController(decisions);
        AtomicReference<Consumer<Boolean>> answer = new AtomicReference<>();
        List<LoaderOutgoing> outgoing = new ArrayList<>();
        List<String> rejected = new ArrayList<>();

        controller.acceptManifest(
                manifest("open_screens", "screens"),
                LoaderPlatform.FABRIC,
                "0.1.0",
                "Example.COM:25565",
                cacheDirectory,
                () -> true,
                (request, decision) -> {
                    assertEquals("example.com:25565", request.serverIdentity());
                    assertEquals(List.of(LoaderPermission.OPEN_SCREENS), request.permissions());
                    answer.set(decision);
                },
                outgoing::add,
                rejected::add);

        assertTrue(outgoing.isEmpty());
        assertFalse(Files.exists(cacheDirectory.resolve("example")));
        assertNotNull(answer.get());
        answer.get().accept(false);
        assertTrue(outgoing.isEmpty());
        assertEquals(1, rejected.size());
        assertFalse(Files.exists(cacheDirectory.resolve("example")));

        new LoaderPermissionController(decisions).acceptManifest(
                manifest("open_screens", "screens"),
                LoaderPlatform.FABRIC,
                "0.1.0",
                "example.com:25565",
                cacheDirectory,
                () -> true,
                (request, decision) -> {
                    throw new AssertionError("stored denial must not prompt again");
                },
                outgoing::add,
                rejected::add);
        assertTrue(outgoing.isEmpty());
        assertEquals(2, rejected.size());
        assertFalse(Files.exists(cacheDirectory.resolve("example")));
    }

    @Test
    void approvalIsReusedOnlyForTheSameServerAndPermissionSet() throws Exception {
        Path decisions = cacheDirectory.resolve("permissions.properties");
        Path cached = cacheDirectory.resolve("example/screen/1/" + HASH + ".bundle");
        Files.createDirectories(cached.getParent());
        Files.write(cached, ARCHIVE);
        List<LoaderOutgoing> outgoing = new ArrayList<>();

        LoaderPermissionController first = new LoaderPermissionController(decisions);
        first.acceptManifest(
                manifest("open_screens", "screens"),
                LoaderPlatform.NEOFORGE,
                "0.1.0",
                "play.example:25565",
                cacheDirectory,
                () -> true,
                (request, decision) -> decision.accept(true),
                outgoing::add,
                reason -> {
                    throw new AssertionError(reason);
                });
        assertEquals(LoaderOutgoing.Kind.ACKNOWLEDGEMENT, outgoing.get(0).kind());

        new LoaderPermissionController(decisions).acceptManifest(
                manifest("open_screens", "screens"),
                LoaderPlatform.NEOFORGE,
                "0.1.0",
                "PLAY.EXAMPLE:25565",
                cacheDirectory,
                () -> true,
                (request, decision) -> {
                    throw new AssertionError("same server decision must be reused");
                },
                outgoing::add,
                reason -> {
                    throw new AssertionError(reason);
                });
        assertEquals(2, outgoing.size());

        AtomicReference<LoaderPermissionRequest> otherServer = new AtomicReference<>();
        new LoaderPermissionController(decisions).acceptManifest(
                manifest("open_screens", "screens"),
                LoaderPlatform.NEOFORGE,
                "0.1.0",
                "other.example:25565",
                cacheDirectory,
                () -> true,
                (request, decision) -> otherServer.set(request),
                outgoing::add,
                reason -> {
                    throw new AssertionError(reason);
                });
        assertNotNull(otherServer.get());
        assertEquals(2, outgoing.size());

        AtomicReference<LoaderPermissionRequest> changedPermissions = new AtomicReference<>();
        new LoaderPermissionController(decisions).acceptManifest(
                manifest("load_assets", "assets"),
                LoaderPlatform.NEOFORGE,
                "0.1.0",
                "play.example:25565",
                cacheDirectory,
                () -> true,
                (request, decision) -> changedPermissions.set(request),
                outgoing::add,
                reason -> {
                    throw new AssertionError(reason);
                });
        assertEquals(
                List.of(LoaderPermission.LOAD_ASSETS),
                changedPermissions.get().permissions());
        assertEquals(2, outgoing.size());
    }

    @Test
    void stalePromptCannotPersistOrStartAfterConnectionEnds() {
        Path decisions = cacheDirectory.resolve("permissions.properties");
        LoaderPermissionController controller = new LoaderPermissionController(decisions);
        AtomicBoolean active = new AtomicBoolean(true);
        AtomicReference<Consumer<Boolean>> answer = new AtomicReference<>();
        List<LoaderOutgoing> outgoing = new ArrayList<>();

        controller.acceptManifest(
                manifest("open_screens", "screens"),
                LoaderPlatform.FABRIC,
                "0.1.0",
                "stale.example:25565",
                cacheDirectory,
                active::get,
                (request, decision) -> answer.set(decision),
                outgoing::add,
                reason -> {
                    throw new AssertionError(reason);
                });
        active.set(false);
        answer.get().accept(true);

        assertTrue(outgoing.isEmpty());
        assertFalse(Files.exists(decisions));
        assertFalse(Files.exists(cacheDirectory.resolve("example")));

        AtomicReference<LoaderPermissionRequest> promptedAgain = new AtomicReference<>();
        new LoaderPermissionController(decisions).acceptManifest(
                manifest("open_screens", "screens"),
                LoaderPlatform.FABRIC,
                "0.1.0",
                "stale.example:25565",
                cacheDirectory,
                () -> true,
                (request, decision) -> promptedAgain.set(request),
                outgoing::add,
                reason -> {
                    throw new AssertionError(reason);
                });
        assertNotNull(promptedAgain.get());
        assertTrue(outgoing.isEmpty());
    }

    @Test
    void lateArtifactCannotReachANewerConnectionSession() throws Exception {
        Path decisions = cacheDirectory.resolve("permissions.properties");
        AtomicBoolean oldActive = new AtomicBoolean(true);
        LoaderPermissionController oldConnection = new LoaderPermissionController(decisions);
        LoaderPermissionController newConnection = new LoaderPermissionController(decisions);
        List<LoaderOutgoing> oldOutgoing = new ArrayList<>();
        List<LoaderOutgoing> newOutgoing = new ArrayList<>();

        oldConnection.acceptManifest(
                manifest("open_screens", "screens"),
                LoaderPlatform.FORGE,
                "0.1.0",
                "transfer.example:25565",
                cacheDirectory,
                oldActive::get,
                (request, decision) -> decision.accept(true),
                oldOutgoing::add,
                reason -> {
                    throw new AssertionError(reason);
                });
        newConnection.acceptManifest(
                manifest("open_screens", "screens"),
                LoaderPlatform.FORGE,
                "0.1.0",
                "transfer.example:25565",
                cacheDirectory,
                () -> true,
                (request, decision) -> {
                    throw new AssertionError("stored approval must be reused");
                },
                newOutgoing::add,
                reason -> {
                    throw new AssertionError(reason);
                });
        assertEquals(LoaderOutgoing.Kind.REQUEST, oldOutgoing.get(0).kind());
        assertEquals(LoaderOutgoing.Kind.REQUEST, newOutgoing.get(0).kind());

        oldActive.set(false);
        assertThrows(
                IllegalArgumentException.class,
                () -> oldConnection.acceptArtifact(
                        chunk(CACHE_KEY, ARCHIVE),
                        oldActive::get));
        LoaderOutgoing acknowledgement = newConnection
                .acceptArtifact(chunk(CACHE_KEY, ARCHIVE), () -> true)
                .orElseThrow();
        assertEquals(LoaderOutgoing.Kind.ACKNOWLEDGEMENT, acknowledgement.kind());
        assertArrayEquals(
                ARCHIVE,
                Files.readAllBytes(cacheDirectory.resolve("example/screen/1/" + HASH + ".bundle")));
    }

    @Test
    void failedDecisionPublicationIsNotReusedInMemory() throws Exception {
        Path decisions = cacheDirectory.resolve("permissions.properties");
        LoaderPermissionController controller = new LoaderPermissionController(decisions);
        AtomicReference<Consumer<Boolean>> answer = new AtomicReference<>();
        List<String> rejected = new ArrayList<>();

        controller.acceptManifest(
                manifest("open_screens", "screens"),
                LoaderPlatform.FABRIC,
                "0.1.0",
                "store.example:25565",
                cacheDirectory,
                () -> true,
                (request, decision) -> answer.set(decision),
                outgoing -> {
                    throw new AssertionError("transfer must not start before decision publication");
                },
                rejected::add);
        Files.createDirectory(decisions);
        answer.get().accept(true);
        assertEquals(1, rejected.size());
        Files.delete(decisions);

        AtomicReference<LoaderPermissionRequest> promptedAgain = new AtomicReference<>();
        controller.acceptManifest(
                manifest("open_screens", "screens"),
                LoaderPlatform.FABRIC,
                "0.1.0",
                "store.example:25565",
                cacheDirectory,
                () -> true,
                (request, decision) -> promptedAgain.set(request),
                outgoing -> {
                    throw new AssertionError("unpersisted approval must not be reused");
                },
                rejected::add);
        assertNotNull(promptedAgain.get());
    }

    private static byte[] chunk(String cacheKey, byte[] bytes) {
        byte[] key = cacheKey.getBytes(StandardCharsets.UTF_8);
        return ByteBuffer.allocate(2 + 2 + key.length + 8 + 1 + bytes.length)
                .order(ByteOrder.BIG_ENDIAN)
                .putShort((short) LoaderHandshake.PROTOCOL_VERSION)
                .putShort((short) key.length)
                .put(key)
                .putLong(0)
                .put((byte) 1)
                .put(bytes)
                .array();
    }

    private static byte[] manifest(String permission, String content) {
        return """
                {"protocol":1,"bundles":[{
                  "owner":"example","id":"screen","version":"1",
                  "artifact":"client/screen.zip","sha256":"%s","size_bytes":%d,
                  "loaders":["fabric","neoforge","forge"],"content":["%s"],
                  "permissions":["%s"],"cache_key":"%s"
                }]}
                """.formatted(HASH, ARCHIVE.length, content, permission, CACHE_KEY)
                .getBytes(StandardCharsets.UTF_8);
    }
}
