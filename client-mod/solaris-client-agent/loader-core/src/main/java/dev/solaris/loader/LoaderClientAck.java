package dev.solaris.loader;

import com.google.gson.annotations.SerializedName;
import java.util.List;
import java.util.Map;

public record LoaderClientAck(
        int protocol,
        LoaderPlatform platform,
        @SerializedName("loader_version") String loaderVersion,
        @SerializedName("accepted_permissions") List<LoaderPermission> acceptedPermissions,
        @SerializedName("cached_bundles") List<String> cachedBundles,
        @SerializedName("carrier_block_state_ids") Map<String, Integer> carrierBlockStateIds) {
}
