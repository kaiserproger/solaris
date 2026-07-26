package dev.solaris.loader;

import java.util.Optional;

public record LoaderScreenDefinition(
        String id,
        String title,
        String body,
        Optional<String> itemId,
        Optional<String> blockId) {
    public LoaderScreenDefinition {
        itemId = itemId == null ? Optional.empty() : itemId;
        blockId = blockId == null ? Optional.empty() : blockId;
    }
}
