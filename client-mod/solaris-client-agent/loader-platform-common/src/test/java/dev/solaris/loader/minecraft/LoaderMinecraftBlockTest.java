package dev.solaris.loader.minecraft;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import dev.solaris.loader.LoaderActivatedContent;
import dev.solaris.loader.LoaderBlockDefinition;
import dev.solaris.loader.LoaderScreenDefinition;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import net.minecraft.resources.Identifier;
import org.junit.jupiter.api.Test;

final class LoaderMinecraftBlockTest {
    @Test
    void referencedBlockResolvesAndGeneratesCarrierModels() {
        LoaderScreenDefinition screen = new LoaderScreenDefinition(
                "example:catalog",
                "Catalog",
                "A custom block",
                Optional.empty(),
                Optional.of("example:ruby_block"));
        LoaderBlockDefinition block = new LoaderBlockDefinition(
                "example:ruby_block",
                "example:block/ruby_block",
                "Ruby Block");
        LoaderBlockDefinition second = new LoaderBlockDefinition(
                "other:sapphire_block",
                "other:block/sapphire_block",
                "Sapphire Block");
        LoaderActivatedContent content = new LoaderActivatedContent(
                List.of(),
                Map.of(screen.id(), screen),
                Map.of(block.id(), block, second.id(), second),
                Map.of(),
                Map.of(),
                Map.of());

        assertEquals(
                block,
                LoaderMinecraftBlock.resolve(screen, content).orElseThrow());
        Map<Identifier, byte[]> resources =
                LoaderMinecraftBlock.generatedResources(content);
        assertEquals(4, resources.size());
        assertTrue(new String(
                        resources.get(Identifier.parse(
                                "solaris_loader:blockstates/loader_block.json")),
                        StandardCharsets.UTF_8)
                .contains("\"model\":\"example:block/ruby_block\""));
        assertTrue(new String(
                        resources.get(Identifier.parse(
                                "solaris_loader:blockstates/loader_block_1.json")),
                        StandardCharsets.UTF_8)
                .contains("\"model\":\"other:block/sapphire_block\""));
        assertTrue(new String(
                        resources.get(Identifier.parse(
                                "solaris_loader:items/loader_block.json")),
                        StandardCharsets.UTF_8)
                .contains("\"model\":\"example:block/ruby_block\""));
        assertTrue(new String(
                        resources.get(Identifier.parse(
                                "solaris_loader:items/loader_block_1.json")),
                        StandardCharsets.UTF_8)
                .contains("\"model\":\"other:block/sapphire_block\""));
    }

    @Test
    void undeclaredBlockDoesNotResolveOrGenerateResources() {
        LoaderScreenDefinition screen = new LoaderScreenDefinition(
                "example:catalog",
                "Catalog",
                "A custom block",
                Optional.empty(),
                Optional.of("example:ruby_block"));

        assertTrue(LoaderMinecraftBlock
                .resolve(screen, LoaderActivatedContent.empty())
                .isEmpty());
        assertTrue(LoaderMinecraftBlock
                .generatedResources(LoaderActivatedContent.empty())
                .isEmpty());
    }
}
