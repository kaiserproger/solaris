package dev.solaris.loader;

import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.function.BooleanSupplier;
import java.util.function.Consumer;
import java.util.function.Supplier;

public final class LoaderPermissionController {
    private static final Map<Path, LoaderPermissionStore> STORES = new HashMap<>();

    private final LoaderClientTransport transport = new LoaderClientTransport();
    private final LoaderPermissionStore store;
    private final Supplier<List<Integer>> carrierBlockStateIds;
    private long generation;
    private boolean expectingArtifact;

    public LoaderPermissionController(Path decisionsFile) {
        this(
                decisionsFile,
                () -> {
                    throw new IllegalStateException(
                            "Solaris Loader block carrier state is unavailable");
                });
    }

    public LoaderPermissionController(
            Path decisionsFile,
            Supplier<List<Integer>> carrierBlockStateIds) {
        store = sharedStore(decisionsFile);
        this.carrierBlockStateIds = carrierBlockStateIds;
    }

    public synchronized void acceptManifest(
            byte[] payload,
            LoaderPlatform platform,
            String loaderVersion,
            String serverIdentity,
            Path cacheDirectory,
            BooleanSupplier connectionActive,
            LoaderPermissionPrompt prompt,
            Consumer<LoaderOutgoing> outgoing,
            Consumer<String> rejected) {
        transport.abort();
        expectingArtifact = false;
        long currentGeneration = ++generation;
        if (!connectionActive.getAsBoolean()) {
            rejected.accept("connection ended before the permission decision");
            return;
        }
        LoaderManifest manifest =
                LoaderHandshake.inspectManifest(payload, platform, loaderVersion);
        LoaderPermissionRequest request = new LoaderPermissionRequest(
                normalizeServerIdentity(serverIdentity),
                requiredPermissions(manifest));
        var stored = store.decision(request);
        if (stored.isPresent()) {
            if (stored.orElseThrow()) {
                startTransfer(
                        currentGeneration,
                        payload,
                        platform,
                        loaderVersion,
                        request,
                        cacheDirectory,
                        connectionActive,
                        outgoing,
                        rejected);
            } else {
                rejected.accept("permissions were denied for " + request.serverIdentity());
            }
            return;
        }
        try {
            prompt.request(request, allowed -> resolvePrompt(
                    currentGeneration,
                    payload,
                    platform,
                    loaderVersion,
                    request,
                    cacheDirectory,
                    connectionActive,
                    outgoing,
                    rejected,
                    allowed));
        } catch (RuntimeException error) {
            rejected.accept("permission prompt failed: " + error.getMessage());
        }
    }

    public synchronized java.util.Optional<LoaderOutgoing> acceptArtifact(
            byte[] payload,
            BooleanSupplier connectionActive) {
        if (!connectionActive.getAsBoolean()) {
            transport.abort();
            expectingArtifact = false;
            generation++;
            throw new IllegalArgumentException(
                    "received a Solaris Loader artifact after its connection ended");
        }
        if (!expectingArtifact) {
            throw new IllegalArgumentException(
                    "received a Solaris Loader artifact without an allowed request");
        }
        var next = transport.acceptArtifact(payload);
        if (next.isPresent()) {
            expectingArtifact = next.orElseThrow().kind() == LoaderOutgoing.Kind.REQUEST;
        }
        return next;
    }

    private synchronized void resolvePrompt(
            long currentGeneration,
            byte[] payload,
            LoaderPlatform platform,
            String loaderVersion,
            LoaderPermissionRequest request,
            Path cacheDirectory,
            BooleanSupplier connectionActive,
            Consumer<LoaderOutgoing> outgoing,
            Consumer<String> rejected,
            boolean allowed) {
        if (currentGeneration != generation) {
            return;
        }
        if (!connectionActive.getAsBoolean()) {
            transport.abort();
            expectingArtifact = false;
            generation++;
            return;
        }
        try {
            store.record(request, allowed);
            if (!allowed) {
                rejected.accept("permissions were denied for " + request.serverIdentity());
                return;
            }
            startTransfer(
                    currentGeneration,
                    payload,
                    platform,
                    loaderVersion,
                    request,
                    cacheDirectory,
                    connectionActive,
                    outgoing,
                    rejected);
        } catch (IllegalArgumentException error) {
            rejected.accept(error.getMessage());
        }
    }

    private void startTransfer(
            long currentGeneration,
            byte[] payload,
            LoaderPlatform platform,
            String loaderVersion,
            LoaderPermissionRequest request,
            Path cacheDirectory,
            BooleanSupplier connectionActive,
            Consumer<LoaderOutgoing> outgoing,
            Consumer<String> rejected) {
        if (currentGeneration != generation || !connectionActive.getAsBoolean()) {
            return;
        }
        try {
            LoaderOutgoing next = transport.acceptManifest(
                    payload,
                    new AllowedEnvironment(
                            platform,
                            loaderVersion,
                            Set.copyOf(request.permissions()),
                            carrierBlockStateIds),
                    cacheDirectory);
            expectingArtifact = next.kind() == LoaderOutgoing.Kind.REQUEST;
            if (!connectionActive.getAsBoolean()) {
                transport.abort();
                expectingArtifact = false;
                generation++;
                return;
            }
            outgoing.accept(next);
        } catch (IllegalArgumentException error) {
            rejected.accept(error.getMessage());
        }
    }

    private static List<LoaderPermission> requiredPermissions(LoaderManifest manifest) {
        LinkedHashSet<LoaderPermission> permissions = new LinkedHashSet<>();
        for (LoaderBundle bundle : manifest.bundles()) {
            permissions.addAll(bundle.permissions());
        }
        return LoaderPermissionStore.orderedPermissions(new ArrayList<>(permissions));
    }

    private static String normalizeServerIdentity(String identity) {
        if (identity == null) {
            throw new IllegalArgumentException("Solaris Loader server identity is missing");
        }
        String normalized = identity.trim().toLowerCase(Locale.ROOT);
        if (normalized.isEmpty() || normalized.length() > 255) {
            throw new IllegalArgumentException(
                    "Solaris Loader server identity must contain 1..=255 characters");
        }
        for (int index = 0; index < normalized.length(); index++) {
            if (Character.isISOControl(normalized.charAt(index))) {
                throw new IllegalArgumentException(
                        "Solaris Loader server identity contains a control character");
            }
        }
        return normalized;
    }

    private static synchronized LoaderPermissionStore sharedStore(Path path) {
        Path normalized = path.toAbsolutePath().normalize();
        return STORES.computeIfAbsent(normalized, LoaderPermissionStore::new);
    }

    private record AllowedEnvironment(
            LoaderPlatform platform,
            String loaderVersion,
            Set<LoaderPermission> grantedPermissions,
            Supplier<List<Integer>> carrierBlockStateIdsSupplier) implements LoaderEnvironment {
        @Override
        public List<Integer> carrierBlockStateIds() {
            return carrierBlockStateIdsSupplier.get();
        }
    }
}
