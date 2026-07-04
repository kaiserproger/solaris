package dev.solaris.agent.client;

import java.nio.file.Path;

public interface ClientFacade {
    ClientSnapshot snapshot();

    void connect(String host, int port);

    void selectHotbarSlot(int slot);

    void lookAtBlock(int x, int y, int z, String face);

    void useItemOn(int x, int y, int z, String face);

    void moveForward(int durationMillis) throws Exception;

    Path screenshot(Path path) throws Exception;

    ClientScenarioReport runScenario(String id, Path screenshotsDir);

    void disconnect();
}
