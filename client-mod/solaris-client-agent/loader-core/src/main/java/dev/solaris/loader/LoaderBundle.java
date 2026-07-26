package dev.solaris.loader;

import com.google.gson.annotations.SerializedName;
import java.util.List;

public record LoaderBundle(
        String owner,
        String id,
        String version,
        String artifact,
        String sha256,
        @SerializedName("size_bytes") long sizeBytes,
        List<LoaderPlatform> loaders,
        List<LoaderContentKind> content,
        List<LoaderPermission> permissions,
        @SerializedName("cache_key") String cacheKey) {
}
