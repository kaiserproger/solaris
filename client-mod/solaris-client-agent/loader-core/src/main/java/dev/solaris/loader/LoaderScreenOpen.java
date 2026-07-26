package dev.solaris.loader;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.Optional;

public final class LoaderScreenOpen {
    public static final int MAX_SCREEN_ID_BYTES = 128;
    public static final int MAX_PAYLOAD_BYTES = 4 + MAX_SCREEN_ID_BYTES;

    private LoaderScreenOpen() {
    }

    public static Optional<LoaderScreenDefinition> resolve(
            byte[] payload,
            LoaderActivatedContent content,
            boolean connectionActive) {
        if (!connectionActive || payload.length < 5 || payload.length > MAX_PAYLOAD_BYTES) {
            return Optional.empty();
        }
        ByteBuffer buffer = ByteBuffer.wrap(payload).order(ByteOrder.BIG_ENDIAN);
        int protocol = Short.toUnsignedInt(buffer.getShort());
        int idLength = Short.toUnsignedInt(buffer.getShort());
        if (protocol != LoaderHandshake.PROTOCOL_VERSION
                || idLength < 1
                || idLength > MAX_SCREEN_ID_BYTES
                || buffer.remaining() != idLength) {
            return Optional.empty();
        }
        byte[] idBytes = new byte[idLength];
        buffer.get(idBytes);
        String id = new String(idBytes, StandardCharsets.UTF_8);
        if (!Arrays.equals(id.getBytes(StandardCharsets.UTF_8), idBytes)) {
            return Optional.empty();
        }
        return Optional.ofNullable(content.screens().get(id));
    }
}
