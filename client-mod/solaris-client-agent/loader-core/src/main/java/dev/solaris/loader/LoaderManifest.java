package dev.solaris.loader;

import java.util.List;

public record LoaderManifest(int protocol, List<LoaderBundle> bundles) {
}
