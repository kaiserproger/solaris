package dev.solaris.loader;

public record LoaderInteractionDefinition(
        String id,
        String screenId,
        String label,
        String payload) {
}
