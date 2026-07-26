package dev.solaris.loader.neoforge;

import dev.solaris.loader.LoaderHandshake;
import net.minecraft.network.FriendlyByteBuf;
import net.minecraft.network.codec.StreamCodec;
import net.minecraft.network.protocol.common.custom.CustomPacketPayload;
import net.minecraft.resources.Identifier;

public record LoaderArtifactPayload(byte[] bytes) implements CustomPacketPayload {
    public static final Type<LoaderArtifactPayload> TYPE =
            new Type<>(Identifier.fromNamespaceAndPath("solaris", "loader/artifact"));
    public static final StreamCodec<FriendlyByteBuf, LoaderArtifactPayload> CODEC =
            StreamCodec.of(
                    (buffer, payload) -> buffer.writeBytes(payload.bytes),
                    buffer -> new LoaderArtifactPayload(readBytes(buffer)));

    public LoaderArtifactPayload {
        bytes = bytes.clone();
    }

    @Override
    public byte[] bytes() {
        return bytes.clone();
    }

    @Override
    public Type<? extends CustomPacketPayload> type() {
        return TYPE;
    }

    private static byte[] readBytes(FriendlyByteBuf buffer) {
        int length = buffer.readableBytes();
        if (length <= 0 || length > LoaderHandshake.MAX_MANIFEST_BYTES) {
            throw new IllegalArgumentException("invalid Solaris Loader artifact length " + length);
        }
        byte[] bytes = new byte[length];
        buffer.readBytes(bytes);
        return bytes;
    }
}
