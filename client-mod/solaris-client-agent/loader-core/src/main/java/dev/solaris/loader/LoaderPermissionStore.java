package dev.solaris.loader;

import java.io.IOException;
import java.io.Reader;
import java.io.Writer;
import java.nio.charset.StandardCharsets;
import java.nio.file.AtomicMoveNotSupportedException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.Base64;
import java.util.List;
import java.util.Optional;
import java.util.Properties;
import java.util.stream.Collectors;

final class LoaderPermissionStore {
    private static final long MAX_FILE_BYTES = 256 * 1024;
    private static final int MAX_DECISIONS = 256;

    private final Path path;
    private final Properties decisions = new Properties();

    LoaderPermissionStore(Path path) {
        this.path = path.toAbsolutePath().normalize();
        load();
    }

    synchronized Optional<Boolean> decision(LoaderPermissionRequest request) {
        String value = decisions.getProperty(key(request));
        return switch (value) {
            case null -> Optional.empty();
            case "allow" -> Optional.of(true);
            case "deny" -> Optional.of(false);
            default -> throw new IllegalArgumentException(
                    "Solaris Loader permission store contains an invalid decision");
        };
    }

    synchronized void record(LoaderPermissionRequest request, boolean allowed) {
        String key = key(request);
        if (!decisions.containsKey(key) && decisions.size() >= MAX_DECISIONS) {
            throw new IllegalArgumentException(
                    "Solaris Loader permission store exceeds " + MAX_DECISIONS + " decisions");
        }
        Object previous = decisions.setProperty(key, allowed ? "allow" : "deny");
        try {
            save();
        } catch (IllegalArgumentException error) {
            if (previous == null) {
                decisions.remove(key);
            } else {
                decisions.put(key, previous);
            }
            throw error;
        }
    }

    private void load() {
        if (!Files.exists(path)) {
            return;
        }
        try {
            if (!Files.isRegularFile(path) || Files.size(path) > MAX_FILE_BYTES) {
                throw new IllegalArgumentException(
                        "Solaris Loader permission store is not a bounded regular file");
            }
            try (Reader reader = Files.newBufferedReader(path, StandardCharsets.UTF_8)) {
                decisions.load(reader);
            }
        } catch (IOException error) {
            throw new IllegalArgumentException("reading Solaris Loader permission store", error);
        }
        if (decisions.size() > MAX_DECISIONS) {
            throw new IllegalArgumentException(
                    "Solaris Loader permission store exceeds " + MAX_DECISIONS + " decisions");
        }
        for (Object value : decisions.values()) {
            if (!value.equals("allow") && !value.equals("deny")) {
                throw new IllegalArgumentException(
                        "Solaris Loader permission store contains an invalid decision");
            }
        }
    }

    private void save() {
        Path directory = path.getParent();
        Path staging = null;
        try {
            Files.createDirectories(directory);
            staging = Files.createTempFile(directory, ".permissions.", ".tmp");
            try (Writer writer = Files.newBufferedWriter(staging, StandardCharsets.UTF_8)) {
                decisions.store(writer, "Solaris Loader per-server permission decisions");
            }
            try {
                Files.move(
                        staging,
                        path,
                        StandardCopyOption.ATOMIC_MOVE,
                        StandardCopyOption.REPLACE_EXISTING);
            } catch (AtomicMoveNotSupportedException error) {
                throw new IllegalArgumentException(
                        "permission filesystem does not support atomic publication",
                        error);
            }
        } catch (IOException error) {
            throw new IllegalArgumentException("writing Solaris Loader permission store", error);
        } finally {
            if (staging != null) {
                try {
                    Files.deleteIfExists(staging);
                } catch (IOException ignored) {
                }
            }
        }
    }

    private static String key(LoaderPermissionRequest request) {
        String server = Base64.getUrlEncoder()
                .withoutPadding()
                .encodeToString(request.serverIdentity().getBytes(StandardCharsets.UTF_8));
        String permissions = orderedPermissions(request.permissions()).stream()
                .map(LoaderPermission::wireName)
                .collect(Collectors.joining(","));
        return server + "|" + permissions;
    }

    static List<LoaderPermission> orderedPermissions(List<LoaderPermission> permissions) {
        return permissions.stream()
                .distinct()
                .sorted((left, right) -> left.wireName().compareTo(right.wireName()))
                .toList();
    }
}
