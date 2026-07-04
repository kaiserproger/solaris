package dev.solaris.agent.javaagent;

import org.junit.jupiter.api.Test;

import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

final class MinecraftClientFacadeTest {
    @Test
    void screenshotBaseDirectoryLetsVanillaWriteRequestedScreenshotsPath() {
        assertEquals(
            Path.of("run"),
            MinecraftClientFacade.screenshotBaseDirectory(Path.of("run/screenshots/m94-02b.png"))
        );
        assertEquals(
            Path.of("."),
            MinecraftClientFacade.screenshotBaseDirectory(Path.of("screenshots/m94-02b.png"))
        );
    }

    @Test
    void screenshotBaseDirectoryRejectsPathsOutsideScreenshotsDirectory() {
        assertThrows(
            IllegalArgumentException.class,
            () -> MinecraftClientFacade.screenshotBaseDirectory(Path.of("run/m94-02b.png"))
        );
        assertThrows(
            IllegalArgumentException.class,
            () -> MinecraftClientFacade.screenshotBaseDirectory(Path.of("m94-02b.png"))
        );
    }

    @Test
    void disconnectClosesNetworkBeforeClearingClientState() {
        List<String> calls = new ArrayList<>();

        DisconnectSequence.run(
            () -> calls.add("network"),
            () -> calls.add("client")
        );

        assertEquals(List.of("network", "client"), calls);
    }
}
