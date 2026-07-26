package dev.solaris.loader;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Set;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

final class LoaderLiveGateFixtureTest {
    private static final String CONTENT =
            "[\"blocks\",\"items\",\"screens\",\"assets\",\"interactions\"]";
    private static final String PERMISSIONS =
            "[\"register_blocks\",\"register_items\",\"open_screens\","
                    + "\"load_assets\",\"send_interactions\"]";

    @TempDir
    Path cacheDirectory;

    @Test
    void shippedTwoOwnerArchivesActivateAllFiveContentKindsTogether() throws Exception {
        byte[] ruby = archive("ruby-live");
        byte[] sapphire = archive("sapphire-live");
        String rubyHash = LoaderTestArchive.sha256(ruby);
        String sapphireHash = LoaderTestArchive.sha256(sapphire);
        writeCache("ruby-live", rubyHash, ruby);
        writeCache("sapphire-live", sapphireHash, sapphire);

        byte[] manifest = """
                {"protocol":1,"bundles":[
                  %s,
                  %s
                ]}
                """.formatted(
                        bundle("ruby-live", rubyHash, ruby.length),
                        bundle("sapphire-live", sapphireHash, sapphire.length))
                .getBytes(StandardCharsets.UTF_8);

        LoaderOutgoing outgoing =
                new LoaderClientTransport().acceptManifest(manifest, environment(), cacheDirectory);
        assertEquals(LoaderOutgoing.Kind.ACKNOWLEDGEMENT, outgoing.kind());
        LoaderActivatedContent active = outgoing.activatedContent();
        assertEquals(2, active.cacheKeys().size());
        assertEquals(2, active.screens().size());
        assertEquals(2, active.blocks().size());
        assertEquals(2, active.items().size());
        assertEquals(4, active.assets().size());
        assertEquals(2, active.interactions().size());
        assertEquals(
                "ruby-live:ruby",
                active.screens().get("ruby-live:showcase").itemId().orElseThrow());
        assertEquals(
                "sapphire-live:sapphire_block",
                active.screens().get("sapphire-live:showcase").blockId().orElseThrow());
        assertTrue(active.interactions().containsKey("ruby-live:confirm"));
        assertTrue(active.interactions().containsKey("sapphire-live:confirm"));
    }

    private static byte[] archive(String owner) throws Exception {
        Path root = Path.of(System.getProperty("solaris.repoRoot"));
        return Files.readAllBytes(root.resolve(
                "examples/loader-live-gate/plugins/" + owner + "/client/rich-content.zip"));
    }

    private void writeCache(String owner, String hash, byte[] archive) throws Exception {
        Path directory = cacheDirectory.resolve(owner + "/rich-content/1");
        Files.createDirectories(directory);
        Files.write(directory.resolve(hash + ".bundle"), archive);
    }

    private static String bundle(String owner, String hash, int size) {
        return """
                {"owner":"%1$s","id":"rich-content","version":"1",
                 "artifact":"client/rich-content.zip","sha256":"%2$s","size_bytes":%3$d,
                 "loaders":["fabric","neoforge","forge"],"content":%4$s,
                 "permissions":%5$s,"cache_key":"%1$s:rich-content/1/%2$s"}
                """.formatted(owner, hash, size, CONTENT, PERMISSIONS);
    }

    private static LoaderEnvironment environment() {
        return new LoaderEnvironment() {
            @Override
            public LoaderPlatform platform() {
                return LoaderPlatform.FABRIC;
            }

            @Override
            public String loaderVersion() {
                return "0.19.3";
            }

            @Override
            public Set<LoaderPermission> grantedPermissions() {
                return Set.of(
                        LoaderPermission.REGISTER_BLOCKS,
                        LoaderPermission.REGISTER_ITEMS,
                        LoaderPermission.OPEN_SCREENS,
                        LoaderPermission.LOAD_ASSETS,
                        LoaderPermission.SEND_INTERACTIONS);
            }

            @Override
            public List<Integer> carrierBlockStateIds() {
                return List.of(321, 654);
            }
        };
    }
}
