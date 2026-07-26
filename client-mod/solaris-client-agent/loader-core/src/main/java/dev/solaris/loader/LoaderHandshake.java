package dev.solaris.loader;

import com.google.gson.Gson;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParseException;
import com.google.gson.JsonParser;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Set;

public final class LoaderHandshake {
    public static final int PROTOCOL_VERSION = 1;
    public static final int MAX_MANIFEST_BYTES = 32_767;
    private static final long MAX_BUNDLE_BYTES = 64L * 1024L * 1024L;
    private static final Gson GSON = new Gson();
    private static final Set<String> MANIFEST_FIELDS = Set.of("protocol", "bundles");
    private static final Set<String> BUNDLE_FIELDS = Set.of(
            "owner",
            "id",
            "version",
            "artifact",
            "sha256",
            "size_bytes",
            "loaders",
            "content",
            "permissions",
            "cache_key");

    private LoaderHandshake() {
    }

    static LoaderManifest validateTransferManifest(
            byte[] payload,
            LoaderEnvironment environment) {
        LoaderManifest manifest =
                validateManifest(payload, environment.platform(), environment.loaderVersion());
        for (LoaderBundle bundle : manifest.bundles()) {
            for (LoaderPermission permission : bundle.permissions()) {
                if (!environment.grantedPermissions().contains(permission)) {
                    throw new IllegalArgumentException("permission was not granted: " + permission);
                }
            }
        }
        return manifest;
    }

    static LoaderManifest inspectManifest(
            byte[] payload,
            LoaderPlatform platform,
            String loaderVersion) {
        return validateManifest(payload, platform, loaderVersion);
    }

    static byte[] acknowledgement(
            LoaderManifest manifest,
            LoaderEnvironment environment,
            LoaderActivatedContent content) {
        List<LoaderPermission> accepted = new ArrayList<>();
        List<String> cached = new ArrayList<>();
        for (LoaderBundle bundle : manifest.bundles()) {
            for (LoaderPermission permission : bundle.permissions()) {
                if (!accepted.contains(permission)) {
                    accepted.add(permission);
                }
            }
            cached.add(bundle.cacheKey());
        }
        Map<String, Integer> carrierBlockStateIds = new LinkedHashMap<>();
        List<String> blockIds = content.blocks().keySet().stream().sorted().toList();
        if (!blockIds.isEmpty()) {
            List<Integer> stateIds = environment.carrierBlockStateIds();
            if (stateIds.size() < blockIds.size()) {
                throw new IllegalArgumentException(
                        "Solaris Loader block carrier capacity is unavailable");
            }
            for (int index = 0; index < blockIds.size(); index++) {
                int stateId = stateIds.get(index);
                if (stateId < 0) {
                    throw new IllegalArgumentException(
                            "Solaris Loader block carrier state must be non-negative");
                }
                carrierBlockStateIds.put(blockIds.get(index), stateId);
            }
        }
        LoaderClientAck ack = new LoaderClientAck(
                PROTOCOL_VERSION,
                environment.platform(),
                environment.loaderVersion(),
                List.copyOf(accepted),
                List.copyOf(cached),
                Collections.unmodifiableMap(carrierBlockStateIds));
        return GSON.toJson(ack).getBytes(StandardCharsets.UTF_8);
    }

    private static LoaderManifest validateManifest(
            byte[] payload,
            LoaderPlatform platform,
            String loaderVersion) {
        if (payload.length == 0 || payload.length > MAX_MANIFEST_BYTES) {
            throw new IllegalArgumentException("loader manifest size is outside 1..=" + MAX_MANIFEST_BYTES);
        }
        LoaderManifest manifest;
        try {
            JsonElement document = JsonParser.parseString(new String(payload, StandardCharsets.UTF_8));
            validateClosedSchema(document);
            manifest = GSON.fromJson(document, LoaderManifest.class);
        } catch (JsonParseException error) {
            throw new IllegalArgumentException("loader manifest is malformed", error);
        }
        validateManifest(manifest, platform, loaderVersion);
        return manifest;
    }

    private static void validateClosedSchema(JsonElement document) {
        if (!document.isJsonObject()) {
            throw new IllegalArgumentException("loader manifest must be a JSON object");
        }
        JsonObject manifest = document.getAsJsonObject();
        rejectUnknownFields(manifest, MANIFEST_FIELDS, "loader manifest");
        JsonElement bundles = manifest.get("bundles");
        if (bundles == null || !bundles.isJsonArray()) {
            return;
        }
        for (JsonElement bundle : bundles.getAsJsonArray()) {
            if (!bundle.isJsonObject()) {
                throw new IllegalArgumentException("loader bundle must be a JSON object");
            }
            rejectUnknownFields(bundle.getAsJsonObject(), BUNDLE_FIELDS, "loader bundle");
        }
    }

    private static void rejectUnknownFields(JsonObject object, Set<String> allowed, String name) {
        for (String field : object.keySet()) {
            if (!allowed.contains(field)) {
                throw new IllegalArgumentException(name + " contains unknown field " + field);
            }
        }
    }

    private static void validateManifest(
            LoaderManifest manifest,
            LoaderPlatform platform,
            String loaderVersion) {
        if (manifest == null || manifest.protocol() != PROTOCOL_VERSION) {
            throw new IllegalArgumentException("unsupported loader protocol");
        }
        if (platform == null) {
            throw new IllegalArgumentException("loader platform is missing");
        }
        requireText(loaderVersion, 64, "loader version");
        if (manifest.bundles() == null || manifest.bundles().isEmpty()) {
            throw new IllegalArgumentException("loader manifest must contain at least one bundle");
        }
        Set<String> cacheKeys = new HashSet<>();
        for (LoaderBundle bundle : manifest.bundles()) {
            validateBundle(bundle, platform);
            if (!cacheKeys.add(bundle.cacheKey())) {
                throw new IllegalArgumentException("duplicate loader cache key " + bundle.cacheKey());
            }
        }
    }

    private static void validateBundle(
            LoaderBundle bundle,
            LoaderPlatform platform) {
        if (bundle == null) {
            throw new IllegalArgumentException("loader bundle is missing");
        }
        requireLiteral(bundle.owner(), 64, "bundle owner");
        requireLiteral(bundle.id(), 48, "bundle id");
        requireLiteral(bundle.version(), 32, "bundle version");
        requireArtifactPath(bundle.artifact());
        if (bundle.sha256() == null || !bundle.sha256().matches("[0-9a-f]{64}")) {
            throw new IllegalArgumentException("bundle sha256 must be 64 lowercase hexadecimal characters");
        }
        if (bundle.sizeBytes() <= 0 || bundle.sizeBytes() > MAX_BUNDLE_BYTES) {
            throw new IllegalArgumentException("bundle size is outside 1..=" + MAX_BUNDLE_BYTES);
        }
        requireList(bundle.loaders(), "bundle loaders");
        requireList(bundle.content(), "bundle content");
        requireList(bundle.permissions(), "bundle permissions");
        if (!bundle.loaders().contains(platform)) {
            throw new IllegalArgumentException("bundle does not support " + platform);
        }
        for (LoaderContentKind content : bundle.content()) {
            LoaderPermission required = requiredPermission(content);
            if (!bundle.permissions().contains(required)) {
                throw new IllegalArgumentException("bundle content is missing permission " + required);
            }
        }
        String expectedCacheKey =
                bundle.owner() + ":" + bundle.id() + "/" + bundle.version() + "/" + bundle.sha256();
        if (!expectedCacheKey.equals(bundle.cacheKey())) {
            throw new IllegalArgumentException("bundle cache key does not match its identity");
        }
    }

    private static LoaderPermission requiredPermission(LoaderContentKind content) {
        if (content == null) {
            throw new IllegalArgumentException("bundle content contains an unknown value");
        }
        return switch (content) {
            case BLOCKS -> LoaderPermission.REGISTER_BLOCKS;
            case ITEMS -> LoaderPermission.REGISTER_ITEMS;
            case SCREENS -> LoaderPermission.OPEN_SCREENS;
            case ASSETS -> LoaderPermission.LOAD_ASSETS;
            case INTERACTIONS -> LoaderPermission.SEND_INTERACTIONS;
        };
    }

    private static void requireText(String value, int maxBytes, String name) {
        if (value == null || value.isEmpty() || value.getBytes(StandardCharsets.UTF_8).length > maxBytes) {
            throw new IllegalArgumentException(name + " must contain 1..=" + maxBytes + " bytes");
        }
    }

    private static void requireLiteral(String value, int maxBytes, String name) {
        requireText(value, maxBytes, name);
        if (value.equals(".") || value.equals("..")) {
            throw new IllegalArgumentException(name + " contains an invalid path segment");
        }
        for (int index = 0; index < value.length(); index++) {
            char character = value.charAt(index);
            if (!(character >= 'a' && character <= 'z')
                    && !(character >= 'A' && character <= 'Z')
                    && !(character >= '0' && character <= '9')
                    && character != '_'
                    && character != '.'
                    && character != '-') {
                throw new IllegalArgumentException(name + " contains invalid characters");
            }
        }
    }

    private static void requireArtifactPath(String path) {
        requireText(path, 160, "bundle artifact");
        if (path.startsWith("/") || path.contains("\\") || path.endsWith("/")) {
            throw new IllegalArgumentException("bundle artifact must be a relative canonical path");
        }
        for (int index = 0; index < path.length(); index++) {
            char character = path.charAt(index);
            if (!(character >= 'a' && character <= 'z')
                    && !(character >= 'A' && character <= 'Z')
                    && !(character >= '0' && character <= '9')
                    && character != '_'
                    && character != '.'
                    && character != '/'
                    && character != '-') {
                throw new IllegalArgumentException("bundle artifact must be a relative canonical path");
            }
        }
        for (String segment : path.split("/", -1)) {
            if (segment.isEmpty() || segment.equals(".") || segment.equals("..")) {
                throw new IllegalArgumentException("bundle artifact must be a relative canonical path");
            }
        }
    }

    private static <T> void requireList(List<T> values, String name) {
        if (values == null || values.isEmpty() || values.contains(null)) {
            throw new IllegalArgumentException(name + " must be non-empty and contain known values");
        }
        if (new HashSet<>(values).size() != values.size()) {
            throw new IllegalArgumentException(name + " contains duplicate values");
        }
    }
}
