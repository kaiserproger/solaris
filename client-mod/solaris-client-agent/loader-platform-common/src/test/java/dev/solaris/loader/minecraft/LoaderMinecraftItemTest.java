package dev.solaris.loader.minecraft;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import dev.solaris.loader.LoaderActivatedContent;
import dev.solaris.loader.LoaderItemDefinition;
import dev.solaris.loader.LoaderScreenDefinition;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import org.junit.jupiter.api.Test;

final class LoaderMinecraftItemTest {
    @Test
    void referencedItemResolvesFromActivatedContent() {
        LoaderScreenDefinition screen = new LoaderScreenDefinition(
                "example:catalog",
                "Catalog",
                "A custom item",
                Optional.of("example:ruby"),
                Optional.empty());
        LoaderActivatedContent content = new LoaderActivatedContent(
                List.of(),
                Map.of(screen.id(), screen),
                Map.of(),
                Map.of(
                        "example:ruby",
                        new LoaderItemDefinition(
                                "example:ruby",
                                "minecraft:paper",
                                "Ruby")),
                Map.of(),
                Map.of());

        var item = LoaderMinecraftItem.resolve(screen, content).orElseThrow();

        assertEquals("example:ruby", item.id());
        assertEquals("minecraft:paper", item.baseItem());
        assertEquals("Ruby", item.name());
    }

    @Test
    void undeclaredItemDoesNotResolve() {
        LoaderScreenDefinition screen = new LoaderScreenDefinition(
                "example:catalog",
                "Catalog",
                "A custom item",
                Optional.of("example:ruby"),
                Optional.empty());
        assertTrue(LoaderMinecraftItem
                .resolve(screen, LoaderActivatedContent.empty())
                .isEmpty());
    }
}
