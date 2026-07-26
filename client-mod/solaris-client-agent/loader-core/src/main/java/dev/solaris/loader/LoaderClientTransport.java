package dev.solaris.loader;

import java.nio.file.Path;
import java.util.Optional;

public final class LoaderClientTransport {
    private LoaderTransferSession session;

    public synchronized LoaderOutgoing acceptManifest(
            byte[] payload,
            LoaderEnvironment environment,
            Path cacheDirectory) {
        if (session != null) {
            session.abort();
        }
        session = LoaderTransferSession.begin(payload, environment, cacheDirectory);
        return session.nextOutgoing();
    }

    public synchronized Optional<LoaderOutgoing> acceptArtifact(byte[] payload) {
        if (session == null) {
            throw new IllegalArgumentException("received a Solaris Loader artifact before its manifest");
        }
        return session.acceptArtifact(payload);
    }

    public synchronized void abort() {
        if (session != null) {
            session.abort();
            session = null;
        }
    }
}
