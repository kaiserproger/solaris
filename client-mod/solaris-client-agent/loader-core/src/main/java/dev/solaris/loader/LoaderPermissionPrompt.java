package dev.solaris.loader;

import java.util.function.Consumer;

@FunctionalInterface
public interface LoaderPermissionPrompt {
    void request(LoaderPermissionRequest request, Consumer<Boolean> decision);
}
