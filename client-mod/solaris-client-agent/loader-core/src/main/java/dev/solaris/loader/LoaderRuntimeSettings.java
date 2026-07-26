package dev.solaris.loader;

import java.nio.file.Path;

public final class LoaderRuntimeSettings {
    public static final String LOADER_VERSION = "0.1.0";
    public static final String CACHE_DIRECTORY_PROPERTY = "solaris.loader.cacheDir";

    private LoaderRuntimeSettings() {
    }

    public static Path cacheDirectory() {
        String configured = System.getProperty(CACHE_DIRECTORY_PROPERTY);
        if (configured == null || configured.isBlank()) {
            String userHome = System.getProperty("user.home");
            if (userHome == null || userHome.isBlank()) {
                throw new IllegalArgumentException(
                        "Solaris Loader needs user.home or " + CACHE_DIRECTORY_PROPERTY);
            }
            return Path.of(userHome, ".solaris", "loader-cache").toAbsolutePath().normalize();
        }
        return Path.of(configured).toAbsolutePath().normalize();
    }

    public static Path permissionDecisionsFile() {
        return cacheDirectory().resolve("permissions.properties");
    }
}
