package dev.solaris.agent.javaagent;

final class DisconnectSequence {
    private DisconnectSequence() {
    }

    static void run(Runnable closeNetworkConnection, Runnable clearClientState) {
        closeNetworkConnection.run();
        clearClientState.run();
    }
}
