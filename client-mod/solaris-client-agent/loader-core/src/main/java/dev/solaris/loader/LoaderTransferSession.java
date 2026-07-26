package dev.solaris.loader;

import com.google.gson.Gson;
import com.google.gson.annotations.SerializedName;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.nio.file.AtomicMoveNotSupportedException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.HexFormat;
import java.util.List;
import java.util.Optional;

final class LoaderTransferSession {
    private static final int MAX_CACHE_KEY_BYTES = 256;
    private static final Gson GSON = new Gson();

    private final LoaderManifest manifest;
    private final LoaderEnvironment environment;
    private final Path cacheDirectory;
    private final List<LoaderBundle> missing;
    private int nextMissing;
    private StagedArtifact staged;

    private LoaderTransferSession(
            LoaderManifest manifest,
            LoaderEnvironment environment,
            Path cacheDirectory,
            List<LoaderBundle> missing) {
        this.manifest = manifest;
        this.environment = environment;
        this.cacheDirectory = cacheDirectory;
        this.missing = missing;
    }

    static LoaderTransferSession begin(
            byte[] payload,
            LoaderEnvironment environment,
            Path cacheDirectory) {
        LoaderManifest manifest =
                LoaderHandshake.validateTransferManifest(payload, environment);
        List<LoaderBundle> missing = new ArrayList<>();
        for (LoaderBundle bundle : manifest.bundles()) {
            if (!isVerified(cachePath(cacheDirectory, bundle), bundle)) {
                missing.add(bundle);
            }
        }
        return new LoaderTransferSession(
                manifest,
                environment,
                cacheDirectory.toAbsolutePath().normalize(),
                List.copyOf(missing));
    }

    LoaderOutgoing nextOutgoing() {
        if (nextMissing >= missing.size()) {
            LoaderActivatedContent content =
                    LoaderContentArchive.activate(manifest, cacheDirectory);
            return new LoaderOutgoing(
                    LoaderOutgoing.Kind.ACKNOWLEDGEMENT,
                    LoaderHandshake.acknowledgement(manifest, environment, content),
                    content);
        }
        LoaderBundle bundle = missing.get(nextMissing);
        if (staged == null) {
            staged = StagedArtifact.open(cachePath(cacheDirectory, bundle), bundle);
        }
        byte[] request = GSON.toJson(new ArtifactRequest(
                        LoaderHandshake.PROTOCOL_VERSION,
                        bundle.cacheKey()))
                .getBytes(StandardCharsets.UTF_8);
        return new LoaderOutgoing(LoaderOutgoing.Kind.REQUEST, request);
    }

    Optional<LoaderOutgoing> acceptArtifact(byte[] payload) {
        if (staged == null || nextMissing >= missing.size()) {
            throw new IllegalArgumentException("received an unexpected Solaris Loader artifact");
        }
        ArtifactChunk chunk = ArtifactChunk.decode(payload);
        LoaderBundle bundle = missing.get(nextMissing);
        if (!bundle.cacheKey().equals(chunk.cacheKey())) {
            staged.abort();
            throw new IllegalArgumentException("artifact cache identity does not match its request");
        }
        boolean complete = staged.write(chunk);
        if (!complete) {
            return Optional.empty();
        }
        staged = null;
        nextMissing++;
        return Optional.of(nextOutgoing());
    }

    void abort() {
        if (staged != null) {
            staged.abort();
            staged = null;
        }
    }

    static Path cachePath(Path root, LoaderBundle bundle) {
        Path normalizedRoot = root.toAbsolutePath().normalize();
        Path path = normalizedRoot
                .resolve(bundle.owner())
                .resolve(bundle.id())
                .resolve(bundle.version())
                .resolve(bundle.sha256() + ".bundle")
                .normalize();
        if (!path.startsWith(normalizedRoot)) {
            throw new IllegalArgumentException("bundle cache identity escapes the Loader cache");
        }
        return path;
    }

    private static boolean isVerified(Path path, LoaderBundle bundle) {
        try {
            if (!Files.isRegularFile(path) || Files.size(path) != bundle.sizeBytes()) {
                return false;
            }
            MessageDigest digest = sha256();
            try (InputStream input = Files.newInputStream(path)) {
                byte[] buffer = new byte[32 * 1024];
                long total = 0;
                int read;
                while ((read = input.read(buffer)) != -1) {
                    total += read;
                    if (total > bundle.sizeBytes()) {
                        return false;
                    }
                    digest.update(buffer, 0, read);
                }
                return total == bundle.sizeBytes()
                        && HexFormat.of().formatHex(digest.digest()).equals(bundle.sha256());
            }
        } catch (IOException error) {
            return false;
        }
    }

    private static MessageDigest sha256() {
        try {
            return MessageDigest.getInstance("SHA-256");
        } catch (NoSuchAlgorithmException error) {
            throw new IllegalStateException("JVM does not provide SHA-256", error);
        }
    }

    private record ArtifactRequest(
            int protocol,
            @SerializedName("cache_key") String cacheKey) {
    }

    private record ArtifactChunk(
            String cacheKey,
            long offset,
            boolean last,
            byte[] bytes) {
        private static ArtifactChunk decode(byte[] payload) {
            ByteBuffer buffer = ByteBuffer.wrap(payload).order(ByteOrder.BIG_ENDIAN);
            if (buffer.remaining() < 2 + 2 + 8 + 1 + 1) {
                throw new IllegalArgumentException("Solaris Loader artifact chunk is truncated");
            }
            int protocol = Short.toUnsignedInt(buffer.getShort());
            if (protocol != LoaderHandshake.PROTOCOL_VERSION) {
                throw new IllegalArgumentException("unsupported Solaris Loader artifact protocol");
            }
            int cacheKeyLength = Short.toUnsignedInt(buffer.getShort());
            if (cacheKeyLength < 1
                    || cacheKeyLength > MAX_CACHE_KEY_BYTES
                    || buffer.remaining() < cacheKeyLength + 8 + 1 + 1) {
                throw new IllegalArgumentException("invalid Solaris Loader artifact cache identity");
            }
            byte[] cacheKey = new byte[cacheKeyLength];
            buffer.get(cacheKey);
            long offset = buffer.getLong();
            int flags = Byte.toUnsignedInt(buffer.get());
            if (offset < 0 || flags > 1 || !buffer.hasRemaining()) {
                throw new IllegalArgumentException("invalid Solaris Loader artifact chunk header");
            }
            byte[] bytes = new byte[buffer.remaining()];
            buffer.get(bytes);
            return new ArtifactChunk(
                    new String(cacheKey, StandardCharsets.UTF_8),
                    offset,
                    flags == 1,
                    bytes);
        }

        @Override
        public byte[] bytes() {
            return bytes.clone();
        }
    }

    private static final class StagedArtifact {
        private final LoaderBundle bundle;
        private final Path finalPath;
        private final Path stagingPath;
        private final OutputStream output;
        private final MessageDigest digest = sha256();
        private long written;

        private StagedArtifact(
                LoaderBundle bundle,
                Path finalPath,
                Path stagingPath,
                OutputStream output) {
            this.bundle = bundle;
            this.finalPath = finalPath;
            this.stagingPath = stagingPath;
            this.output = output;
        }

        private static StagedArtifact open(Path finalPath, LoaderBundle bundle) {
            try {
                Files.createDirectories(finalPath.getParent());
                Path staging = Files.createTempFile(
                        finalPath.getParent(),
                        bundle.sha256() + ".",
                        ".part");
                return new StagedArtifact(
                        bundle,
                        finalPath,
                        staging,
                        Files.newOutputStream(staging));
            } catch (IOException error) {
                throw new IllegalArgumentException("creating Solaris Loader staging file", error);
            }
        }

        private boolean write(ArtifactChunk chunk) {
            try {
                if (chunk.offset() != written) {
                    throw new IllegalArgumentException(
                            "artifact chunk offset does not match staged length");
                }
                byte[] bytes = chunk.bytes();
                long next = Math.addExact(written, bytes.length);
                if (next > bundle.sizeBytes()) {
                    throw new IllegalArgumentException(
                            "artifact bytes exceed the declared bundle size");
                }
                output.write(bytes);
                digest.update(bytes);
                written = next;
                if (!chunk.last()) {
                    if (written == bundle.sizeBytes()) {
                        throw new IllegalArgumentException(
                                "artifact reached its declared size without a final chunk");
                    }
                    return false;
                }
                if (written != bundle.sizeBytes()) {
                    throw new IllegalArgumentException(
                            "final artifact size does not match the manifest");
                }
                String actual = HexFormat.of().formatHex(digest.digest());
                if (!actual.equals(bundle.sha256())) {
                    throw new IllegalArgumentException(
                            "final artifact SHA-256 does not match the manifest");
                }
                output.close();
                try {
                    Files.move(
                            stagingPath,
                            finalPath,
                            StandardCopyOption.ATOMIC_MOVE,
                            StandardCopyOption.REPLACE_EXISTING);
                } catch (AtomicMoveNotSupportedException error) {
                    throw new IllegalArgumentException(
                            "cache filesystem does not support atomic Loader publication",
                            error);
                }
                return true;
            } catch (IOException | ArithmeticException | IllegalArgumentException error) {
                abort();
                if (error instanceof IllegalArgumentException argument) {
                    throw argument;
                }
                throw new IllegalArgumentException("writing Solaris Loader artifact", error);
            }
        }

        private void abort() {
            try {
                output.close();
            } catch (IOException ignored) {
            }
            try {
                Files.deleteIfExists(stagingPath);
            } catch (IOException ignored) {
            }
        }
    }
}
