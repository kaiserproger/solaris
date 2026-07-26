package dev.solaris.loader.forge;

import dev.solaris.loader.LoaderHandshake;
import net.minecraft.network.FriendlyByteBuf;
import net.minecraft.network.codec.StreamCodec;
import net.minecraft.network.protocol.common.custom.CustomPacketPayload;
import net.minecraft.resources.Identifier;

public record LoaderAckPayload(byte[] bytes) implements CustomPacketPayload {
    public static final Type<LoaderAckPayload> TYPE =
            new Type<>(Identifier.fromNamespaceAndPath("solaris", "loader/ack"));
    public static final StreamCodec<FriendlyByteBuf, LoaderAckPayload> CODEC =
            StreamCodec.of(
                    (buffer, payload) -> buffer.writeBytes(payload.bytes),
                    buffer -> new LoaderAckPayload(readBytes(buffer)));

    public LoaderAckPayload {
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
            throw new IllegalArgumentException("invalid Solaris Loader acknowledgement length " + length);
        }
        byte[] bytes = new byte[length];
        buffer.readBytes(bytes);
        return bytes;
    }
}
