package dev.solaris.loader;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.Optional;

public final class LoaderInteractionAction {
    public static final int MAX_INTERACTION_ID_BYTES = 128;
    public static final int MAX_INTERACTION_PAYLOAD_BYTES = 4 * 1024;
    public static final int MAX_PAYLOAD_BYTES =
            6 + MAX_INTERACTION_ID_BYTES + MAX_INTERACTION_PAYLOAD_BYTES;

    private LoaderInteractionAction() {
    }

    public static Optional<byte[]> encode(
            LoaderInteractionDefinition definition,
            LoaderActivatedContent content,
            boolean connectionActive) {
        if (!connectionActive
                || !definition.equals(content.interactions().get(definition.id()))) {
            return Optional.empty();
        }
        byte[] id = definition.id().getBytes(StandardCharsets.UTF_8);
        byte[] payload = definition.payload().getBytes(StandardCharsets.UTF_8);
        if (id.length < 1
                || id.length > MAX_INTERACTION_ID_BYTES
                || payload.length > MAX_INTERACTION_PAYLOAD_BYTES) {
            return Optional.empty();
        }
        return Optional.of(ByteBuffer
                .allocate(6 + id.length + payload.length)
                .order(ByteOrder.BIG_ENDIAN)
                .putShort((short) LoaderHandshake.PROTOCOL_VERSION)
                .putShort((short) id.length)
                .put(id)
                .putShort((short) payload.length)
                .put(payload)
                .array());
    }
}
