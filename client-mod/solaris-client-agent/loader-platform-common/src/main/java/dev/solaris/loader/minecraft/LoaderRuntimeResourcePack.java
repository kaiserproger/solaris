package dev.solaris.loader.minecraft;

import dev.solaris.loader.LoaderActivatedContent;
import dev.solaris.loader.LoaderAssetDefinition;
import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.BooleanSupplier;
import net.minecraft.client.Minecraft;
import net.minecraft.network.chat.Component;
import net.minecraft.resources.Identifier;
import net.minecraft.server.packs.PackLocationInfo;
import net.minecraft.server.packs.PackResources;
import net.minecraft.server.packs.PackSelectionConfig;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.repository.Pack;
import net.minecraft.server.packs.repository.PackCompatibility;
import net.minecraft.server.packs.repository.PackSource;
import net.minecraft.server.packs.repository.RepositorySource;
import net.minecraft.server.packs.metadata.MetadataSectionType;
import net.minecraft.server.packs.resources.IoSupplier;
import net.minecraft.server.packs.resources.ResourceManager;
import net.minecraft.world.flag.FeatureFlagSet;

/**
 * One transient, connection-owned client resource pack backed by verified
 * Loader asset bytes.
 */
public final class LoaderRuntimeResourcePack {
    public static final String PACK_ID = "solaris_loader/runtime";
    private static final PackLocationInfo LOCATION = new PackLocationInfo(
            PACK_ID,
            Component.literal("Solaris Loader runtime content"),
            PackSource.SERVER,
            Optional.empty());
    private static final PackSelectionConfig SELECTION =
            new PackSelectionConfig(true, Pack.Position.TOP, true);

    private final AtomicLong generations = new AtomicLong();
    private final AtomicReference<Mount> mounted = new AtomicReference<>();

    public RepositorySource repositorySource() {
        return output -> {
            Mount current = mounted.get();
            if (current != null && current.isActive() && !current.resources().isEmpty()) {
                output.accept(pack(current.resources()));
            }
        };
    }

    public long publish(
            Object origin,
            BooleanSupplier originActive,
            LoaderActivatedContent content) {
        if (!originActive.getAsBoolean()) {
            throw new IllegalStateException(
                    "Loader resource connection is no longer active");
        }
        LoaderMinecraftItem.validate(content);
        LoaderMinecraftBlock.validate(content);
        long generation = generations.incrementAndGet();
        mounted.set(new Mount(
                generation,
                origin,
                originActive,
                resources(content)));
        return generation;
    }

    public boolean clear(Object origin) {
        while (true) {
            Mount current = mounted.get();
            if (current == null || current.origin() != origin) {
                return false;
            }
            if (mounted.compareAndSet(current, null)) {
                generations.incrementAndGet();
                return true;
            }
        }
    }

    public boolean owns(Object origin) {
        Mount current = mounted.get();
        return current != null && current.origin() == origin && current.isActive();
    }

    public CompletableFuture<Void> mount(
            Minecraft client,
            Object origin,
            BooleanSupplier originActive,
            LoaderActivatedContent content) {
        CompletableFuture<Void> result = new CompletableFuture<>();
        client.execute(() -> {
            try {
                Mount previous = mounted.get();
                long generation = publish(origin, originActive, content);
                reloadForMount(client, origin, generation, previous)
                        .whenCompleteAsync((unused, error) -> {
                            if (error == null) {
                                result.complete(null);
                            } else {
                                result.completeExceptionally(error);
                            }
                        }, client);
            } catch (RuntimeException error) {
                result.completeExceptionally(error);
            }
        });
        return result;
    }

    public CompletableFuture<Boolean> unmount(
            Minecraft client,
            Object origin,
            Runnable afterClear) {
        CompletableFuture<Boolean> result = new CompletableFuture<>();
        client.execute(() -> {
            Mount previous = mounted.get();
            if (previous == null
                    || previous.origin() != origin
                    || !mounted.compareAndSet(previous, null)) {
                result.complete(false);
                return;
            }
            generations.incrementAndGet();
            afterClear.run();
            reloadForRemoval(client, previous)
                    .whenCompleteAsync((unused, error) -> {
                        if (error == null) {
                            result.complete(true);
                        } else {
                            result.completeExceptionally(error);
                        }
                    }, client);
        });
        return result;
    }

    private CompletableFuture<Void> reloadForMount(
            Minecraft client,
            Object origin,
            long generation,
            Mount previous) {
        return client.reloadResourcePacks().thenComposeAsync(unused -> {
            if (mountedResourcesAreVisible(
                    client.getResourceManager(), origin, generation, previous)) {
                return CompletableFuture.completedFuture(null);
            }
            return client.reloadResourcePacks().thenRunAsync(() -> {
                if (!mountedResourcesAreVisible(
                        client.getResourceManager(), origin, generation, previous)) {
                    throw new IllegalStateException(
                            "Loader resources were not applied to Minecraft");
                }
            }, client);
        }, client);
    }

    private CompletableFuture<Void> reloadForRemoval(
            Minecraft client,
            Mount previous) {
        return client.reloadResourcePacks().thenComposeAsync(unused -> {
            if (mounted.get() != null || resourcesAreAbsent(
                    client.getResourceManager(), previous.resources().keySet())) {
                return CompletableFuture.completedFuture(null);
            }
            return client.reloadResourcePacks().thenRunAsync(() -> {
                if (mounted.get() == null && !resourcesAreAbsent(
                        client.getResourceManager(), previous.resources().keySet())) {
                    throw new IllegalStateException(
                            "Loader resources remained mounted after disconnect");
                }
            }, client);
        }, client);
    }

    private boolean mountedResourcesAreVisible(
            ResourceManager manager,
            Object origin,
            long generation,
            Mount previous) {
        Mount current = mounted.get();
        if (current == null
                || current.origin() != origin
                || current.generation() != generation
                || !current.isActive()) {
            return false;
        }
        if (previous != null) {
            for (Identifier old : previous.resources().keySet()) {
                if (!current.resources().containsKey(old)
                        && hasLoaderResource(manager, old)) {
                    return false;
                }
            }
        }
        for (Map.Entry<Identifier, byte[]> entry : current.resources().entrySet()) {
            Optional<net.minecraft.server.packs.resources.Resource> resource =
                    manager.getResource(entry.getKey())
                            .filter(found -> PACK_ID.equals(found.sourcePackId()));
            if (resource.isEmpty()) {
                return false;
            }
            try (InputStream stream = resource.orElseThrow().open()) {
                if (!Arrays.equals(entry.getValue(), stream.readAllBytes())) {
                    return false;
                }
            } catch (IOException error) {
                throw new IllegalStateException(
                        "reading mounted Loader resource " + entry.getKey(),
                        error);
            }
        }
        return true;
    }

    private static boolean resourcesAreAbsent(
            ResourceManager manager,
            Set<Identifier> resources) {
        return resources.stream().noneMatch(id -> hasLoaderResource(manager, id));
    }

    private static boolean hasLoaderResource(
            ResourceManager manager,
            Identifier id) {
        return manager.getResourceStack(id).stream()
                .anyMatch(resource -> PACK_ID.equals(resource.sourcePackId()));
    }

    private static Map<Identifier, byte[]> resources(
            LoaderActivatedContent content) {
        Map<Identifier, byte[]> resources = new LinkedHashMap<>();
        for (LoaderAssetDefinition asset : content.assets().values()) {
            Identifier location = resourceLocation(asset.archivePath());
            if (resources.putIfAbsent(location, asset.bytes()) != null) {
                throw new IllegalArgumentException(
                        "duplicate Loader resource path " + location);
            }
        }
        for (Map.Entry<Identifier, byte[]> entry
                : LoaderMinecraftBlock.generatedResources(content).entrySet()) {
            if (resources.putIfAbsent(entry.getKey(), entry.getValue()) != null) {
                throw new IllegalArgumentException(
                        "Loader asset collides with generated block resource "
                                + entry.getKey());
            }
        }
        return Map.copyOf(resources);
    }

    private static Identifier resourceLocation(String archivePath) {
        String prefix = "assets/";
        if (!archivePath.startsWith(prefix)) {
            throw new IllegalArgumentException(
                    "Loader asset is outside the Minecraft assets root: " + archivePath);
        }
        int namespaceEnd = archivePath.indexOf('/', prefix.length());
        if (namespaceEnd < 0 || namespaceEnd == archivePath.length() - 1) {
            throw new IllegalArgumentException(
                    "Loader asset path has no namespace or resource path: " + archivePath);
        }
        String namespace = archivePath.substring(prefix.length(), namespaceEnd);
        String path = archivePath.substring(namespaceEnd + 1);
        Identifier location = Identifier.tryBuild(namespace, path);
        if (location == null) {
            throw new IllegalArgumentException(
                    "Loader asset path is not a Minecraft resource id: " + archivePath);
        }
        return location;
    }

    private static Pack pack(Map<Identifier, byte[]> resources) {
        Pack.ResourcesSupplier supplier = new Pack.ResourcesSupplier() {
            @Override
            public PackResources openPrimary(PackLocationInfo location) {
                return new MemoryPackResources(location, resources);
            }

            @Override
            public PackResources openFull(
                    PackLocationInfo location,
                    Pack.Metadata metadata) {
                return new MemoryPackResources(location, resources);
            }
        };
        Pack.Metadata metadata = new Pack.Metadata(
                Component.literal("Verified Solaris Loader assets"),
                PackCompatibility.COMPATIBLE,
                FeatureFlagSet.of(),
                List.of());
        return new Pack(LOCATION, supplier, metadata, SELECTION);
    }

    private record Mount(
            long generation,
            Object origin,
            BooleanSupplier originActive,
            Map<Identifier, byte[]> resources) {
        boolean isActive() {
            try {
                return originActive.getAsBoolean();
            } catch (RuntimeException ignored) {
                return false;
            }
        }
    }

    private static final class MemoryPackResources implements PackResources {
        private final PackLocationInfo location;
        private final Map<Identifier, byte[]> resources;
        private final Map<String, List<Identifier>> namespaces;

        private MemoryPackResources(
                PackLocationInfo location,
                Map<Identifier, byte[]> resources) {
            this.location = location;
            this.resources = resources;
            Map<String, List<Identifier>> grouped = new HashMap<>();
            for (Identifier id : resources.keySet()) {
                grouped.computeIfAbsent(id.getNamespace(), ignored -> new ArrayList<>())
                        .add(id);
            }
            this.namespaces = Map.copyOf(grouped);
        }

        @Override
        public IoSupplier<InputStream> getRootResource(String... path) {
            return null;
        }

        @Override
        public IoSupplier<InputStream> getResource(
                PackType type,
                Identifier location) {
            if (type != PackType.CLIENT_RESOURCES) {
                return null;
            }
            byte[] bytes = resources.get(location);
            return bytes == null ? null : () -> new ByteArrayInputStream(bytes);
        }

        @Override
        public void listResources(
                PackType type,
                String namespace,
                String path,
                ResourceOutput output) {
            if (type != PackType.CLIENT_RESOURCES) {
                return;
            }
            for (Identifier id : namespaces.getOrDefault(namespace, List.of())) {
                if (id.getPath().startsWith(path)) {
                    byte[] bytes = resources.get(id);
                    output.accept(id, () -> new ByteArrayInputStream(bytes));
                }
            }
        }

        @Override
        public Set<String> getNamespaces(PackType type) {
            return type == PackType.CLIENT_RESOURCES
                    ? namespaces.keySet()
                    : Set.of();
        }

        @Override
        public <T> T getMetadataSection(
                MetadataSectionType<T> type) {
            return null;
        }

        @Override
        public PackLocationInfo location() {
            return location;
        }

        @Override
        public void close() {
        }
    }
}
