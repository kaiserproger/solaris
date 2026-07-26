package dev.solaris.loader.minecraft;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import dev.solaris.loader.LoaderActivatedContent;
import dev.solaris.loader.LoaderAssetDefinition;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.atomic.AtomicBoolean;
import net.minecraft.resources.Identifier;
import net.minecraft.server.packs.PackResources;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.repository.Pack;
import org.junit.jupiter.api.Test;

final class LoaderRuntimeResourcePackTest {
    @Test
    void verifiedAssetsPublishAsOneConnectionOwnedMinecraftPack() throws Exception {
        LoaderRuntimeResourcePack runtime = new LoaderRuntimeResourcePack();
        Object origin = new Object();
        AtomicBoolean active = new AtomicBoolean(true);
        byte[] logo = "logo".getBytes(StandardCharsets.UTF_8);
        LoaderActivatedContent content = new LoaderActivatedContent(
                List.of("example:content/1/hash"),
                Map.of(),
                Map.of(),
                Map.of(),
                Map.of("example:logo", new LoaderAssetDefinition(
                        "example:logo",
                        "assets/example/textures/gui/logo.bin",
                        logo)),
                Map.of());

        runtime.publish(origin, active::get, content);
        List<Pack> packs = new ArrayList<>();
        runtime.repositorySource().loadPacks(packs::add);

        assertEquals(1, packs.size());
        assertEquals(LoaderRuntimeResourcePack.PACK_ID, packs.getFirst().getId());
        try (PackResources resources = packs.getFirst().open()) {
            Identifier location = Identifier.parse("example:textures/gui/logo.bin");
            try (InputStream stream = resources
                    .getResource(PackType.CLIENT_RESOURCES, location)
                    .get()) {
                assertArrayEquals(logo, stream.readAllBytes());
            }
            List<Identifier> listed = new ArrayList<>();
            resources.listResources(
                    PackType.CLIENT_RESOURCES,
                    "example",
                    "textures/gui",
                    (id, supplier) -> listed.add(id));
            assertEquals(List.of(location), listed);
        }

        assertFalse(runtime.clear(new Object()));
        assertTrue(runtime.owns(origin));
        active.set(false);
        packs.clear();
        runtime.repositorySource().loadPacks(packs::add);
        assertTrue(packs.isEmpty());
        active.set(true);
        assertTrue(runtime.clear(origin));
        assertFalse(runtime.owns(origin));
    }

    @Test
    void invalidOrDuplicateMinecraftResourcePathsFailBeforePublication() {
        LoaderRuntimeResourcePack runtime = new LoaderRuntimeResourcePack();
        Object origin = new Object();
        LoaderActivatedContent outsideAssets = new LoaderActivatedContent(
                List.of(),
                Map.of(),
                Map.of(),
                Map.of(),
                Map.of("example:logo", new LoaderAssetDefinition(
                        "example:logo",
                        "client/example/logo.bin",
                        new byte[] {1})),
                Map.of());
        assertThrows(
                IllegalArgumentException.class,
                () -> runtime.publish(origin, () -> true, outsideAssets));

        LoaderActivatedContent duplicatePath = new LoaderActivatedContent(
                List.of(),
                Map.of(),
                Map.of(),
                Map.of(),
                Map.of(
                        "example:first", new LoaderAssetDefinition(
                                "example:first",
                                "assets/example/shared.bin",
                                new byte[] {1}),
                        "example:second", new LoaderAssetDefinition(
                                "example:second",
                                "assets/example/shared.bin",
                                new byte[] {2})),
                Map.of());
        assertThrows(
                IllegalArgumentException.class,
                () -> runtime.publish(origin, () -> true, duplicatePath));
    }
}
