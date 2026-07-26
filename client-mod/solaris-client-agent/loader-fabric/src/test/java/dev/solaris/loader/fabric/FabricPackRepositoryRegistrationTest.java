package dev.solaris.loader.fabric;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.charset.StandardCharsets;
import java.util.Set;
import net.minecraft.server.packs.repository.RepositorySource;
import org.junit.jupiter.api.Test;

final class FabricPackRepositoryRegistrationTest {
    @Test
    void immutableVanillaSourcesAreCopiedBeforeRuntimeRegistration() {
        RepositorySource vanilla = output -> {};
        RepositorySource runtime = output -> {};

        Set<RepositorySource> sources =
                SolarisFabricLoader.mutablePackSources(Set.of(vanilla), runtime);

        assertEquals(Set.of(vanilla, runtime), sources);
        assertTrue(sources.add(output -> {}));
    }

    @Test
    void blockCarrierUsesThePreFreezeMainEntrypoint() throws Exception {
        try (var input = getClass().getResourceAsStream("/fabric.mod.json")) {
            assertNotNull(input);
            String descriptor = new String(
                    input.readAllBytes(),
                    StandardCharsets.UTF_8);
            assertTrue(descriptor.contains("\"main\""));
            assertTrue(descriptor.contains(
                    "dev.solaris.loader.fabric.SolarisFabricContent"));
        }
    }
}
