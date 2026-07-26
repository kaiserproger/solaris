package dev.solaris.loader;

import java.util.List;

public record LoaderPermissionRequest(
        String serverIdentity,
        List<LoaderPermission> permissions) {
    public LoaderPermissionRequest {
        permissions = List.copyOf(permissions);
    }
}
