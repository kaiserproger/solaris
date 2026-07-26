package dev.solaris.loader;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import java.util.Set;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

final class LoaderContentArchiveTest {
    @TempDir
    Path cacheDirectory;

    @Test
    void verifiedArchiveActivatesClosedScreensAndAssetsBeforeAck() throws Exception {
        byte[] asset = "logo".getBytes(StandardCharsets.UTF_8);
        byte[] archive = LoaderTestArchive.screenAndAsset(asset);
        String hash = LoaderTestArchive.sha256(archive);
        Files.createDirectories(cacheDirectory.resolve("example/content/1"));
        Files.write(cacheDirectory.resolve("example/content/1/" + hash + ".bundle"), archive);

        LoaderOutgoing outgoing = new LoaderClientTransport().acceptManifest(
                manifest(
                        archive,
                        hash,
                        "[\"screens\",\"assets\"]",
                        "[\"open_screens\",\"load_assets\"]"),
                environment(Set.of(
                        LoaderPermission.OPEN_SCREENS,
                        LoaderPermission.LOAD_ASSETS)),
                cacheDirectory);

        assertEquals(LoaderOutgoing.Kind.ACKNOWLEDGEMENT, outgoing.kind());
        LoaderActivatedContent active = outgoing.activatedContent();
        assertEquals("Welcome", active.screens().get("example:welcome").title());
        assertArrayEquals(asset, active.assets().get("example:logo").bytes());
        assertEquals(1, active.cacheKeys().size());
        byte[] returnedBytes = active.assets().get("example:logo").bytes();
        returnedBytes[0] = 'X';
        assertArrayEquals(asset, active.assets().get("example:logo").bytes());
        assertThrows(UnsupportedOperationException.class, active.assets()::clear);
    }

    @Test
    void verifiedInteractionIsOwnedBoundedAndEncodesOnlyWhileActive() throws Exception {
        byte[] archive = LoaderTestArchive.screenAndInteraction();
        String hash = LoaderTestArchive.sha256(archive);
        writeCache(archive);

        LoaderOutgoing outgoing = new LoaderClientTransport()
                .acceptManifest(
                        manifest(
                                archive,
                                hash,
                                "[\"screens\",\"interactions\"]",
                                "[\"open_screens\",\"send_interactions\"]"),
                        environment(Set.of(
                                LoaderPermission.OPEN_SCREENS,
                                LoaderPermission.SEND_INTERACTIONS)),
                        cacheDirectory);
        LoaderActivatedContent active = outgoing.activatedContent();

        LoaderInteractionDefinition interaction =
                active.interactions().get("example:continue");
        assertEquals("example:welcome", interaction.screenId());
        assertEquals("Continue", interaction.label());
        assertTrue(LoaderInteractionAction.encode(interaction, active, true).isPresent());
        assertTrue(LoaderInteractionAction
                .encode(interaction, LoaderActivatedContent.empty(), true)
                .isEmpty());
        assertTrue(LoaderInteractionAction.encode(interaction, active, false).isEmpty());
        assertThrows(UnsupportedOperationException.class, active.interactions()::clear);
    }

    @Test
    void verifiedItemDefinitionActivatesForItsReferencingScreen() throws Exception {
        byte[] archive = LoaderTestArchive.screenAndItem();
        String hash = LoaderTestArchive.sha256(archive);
        writeCache(archive);

        LoaderOutgoing outgoing = new LoaderClientTransport()
                .acceptManifest(
                        manifest(
                                archive,
                                hash,
                                "[\"screens\",\"items\",\"assets\"]",
                                "[\"open_screens\",\"register_items\",\"load_assets\"]"),
                        environment(Set.of(
                                LoaderPermission.OPEN_SCREENS,
                                LoaderPermission.REGISTER_ITEMS,
                                LoaderPermission.LOAD_ASSETS)),
                        cacheDirectory);
        LoaderActivatedContent active = outgoing.activatedContent();

        LoaderItemDefinition item = active.items().get("example:ruby");
        assertEquals("minecraft:paper", item.baseItem());
        assertEquals("Ruby", item.name());
        assertEquals(
                "example:ruby",
                active.screens().get("example:catalog").itemId().orElseThrow());
        assertTrue(active.assets().containsKey("example:ruby_definition"));
        assertThrows(UnsupportedOperationException.class, active.items()::clear);
    }

    @Test
    void itemRequiresItsOwnedDefinitionAsset() throws Exception {
        byte[] archive = LoaderTestArchive.archive(
                """
                {"schema":1,"screens":[],"blocks":[],"items":[{
                  "id":"example:ruby","base_item":"minecraft:paper","name":"Ruby"
                }],"assets":[],"interactions":[]}
                """,
                Map.of());
        writeCache(archive);

        assertThrows(
                IllegalArgumentException.class,
                () -> new LoaderClientTransport().acceptManifest(
                        manifest(
                                archive,
                                LoaderTestArchive.sha256(archive),
                                "[\"items\"]",
                                "[\"register_items\"]"),
                        environment(Set.of(LoaderPermission.REGISTER_ITEMS)),
                        cacheDirectory));
    }

    @Test
    void verifiedBlockModelActivatesForItsReferencingScreen() throws Exception {
        byte[] archive = LoaderTestArchive.screenAndBlock();
        String hash = LoaderTestArchive.sha256(archive);
        writeCache(archive);

        LoaderOutgoing outgoing = new LoaderClientTransport()
                .acceptManifest(
                        manifest(
                                archive,
                                hash,
                                "[\"screens\",\"blocks\",\"assets\"]",
                                "[\"open_screens\",\"register_blocks\",\"load_assets\"]"),
                        environment(Set.of(
                                LoaderPermission.OPEN_SCREENS,
                                LoaderPermission.REGISTER_BLOCKS,
                                LoaderPermission.LOAD_ASSETS)),
                        cacheDirectory);
        LoaderActivatedContent active = outgoing.activatedContent();

        assertTrue(new String(outgoing.bytes(), StandardCharsets.UTF_8)
                .contains("\"carrier_block_state_ids\":{\"example:ruby_block\":321}"));
        LoaderBlockDefinition block = active.blocks().get("example:ruby_block");
        assertEquals("example:block/ruby_block", block.model());
        assertEquals("Ruby Block", block.name());
        assertEquals(
                "example:ruby_block",
                active.screens().get("example:catalog").blockId().orElseThrow());
        assertTrue(active.assets().containsKey("example:ruby_block_model"));
        assertThrows(UnsupportedOperationException.class, active.blocks()::clear);
    }

    @Test
    void blockRequiresItsOwnedModelAsset() throws Exception {
        byte[] archive = LoaderTestArchive.archive(
                """
                {"schema":1,"screens":[],"blocks":[{
                  "id":"example:ruby_block","model":"example:block/ruby_block",
                  "name":"Ruby Block"
                }],"items":[],"assets":[],"interactions":[]}
                """,
                Map.of());
        writeCache(archive);

        assertThrows(
                IllegalArgumentException.class,
                () -> new LoaderClientTransport().acceptManifest(
                        manifest(
                                archive,
                                LoaderTestArchive.sha256(archive),
                                "[\"blocks\"]",
                                "[\"register_blocks\"]"),
                        environment(Set.of(LoaderPermission.REGISTER_BLOCKS)),
                        cacheDirectory));
    }

    @Test
    void interactionMustBeOwnedAndReferenceItsDeclaredScreen() throws Exception {
        byte[] foreign = LoaderTestArchive.archive(
                """
                {"schema":1,"screens":[{
                  "id":"example:welcome","title":"Welcome","body":"Body"
                }],"blocks":[],"items":[],"assets":[],"interactions":[{
                  "id":"other:continue","screen_id":"example:welcome",
                  "label":"Continue","payload":"accepted"
                }]}
                """,
                Map.of());
        writeCache(foreign);
        assertThrows(
                IllegalArgumentException.class,
                () -> new LoaderClientTransport().acceptManifest(
                        manifest(
                                foreign,
                                LoaderTestArchive.sha256(foreign),
                                "[\"screens\",\"interactions\"]",
                                "[\"open_screens\",\"send_interactions\"]"),
                        environment(Set.of(
                                LoaderPermission.OPEN_SCREENS,
                                LoaderPermission.SEND_INTERACTIONS)),
                        cacheDirectory));

        byte[] missingScreen = LoaderTestArchive.archive(
                """
                {"schema":1,"screens":[{
                  "id":"example:welcome","title":"Welcome","body":"Body"
                }],"blocks":[],"items":[],"assets":[],"interactions":[{
                  "id":"example:continue","screen_id":"example:missing",
                  "label":"Continue","payload":"accepted"
                }]}
                """,
                Map.of());
        writeCache(missingScreen);
        assertThrows(
                IllegalArgumentException.class,
                () -> new LoaderClientTransport().acceptManifest(
                        manifest(
                                missingScreen,
                                LoaderTestArchive.sha256(missingScreen),
                                "[\"screens\",\"interactions\"]",
                                "[\"open_screens\",\"send_interactions\"]"),
                        environment(Set.of(
                                LoaderPermission.OPEN_SCREENS,
                                LoaderPermission.SEND_INTERACTIONS)),
                        cacheDirectory));
    }

    @Test
    void unknownContentIsRejectedBeforeRequestOrStaging() {
        byte[] archive = LoaderTestArchive.screenOnly();
        String hash = LoaderTestArchive.sha256(archive);

        assertThrows(
                IllegalArgumentException.class,
                () -> new LoaderClientTransport().acceptManifest(
                        manifest(
                                archive,
                                hash,
                                "[\"future\"]",
                                "[\"register_blocks\"]"),
                        environment(Set.of(LoaderPermission.REGISTER_BLOCKS)),
                        cacheDirectory));
        assertFalse(Files.exists(cacheDirectory.resolve("example")));
    }

    @Test
    void closedIndexRejectsUnknownFieldsBeforeAcknowledgement() throws Exception {
        byte[] archive = LoaderTestArchive.archive(
                """
                {"schema":1,"screens":[{
                  "id":"example:welcome","title":"Welcome","body":"Body"
                }],"blocks":[],"items":[],"assets":[],"interactions":[],"future":true}
                """,
                Map.of());
        writeCache(archive);

        assertThrows(
                IllegalArgumentException.class,
                () -> new LoaderClientTransport().acceptManifest(
                        manifest(
                                archive,
                                LoaderTestArchive.sha256(archive),
                                "[\"screens\"]",
                                "[\"open_screens\"]"),
                        environment(Set.of(LoaderPermission.OPEN_SCREENS)),
                        cacheDirectory));
    }

    @Test
    void exactAssetBytesAreVerifiedBeforeAcknowledgement() throws Exception {
        byte[] expectedAsset = new byte[] {'x'};
        byte[] archive = LoaderTestArchive.archive(
                """
                {"schema":1,"screens":[],"blocks":[],"items":[],"assets":[{
                  "id":"example:logo","path":"assets/example/logo.bin",
                  "sha256":"%s","size_bytes":1
                }],"interactions":[]}
                """.formatted(LoaderTestArchive.sha256(expectedAsset)),
                Map.of("assets/example/logo.bin", new byte[] {'y'}));
        writeCache(archive);

        assertThrows(
                IllegalArgumentException.class,
                () -> new LoaderClientTransport().acceptManifest(
                        manifest(
                                archive,
                                LoaderTestArchive.sha256(archive),
                                "[\"assets\"]",
                                "[\"load_assets\"]"),
                        environment(Set.of(LoaderPermission.LOAD_ASSETS)),
                        cacheDirectory));
    }

    @Test
    void indexRequiresExactJsonTypes() throws Exception {
        byte[] stringSchema = LoaderTestArchive.archive(
                """
                {"schema":"1","screens":[{
                  "id":"example:welcome","title":"Welcome","body":"Body"
                }],"blocks":[],"items":[],"assets":[],"interactions":[]}
                """,
                Map.of());
        writeCache(stringSchema);
        assertThrows(
                IllegalArgumentException.class,
                () -> new LoaderClientTransport().acceptManifest(
                        manifest(
                                stringSchema,
                                LoaderTestArchive.sha256(stringSchema),
                                "[\"screens\"]",
                                "[\"open_screens\"]"),
                        environment(Set.of(LoaderPermission.OPEN_SCREENS)),
                        cacheDirectory));

        byte[] numericTitle = LoaderTestArchive.archive(
                """
                {"schema":1,"screens":[{
                  "id":"example:welcome","title":7,"body":"Body"
                }],"blocks":[],"items":[],"assets":[],"interactions":[]}
                """,
                Map.of());
        writeCache(numericTitle);
        assertThrows(
                IllegalArgumentException.class,
                () -> new LoaderClientTransport().acceptManifest(
                        manifest(
                                numericTitle,
                                LoaderTestArchive.sha256(numericTitle),
                                "[\"screens\"]",
                                "[\"open_screens\"]"),
                        environment(Set.of(LoaderPermission.OPEN_SCREENS)),
                        cacheDirectory));
    }

    @Test
    void archiveMustStartWithItsIndexAndUseCanonicalDeclaredPaths() throws Exception {
        byte[] leadingEntry = LoaderTestArchive.archiveWithLeadingEntry(
                """
                {"schema":1,"screens":[{
                  "id":"example:welcome","title":"Welcome","body":"Body"
                }],"blocks":[],"items":[],"assets":[],"interactions":[]}
                """);
        writeCache(leadingEntry);
        assertThrows(
                IllegalArgumentException.class,
                () -> new LoaderClientTransport().acceptManifest(
                        manifest(
                                leadingEntry,
                                LoaderTestArchive.sha256(leadingEntry),
                                "[\"screens\"]",
                                "[\"open_screens\"]"),
                        environment(Set.of(LoaderPermission.OPEN_SCREENS)),
                        cacheDirectory));

        byte[] escapingPath = LoaderTestArchive.archive(
                """
                {"schema":1,"screens":[],"blocks":[],"items":[],"assets":[{
                  "id":"example:logo","path":"assets/../logo.bin",
                  "sha256":"%s","size_bytes":1
                }],"interactions":[]}
                """.formatted(LoaderTestArchive.sha256(new byte[] {'x'})),
                Map.of("assets/../logo.bin", new byte[] {'x'}));
        writeCache(escapingPath);
        assertThrows(
                IllegalArgumentException.class,
                () -> new LoaderClientTransport().acceptManifest(
                        manifest(
                                escapingPath,
                                LoaderTestArchive.sha256(escapingPath),
                                "[\"assets\"]",
                                "[\"load_assets\"]"),
                        environment(Set.of(LoaderPermission.LOAD_ASSETS)),
                        cacheDirectory));
    }

    @Test
    void combinedRegistryLimitsAreClosed() {
        assertDoesNotThrow(() -> LoaderContentArchive.ensureRegistryBounds(
                64,
                8,
                128,
                128,
                64,
                64L * 1024L * 1024L));
        assertThrows(
                IllegalArgumentException.class,
                () -> LoaderContentArchive.ensureRegistryBounds(
                        65, 1, 128, 128, 64, 0));
        assertThrows(
                IllegalArgumentException.class,
                () -> LoaderContentArchive.ensureRegistryBounds(
                        64, 9, 128, 128, 64, 0));
        assertThrows(
                IllegalArgumentException.class,
                () -> LoaderContentArchive.ensureRegistryBounds(
                        64, 1, 129, 128, 64, 0));
        assertThrows(
                IllegalArgumentException.class,
                () -> LoaderContentArchive.ensureRegistryBounds(
                        64, 1, 128, 129, 64, 0));
        assertThrows(
                IllegalArgumentException.class,
                () -> LoaderContentArchive.ensureRegistryBounds(
                        64, 1, 128, 128, 65, 0));
        assertThrows(
                IllegalArgumentException.class,
                () -> LoaderContentArchive.ensureRegistryBounds(
                        64,
                        1,
                        128,
                        128,
                        64,
                        64L * 1024L * 1024L + 1));
    }

    private void writeCache(byte[] archive) throws Exception {
        String hash = LoaderTestArchive.sha256(archive);
        Files.createDirectories(cacheDirectory.resolve("example/content/1"));
        Files.write(cacheDirectory.resolve("example/content/1/" + hash + ".bundle"), archive);
    }

    private static LoaderEnvironment environment(Set<LoaderPermission> permissions) {
        return new LoaderEnvironment() {
            @Override
            public LoaderPlatform platform() {
                return LoaderPlatform.FABRIC;
            }

            @Override
            public String loaderVersion() {
                return "0.1.0";
            }

            @Override
            public Set<LoaderPermission> grantedPermissions() {
                return permissions;
            }

            @Override
            public List<Integer> carrierBlockStateIds() {
                return List.of(321, 654);
            }
        };
    }

    private static byte[] manifest(
            byte[] archive,
            String hash,
            String content,
            String permissions) {
        return """
                {"protocol":1,"bundles":[{
                  "owner":"example","id":"content","version":"1",
                  "artifact":"client/content.zip","sha256":"%s","size_bytes":%d,
                  "loaders":["fabric"],"content":%s,"permissions":%s,
                  "cache_key":"example:content/1/%s"
                }]}
                """.formatted(hash, archive.length, content, permissions, hash)
                .getBytes(StandardCharsets.UTF_8);
    }
}
