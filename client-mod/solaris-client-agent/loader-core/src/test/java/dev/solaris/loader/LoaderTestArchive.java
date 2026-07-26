package dev.solaris.loader;

import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.HexFormat;
import java.util.Map;
import java.util.zip.ZipEntry;
import java.util.zip.ZipOutputStream;

final class LoaderTestArchive {
    private LoaderTestArchive() {
    }

    static byte[] screenOnly() {
        return archive(
                """
                {"schema":1,"screens":[{
                  "id":"example:welcome","title":"Welcome","body":"Hello from Solaris"
                }],"blocks":[],"items":[],"assets":[],"interactions":[]}
                """,
                Map.of());
    }

    static byte[] screenAndAsset(byte[] asset) {
        return archive(
                """
                {"schema":1,"screens":[{
                  "id":"example:welcome","title":"Welcome","body":"Hello from Solaris"
                }],"blocks":[],"items":[],"assets":[{
                  "id":"example:logo","path":"assets/example/logo.bin",
                  "sha256":"%s","size_bytes":%d
                }],"interactions":[]}
                """.formatted(sha256(asset), asset.length),
                Map.of("assets/example/logo.bin", asset));
    }

    static byte[] screenAndInteraction() {
        return archive(
                """
                {"schema":1,"screens":[{
                  "id":"example:welcome","title":"Welcome","body":"Hello from Solaris"
                }],"blocks":[],"items":[],"assets":[],"interactions":[{
                  "id":"example:continue","screen_id":"example:welcome",
                  "label":"Continue","payload":"accepted"
                }]}
                """,
                Map.of());
    }

    static byte[] screenAndItem() {
        byte[] definition = """
                {"model":{"type":"minecraft:model","model":"example:item/ruby"}}
                """.getBytes(StandardCharsets.UTF_8);
        return archive(
                """
                {"schema":1,"screens":[{
                  "id":"example:catalog","title":"Catalog","body":"A custom item",
                  "item_id":"example:ruby"
                }],"blocks":[],"items":[{
                  "id":"example:ruby","base_item":"minecraft:paper","name":"Ruby"
                }],"assets":[{
                  "id":"example:ruby_definition",
                  "path":"assets/example/items/ruby.json",
                  "sha256":"%s","size_bytes":%d
                }],"interactions":[]}
                """.formatted(sha256(definition), definition.length),
                Map.of("assets/example/items/ruby.json", definition));
    }

    static byte[] screenAndBlock() {
        byte[] model = """
                {
                  "parent":"minecraft:block/cube_all",
                  "textures":{"all":"minecraft:block/redstone_block"}
                }
                """.getBytes(StandardCharsets.UTF_8);
        return archive(
                """
                {"schema":1,"screens":[{
                  "id":"example:catalog","title":"Catalog","body":"A custom block",
                  "block_id":"example:ruby_block"
                }],"blocks":[{
                  "id":"example:ruby_block","model":"example:block/ruby_block",
                  "name":"Ruby Block"
                }],"items":[],"assets":[{
                  "id":"example:ruby_block_model",
                  "path":"assets/example/models/block/ruby_block.json",
                  "sha256":"%s","size_bytes":%d
                }],"interactions":[]}
                """.formatted(sha256(model), model.length),
                Map.of("assets/example/models/block/ruby_block.json", model));
    }

    static byte[] archive(String index, Map<String, byte[]> entries) {
        try {
            ByteArrayOutputStream bytes = new ByteArrayOutputStream();
            try (ZipOutputStream zip = new ZipOutputStream(bytes)) {
                write(zip, "solaris-client.json", index.getBytes(StandardCharsets.UTF_8));
                for (var entry : entries.entrySet()) {
                    write(zip, entry.getKey(), entry.getValue());
                }
            }
            return bytes.toByteArray();
        } catch (Exception error) {
            throw new AssertionError(error);
        }
    }

    static byte[] archiveWithLeadingEntry(String index) {
        try {
            ByteArrayOutputStream bytes = new ByteArrayOutputStream();
            try (ZipOutputStream zip = new ZipOutputStream(bytes)) {
                write(zip, "assets/example/leading.bin", new byte[] {'x'});
                write(zip, "solaris-client.json", index.getBytes(StandardCharsets.UTF_8));
            }
            return bytes.toByteArray();
        } catch (Exception error) {
            throw new AssertionError(error);
        }
    }

    static String sha256(byte[] bytes) {
        try {
            return HexFormat.of().formatHex(
                    MessageDigest.getInstance("SHA-256").digest(bytes));
        } catch (Exception error) {
            throw new AssertionError(error);
        }
    }

    private static void write(ZipOutputStream zip, String path, byte[] bytes)
            throws Exception {
        zip.putNextEntry(new ZipEntry(path));
        zip.write(bytes);
        zip.closeEntry();
    }
}
